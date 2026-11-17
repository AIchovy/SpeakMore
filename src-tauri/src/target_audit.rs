//! Explicit development app. Never starts the product runtime, microphone,
//! workers, database, clipboard or event poster. Reuses the production probe.
use crate::{session_runtime::NativeInput, target, workspace_lifecycle};
use core_foundation::{
    base::TCFType,
    boolean::CFBoolean,
    dictionary::CFDictionary,
    string::{CFString, CFStringRef},
};
use serde::Serialize;
use speakmore_core::{
    delivery::ManualReason,
    error::{AppError, Result},
};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{Manager, State, WebviewWindow};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub(crate) const IDENTIFIER: &str = "com.speakmore.inputcheck.20260916";
const PREPARE: Duration = Duration::from_secs(8);
const OBSERVE: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Idle,
    Countdown,
    Capturing,
    Observing,
    Passed,
    Failed,
    Cancelled,
}
impl Phase {
    fn active(self) -> bool {
        matches!(self, Self::Countdown | Self::Capturing | Self::Observing)
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    revision: u64,
    phase: Phase,
    accessibility: bool,
    remaining_ms: u64,
    reason: Option<&'static str>,
    operation: Option<&'static str>,
    native_error: Option<i32>,
}
struct Data {
    revision: u64,
    id: Uuid,
    phase: Phase,
    until: Instant,
    reason: Option<&'static str>,
    operation: Option<&'static str>,
    native_error: Option<i32>,
    cancel: CancellationToken,
    lease: Option<Arc<target::Lease>>,
}
struct Audit {
    data: Mutex<Data>,
    service: Arc<target::Service>,
    input: NativeInput,
}
impl Audit {
    fn snapshot(&self) -> Snapshot {
        let data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        Snapshot {
            revision: data.revision,
            phase: data.phase,
            accessibility: crate::permissions::read().accessibility,
            remaining_ms: if data.phase.active() {
                data.until
                    .saturating_duration_since(Instant::now())
                    .as_millis() as u64
            } else {
                0
            },
            reason: data.reason,
            operation: data.operation,
            native_error: data.native_error,
        }
    }
    fn publish(&self, id: Uuid, phase: Phase, reason: Option<ManualReason>, duration: Duration) {
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        if data.id != id || data.cancel.is_cancelled() {
            return;
        }
        data.revision += 1;
        data.phase = phase;
        data.reason = reason.map(ManualReason::as_str);
        data.operation = (phase == Phase::Failed).then(target::diagnostics::last);
        data.native_error = (phase == Phase::Failed)
            .then(target::diagnostics::error_code)
            .flatten();
        data.until = Instant::now() + duration;
    }
    fn cancel(&self) {
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        data.cancel.cancel();
        if let Some(lease) = data.lease.take() {
            lease.end();
        }
        if data.phase.active() {
            data.revision += 1;
            data.phase = Phase::Cancelled;
            data.reason = None;
        }
    }
    fn begin(self: &Arc<Self>) -> Result<()> {
        self.begin_with_permission(crate::permissions::read().accessibility)
    }
    fn begin_with_permission(self: &Arc<Self>, allowed: bool) -> Result<()> {
        let (id, cancel) = {
            let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
            if data.phase.active() {
                return Err(AppError::busy());
            }
            if !allowed {
                return Err(AppError::new(
                    "AccessibilityRequired",
                    "请先允许独立验证应用访问辅助功能。",
                    true,
                ));
            }
            let id = Uuid::new_v4();
            let cancel = CancellationToken::new();
            *data = Data {
                revision: data.revision + 1,
                id,
                phase: Phase::Countdown,
                until: Instant::now() + PREPARE,
                reason: None,
                operation: None,
                native_error: None,
                cancel: cancel.clone(),
                lease: None,
            };
            (id, cancel)
        };
        let audit = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::select! { biased; _ = cancel.cancelled() => return, _ = tokio::time::sleep(PREPARE) => {} }
            audit.publish(id, Phase::Capturing, None, Duration::from_millis(300));
            let lease = {
                let mut data = audit.data.lock().unwrap_or_else(|e| e.into_inner());
                if data.id != id || cancel.is_cancelled() {
                    return;
                }
                let lease = Arc::new(audit.service.begin(
                    id,
                    audit.input.target_generation(),
                    Instant::now(),
                ));
                data.lease = Some(lease.clone());
                lease
            };
            let mut observed_at = None;
            loop {
                if cancel.is_cancelled() {
                    break;
                }
                match lease.audit_outcome() {
                    Some(Err(reason)) => {
                        audit.publish(id, Phase::Failed, Some(reason), Duration::ZERO);
                        break;
                    }
                    Some(Ok(())) if observed_at.is_none() => {
                        observed_at = Some(Instant::now());
                        audit.publish(id, Phase::Observing, None, OBSERVE);
                    }
                    _ => {}
                }
                if observed_at.is_some_and(|at: Instant| at.elapsed() >= OBSERVE) {
                    // New post-observation read, not the previous heartbeat.
                    match lease.recheck() {
                        Ok(check) => loop {
                            if cancel.is_cancelled() {
                                break;
                            }
                            if let Some(result) = check.poll() {
                                match result.and_then(|()| {
                                    lease
                                        .audit_outcome()
                                        .unwrap_or(Err(ManualReason::TargetUnavailable))
                                }) {
                                    Ok(()) => {
                                        audit.publish(id, Phase::Passed, None, Duration::ZERO)
                                    }
                                    Err(reason) => audit.publish(
                                        id,
                                        Phase::Failed,
                                        Some(reason),
                                        Duration::ZERO,
                                    ),
                                }
                                break;
                            }
                            tokio::select! { biased; _ = cancel.cancelled() => break, _ = tokio::time::sleep(Duration::from_millis(20)) => {} }
                        },
                        Err(reason) => {
                            audit.publish(id, Phase::Failed, Some(reason), Duration::ZERO)
                        }
                    }
                    break;
                }
                tokio::select! { biased; _ = cancel.cancelled() => break, _ = tokio::time::sleep(Duration::from_millis(20)) => {} }
            }
            lease.end();
        });
        Ok(())
    }
}

#[tauri::command]
fn audit_status(window: WebviewWindow, state: State<'_, Arc<Audit>>) -> Result<Snapshot> {
    crate::main_only(&window)?;
    Ok(state.snapshot())
}
#[tauri::command]
fn audit_begin(window: WebviewWindow, state: State<'_, Arc<Audit>>) -> Result<()> {
    crate::main_only(&window)?;
    state.inner().begin()
}
#[tauri::command]
fn audit_cancel(window: WebviewWindow, state: State<'_, Arc<Audit>>) -> Result<()> {
    crate::main_only(&window)?;
    state.cancel();
    Ok(())
}
#[tauri::command]
async fn audit_permission_action(
    window: WebviewWindow,
    state: State<'_, Arc<Audit>>,
    request: bool,
) -> Result<()> {
    crate::main_only(&window)?;
    if state.snapshot().phase.active() {
        return Err(AppError::busy());
    }
    let (sender, reply) = tokio::sync::oneshot::channel();
    window
        .run_on_main_thread(move || {
            let result = if request {
                #[link(name = "ApplicationServices", kind = "framework")]
                unsafe extern "C" {
                    static kAXTrustedCheckOptionPrompt: CFStringRef;
                    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> u8;
                }
                let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
                let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
                // Explicit button only. This requests the system prompt, never
                // edits TCC, grants permission itself, or starts an observation.
                unsafe { AXIsProcessTrustedWithOptions(options.as_CFTypeRef()) };
                Ok(())
            } else {
                use objc2_foundation::{NSString, NSURL};
                NSURL::URLWithString(&NSString::from_str(
                    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
                ))
                .filter(|url| objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(url))
                .map(|_| ())
                .ok_or_else(|| {
                    AppError::new(
                        "SystemSettingsUnavailable",
                        "请手动打开系统设置 → 隐私与安全性 → 辅助功能。",
                        true,
                    )
                })
            };
            let _ = sender.send(result);
        })
        .map_err(|_| AppError::new("AuditUnavailable", "原生验证入口不可用。", true))?;
    reply
        .await
        .map_err(|_| AppError::new("AuditUnavailable", "原生验证入口已关闭。", true))?
}

pub(crate) fn run(context: tauri::Context<tauri::Wry>) {
    assert_eq!(context.config().identifier, IDENTIFIER);
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            let input = NativeInput::default();
            workspace_lifecycle::setup(input.clone());
            let service = target::Service::native(input.clone())
                .ok_or("Unable to create native target observer")?;
            app.manage(Arc::new(Audit {
                data: Mutex::new(Data {
                    revision: 0,
                    id: Uuid::nil(),
                    phase: Phase::Idle,
                    until: Instant::now(),
                    reason: None,
                    operation: None,
                    native_error: None,
                    cancel: CancellationToken::new(),
                    lease: None,
                }),
                service: Arc::new(service),
                input,
            }));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            audit_status,
            audit_begin,
            audit_cancel,
            audit_permission_action
        ])
        .build(context)
        .expect("Unable to start standalone target audit")
        .run(|app, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. }
                    | tauri::RunEvent::Exit
                    | tauri::RunEvent::WindowEvent {
                        event: tauri::WindowEvent::CloseRequested { .. },
                        ..
                    }
            ) {
                app.state::<Arc<Audit>>().cancel();
                if matches!(event, tauri::RunEvent::Exit) {
                    workspace_lifecycle::destroy();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeDriver(Arc<AtomicUsize>);
    struct FakeWatch;
    impl target::Driver for FakeDriver {
        fn capture(
            &mut self,
            _: u64,
            _: Instant,
        ) -> std::result::Result<Box<dyn target::Watch>, ManualReason> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeWatch))
        }
    }
    impl target::Watch for FakeWatch {
        fn check(&mut self, _: Instant) -> std::result::Result<(), ManualReason> {
            Ok(())
        }
    }
    fn fixture() -> (Arc<Audit>, Arc<AtomicUsize>) {
        let captures = Arc::new(AtomicUsize::new(0));
        let driver_captures = captures.clone();
        let service = target::Service::for_test(move || Box::new(FakeDriver(driver_captures)));
        (
            Arc::new(Audit {
                data: Mutex::new(Data {
                    revision: 0,
                    id: Uuid::nil(),
                    phase: Phase::Idle,
                    until: Instant::now(),
                    reason: None,
                    operation: None,
                    native_error: None,
                    cancel: CancellationToken::new(),
                    lease: None,
                }),
                service: Arc::new(service),
                input: NativeInput::default(),
            }),
            captures,
        )
    }

    #[tokio::test]
    async fn denied_permission_and_cancelled_countdown_never_probe_or_revive() {
        let (audit, captures) = fixture();
        assert!(audit.begin_with_permission(false).is_err());
        assert!(audit.snapshot().phase == Phase::Idle);
        audit.begin_with_permission(true).unwrap();
        assert!(audit.begin_with_permission(true).is_err());
        let old_id = audit.data.lock().unwrap().id;
        audit.cancel();
        audit.publish(old_id, Phase::Passed, None, Duration::ZERO);
        assert!(audit.snapshot().phase == Phase::Cancelled);
        // The cancelled async task must actually release its owner; merely
        // not seeing a capture yet would not prove the delayed task stopped.
        tokio::time::timeout(Duration::from_secs(2), async {
            while Arc::strong_count(&audit) != 1 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(captures.load(Ordering::SeqCst), 0);
        audit.begin_with_permission(true).unwrap();
        audit.publish(
            old_id,
            Phase::Failed,
            Some(ManualReason::ProbeTimeout),
            Duration::ZERO,
        );
        assert!(audit.snapshot().phase == Phase::Countdown);
        audit.cancel();
    }

    #[test]
    fn cancel_ends_the_native_lease_and_late_completion_stays_cancelled() {
        let (audit, _) = fixture();
        let id = Uuid::new_v4();
        let lease = Arc::new(audit.service.begin(id, 0, Instant::now()));
        {
            let mut data = audit.data.lock().unwrap();
            data.id = id;
            data.phase = Phase::Observing;
            data.lease = Some(lease.clone());
        }
        audit.cancel();
        assert_eq!(
            lease.audit_outcome(),
            Some(Err(ManualReason::TargetUnavailable))
        );
        audit.publish(id, Phase::Passed, None, Duration::ZERO);
        let result = audit.snapshot();
        assert!(result.phase == Phase::Cancelled);
        assert!(result.reason.is_none());
    }
}
