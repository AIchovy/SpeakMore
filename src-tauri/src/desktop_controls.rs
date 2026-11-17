use crate::native_locale::Locale;
use crate::shortcut_manager::{self, Platform, Registration};
use crate::{session_runtime, Backend};
use serde::Serialize;
use speakmore_core::shortcuts::{Action, Binding, Preferences};
use speakmore_core::{
    error::{AppError, Result},
    session::{Phase, SessionSnapshot},
};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use uuid::Uuid;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    #[serde(flatten)]
    bindings: shortcut_manager::Status,
    hud_available: bool,
    service_error: Option<AppError>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordingStatus {
    revision: u64,
    session: SessionSnapshot,
    audio_level: f32,
    ui_locale: String,
    motion_policy: String,
    preview: bool,
    notice: Option<crate::recording_notice::Notice>,
}
struct Controls {
    shortcut_available: bool,
    presentation: Mutex<crate::recording_notice::Presentation>,
    shortcuts: shortcut_manager::Manager,
    routes: Arc<[AtomicU64; 2]>,
    hud_available: bool,
    preview: AtomicBool,
    queued: AtomicBool,
    hud_ready: AtomicBool,
    label: MenuItem<tauri::Wry>,
    open: MenuItem<tauri::Wry>,
    results: MenuItem<tauri::Wry>,
    start: MenuItem<tauri::Wry>,
    finish: MenuItem<tauri::Wry>,
    cancel: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
    meeting_pause: MenuItem<tauri::Wry>,
    meeting_stop: MenuItem<tauri::Wry>,
    locale: Mutex<Locale>,
    #[cfg(target_os = "macos")]
    application_menu: crate::native_menus::Labels,
    last_label: Mutex<String>,
}
impl Controls {
    fn localize(&self, locale: Locale) -> tauri::Result<()> {
        for (item, zh, en) in [
            (&self.open, "打开 SpeakMore", "Open SpeakMore"),
            (&self.results, "最近结果", "Recent Results"),
            (&self.start, "开始听写", "Start Dictation"),
            (&self.finish, "完成听写", "Finish Dictation"),
            (&self.cancel, "取消听写", "Cancel Dictation"),
            (&self.quit, "退出 SpeakMore", "Quit SpeakMore"),
            (&self.meeting_pause, MEETING_PAUSE.0, MEETING_PAUSE.1),
            (&self.meeting_stop, MEETING_STOP.0, MEETING_STOP.1),
        ] {
            item.set_text(locale.text(zh, en))?;
        }
        #[cfg(target_os = "macos")]
        self.application_menu.apply(locale)?;
        Ok(())
    }
}

const MEETING_PAUSE: (&str, &str) = ("暂停 / 继续会议录音", "Pause / Resume Meeting Recording");
const MEETING_STOP: (&str, &str) = ("结束会议并整理", "Finish Meeting and Summarize");
fn meeting_label(locale: Locale, paused: bool, ms: u64) -> String {
    format!(
        "{} {:02}:{:02}",
        if paused {
            locale.text("会议已暂停", "Meeting Paused")
        } else {
            locale.text("会议录音中", "Recording Meeting")
        },
        ms / 60000,
        ms / 1000 % 60
    )
}
fn status_label(locale: Locale, session: &SessionSnapshot, can_start: bool) -> String {
    if session.session_id.is_none() {
        return if can_start {
            locale.text("已就绪", "Ready")
        } else {
            locale.text("请打开 SpeakMore 完成准备", "Open SpeakMore to Get Ready")
        }
        .into();
    }
    if session.phase == Phase::Recording {
        let seconds = session.elapsed_ms.min(speakmore_core::MAX_RECORDING_MS) / 1000;
        return format!(
            "{} {:02}:{:02} / 05:00",
            locale.text("录音中", "Listening"),
            seconds / 60,
            seconds % 60
        );
    }
    match session.phase {
        Phase::Preparing => locale.text("正在准备", "Preparing"),
        Phase::FinalizingAudio => locale.text("正在处理录音", "Finalizing Audio"),
        Phase::Transcribing => locale.text("正在转写", "Transcribing"),
        Phase::Polishing => locale.text("正在整理文字", "Refining Text"),
        Phase::DeliveryCheck => locale.text("正在检查输入目标", "Checking Input Target"),
        Phase::Submitting => locale.text("正在输入", "Inserting Text"),
        Phase::Cancelling => locale.text("正在取消", "Cancelling"),
        Phase::Finishing => locale.text("正在完成", "Finishing"),
        Phase::Idle | Phase::Recording => locale.text("正在处理", "Processing"),
    }
    .into()
}

fn controls(app: &tauri::AppHandle) -> Result<tauri::State<'_, Controls>> {
    app.try_state::<Controls>().ok_or_else(|| {
        AppError::new(
            "ControlsUnavailable",
            "原生录音控制不可用，请重新启动。",
            true,
        )
    })
}
fn runtime(app: &tauri::AppHandle) -> Result<session_runtime::Handle> {
    app.state::<Backend>().runtime.clone()
}

pub(crate) fn show_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}
pub(crate) fn setup(app: &tauri::AppHandle, preferences: Result<Preferences>) -> tauri::Result<()> {
    // Plugin initialization failure must leave the main application usable too,
    // rather than failing the application's Builder before showing its window.
    let shortcut_available = app
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .is_ok();
    let locale = runtime(app)
        .and_then(|r| r.snapshot())
        .map(|s| Locale::from_preference(&s.settings.ui_locale))
        .unwrap_or_else(|_| crate::native_locale::system_locale());
    #[cfg(target_os = "macos")]
    let application_menu = crate::native_menus::install(app, locale)?;
    let label = MenuItem::with_id(app, "recording-status", "SpeakMore", false, None::<&str>)?;
    let open = MenuItem::with_id(
        app,
        "show-main",
        locale.text("打开 SpeakMore", "Open SpeakMore"),
        true,
        None::<&str>,
    )?;
    let results = MenuItem::with_id(
        app,
        "show-results",
        locale.text("最近结果", "Recent Results"),
        true,
        None::<&str>,
    )?;
    let start = MenuItem::with_id(
        app,
        "start-recording",
        locale.text("开始听写", "Start Dictation"),
        false,
        None::<&str>,
    )?;
    let finish = MenuItem::with_id(
        app,
        "finish-recording",
        locale.text("完成听写", "Finish Dictation"),
        false,
        None::<&str>,
    )?;
    let cancel = MenuItem::with_id(
        app,
        "cancel-recording",
        locale.text("取消听写", "Cancel Dictation"),
        false,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        "quit-app",
        locale.text("退出 SpeakMore", "Quit SpeakMore"),
        true,
        None::<&str>,
    )?;
    let meeting_pause = MenuItem::with_id(
        app,
        "meeting-pause",
        locale.text(MEETING_PAUSE.0, MEETING_PAUSE.1),
        false,
        None::<&str>,
    )?;
    let meeting_stop = MenuItem::with_id(
        app,
        "meeting-stop",
        locale.text(MEETING_STOP.0, MEETING_STOP.1),
        false,
        None::<&str>,
    )?;
    let menu = Menu::with_items(
        app,
        &[
            &label,
            &open,
            &results,
            &PredefinedMenuItem::separator(app)?,
            &start,
            &finish,
            &cancel,
            &PredefinedMenuItem::separator(app)?,
            &meeting_pause,
            &meeting_stop,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    // The transparent brand mark adapts to the macOS menu bar appearance.
    TrayIconBuilder::with_id("speakmore")
        .icon(tauri::include_image!("icons/tray.png"))
        .icon_as_template(true)
        .tooltip("SpeakMore")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show-main" => show_main(app),
            "show-results" => {
                show_main(app);
                let _ = app.emit("show_recording_results", ());
            }
            action @ ("meeting-pause" | "meeting-stop") => {
                if let Ok(service) = &app.state::<crate::meetings::Managed>().0 {
                    service.menu_recording(action == "meeting-stop");
                }
            }
            "quit-app" => app.exit(0),
            "start-recording" => {
                if let Ok(runtime) = runtime(app) {
                    let app = app.clone();
                    let start = runtime.start_from_menu();
                    tauri::async_runtime::spawn(async move {
                        if start.await.is_err() {
                            show_main(&app);
                        }
                    });
                }
            }
            action @ ("finish-recording" | "cancel-recording") => {
                if let Ok(runtime) = runtime(app) {
                    if let Some(id) = runtime.snapshot().ok().and_then(|s| s.session.session_id) {
                        let _ = runtime.native_stop(id, action == "cancel-recording");
                    }
                }
            }
            _ => {}
        })
        .build(app)?;
    #[cfg(target_os = "macos")]
    let hud = crate::recording_panel::create(app).is_ok();
    #[cfg(not(target_os = "macos"))]
    let hud = false;
    app.manage(Controls {
        shortcut_available,
        presentation: Mutex::new(crate::recording_notice::Presentation::default()),
        shortcuts: shortcut_manager::Manager::new(preferences),
        routes: Arc::new([AtomicU64::new(0), AtomicU64::new(0)]),
        hud_available: hud,
        preview: AtomicBool::new(false),
        queued: AtomicBool::new(false),
        hud_ready: AtomicBool::new(false),
        label,
        open,
        results,
        start,
        finish,
        cancel,
        quit,
        meeting_pause,
        meeting_stop,
        locale: Mutex::new(locale),
        #[cfg(target_os = "macos")]
        application_menu,
        last_label: Mutex::new(String::new()),
    });
    if let Ok(runtime) = runtime(app) {
        runtime.native_input().set_hold_enabled(false);
    }
    // Registration must not wait for JavaScript in an invisible WebView.
    let _ = register(app);
    refresh(app);
    // Session publishes no longer tick while idle: keep the shortcut-capture
    // validity check and the meeting tray clock moving on their own.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            interval.tick().await;
            let editing = runtime(&app).is_ok_and(|r| r.native_input().editing());
            let meeting = app
                .try_state::<crate::meetings::Managed>()
                .is_some_and(|m| m.0.as_ref().is_ok_and(|s| s.recording_status().is_some()));
            if editing || meeting {
                refresh(&app);
            }
        }
    });
    Ok(())
}
pub(crate) fn status(app: &tauri::AppHandle) -> Result<Status> {
    let state = controls(app)?;
    Ok(Status {
        bindings: state.shortcuts.status(),
        hud_available: state.hud_available,
        service_error: if !state.hud_available {
            Some(AppError::new(
                "HudUnavailable",
                "悬浮录音条不可用，可在主窗口听写。",
                true,
            ))
        } else if !state.shortcut_available {
            Some(AppError::new(
                "ControlsUnavailable",
                "系统快捷键服务不可用，请重新启动应用后重试。",
                true,
            ))
        } else {
            None
        },
    })
}
pub(crate) fn register(app: &tauri::AppHandle) -> Result<Status> {
    let state = controls(app)?;
    if state.shortcut_available {
        let platform = Registrar {
            app: app.clone(),
            runtime: runtime(app)?,
            routes: state.routes.clone(),
        };
        state.shortcuts.register_saved(&platform)?;
    }
    let _ = app.emit("controls_changed", ());
    status(app)
}
struct Registrar {
    app: tauri::AppHandle,
    runtime: session_runtime::Handle,
    routes: Arc<[AtomicU64; 2]>,
}
impl Platform for Registrar {
    fn register(&self, registration: &Registration) -> Result<()> {
        if registration.accelerator == "Fn" {
            #[cfg(target_os = "macos")]
            {
                let action = registration.action;
                let generation = registration.generation;
                let routes = self.routes.clone();
                let runtime = self.runtime.clone();
                let app = self.app.clone();
                let active_routes = routes.clone();
                return crate::fn_shortcut::register(
                    &self.app,
                    generation,
                    runtime.native_input(),
                    move || {
                        active_routes[shortcut_manager::index(action)].load(Ordering::Acquire)
                            == generation
                    },
                    move || {
                        if routes[shortcut_manager::index(action)].load(Ordering::Acquire)
                            != generation
                        {
                            return;
                        }
                        let input = runtime.native_input();
                        let snapshot = runtime
                            .read(|s| (s.session.session_id, s.can_start, s.revision))
                            .ok();
                        match action {
                            Action::PushToTalk => {
                                if !input.editing()
                                    && snapshot.is_some_and(|(id, can_start, _)| {
                                        id.is_none() && !can_start
                                    })
                                {
                                    show_main(&app);
                                }
                                input.native_toggle();
                            }
                            Action::Cancel => {
                                input.cancel_key(
                                    true,
                                    snapshot.and_then(|(id, _, _)| id),
                                    snapshot.map_or(0, |(_, _, revision)| revision),
                                );
                                input.cancel_key(false, None, 0);
                            }
                        }
                    },
                );
            }
            #[cfg(not(target_os = "macos"))]
            return Err(AppError::invalid("Fn 单键仅支持 macOS。"));
        }
        let shortcut: Shortcut = registration
            .accelerator
            .parse()
            .map_err(|_| AppError::invalid("此按键组合不受支持。"))?;
        let action = registration.action;
        let generation = registration.generation;
        let routes = self.routes.clone();
        let runtime = self.runtime.clone();
        let input = runtime.native_input();
        self.app
            .global_shortcut()
            .on_shortcut(shortcut, move |app, _, event| {
                if routes[shortcut_manager::index(action)].load(Ordering::Acquire) != generation {
                    return;
                }
                let down = event.state == ShortcutState::Pressed;
                match action {
                    Action::PushToTalk => {
                        if down
                            && !input.editing()
                            && runtime
                                .read(|s| s.session.session_id.is_none() && !s.can_start)
                                .unwrap_or(false)
                        {
                            show_main(app);
                        }
                        input.native_key(down);
                    }
                    Action::Cancel => {
                        // Only a fresh press reads the session; a release never cancels.
                        let snapshot = down
                            .then(|| runtime.read(|s| (s.session.session_id, s.revision)).ok())
                            .flatten();
                        input.cancel_key(
                            down,
                            snapshot.and_then(|(id, _)| id),
                            snapshot.map_or(0, |(_, revision)| revision),
                        );
                    }
                }
            })
            .map_err(|_| {
                AppError::new(
                    "ShortcutConflict",
                    "该组合无法注册，可能已被占用。请选择其他组合。",
                    true,
                )
            })
    }
    fn unregister(&self, registration: &Registration) -> Result<()> {
        if registration.accelerator == "Fn" {
            #[cfg(target_os = "macos")]
            return crate::fn_shortcut::unregister(registration.generation);
            #[cfg(not(target_os = "macos"))]
            return Ok(());
        }
        let shortcut: Shortcut = registration
            .accelerator
            .parse()
            .map_err(|_| AppError::invalid("此按键组合不受支持。"))?;
        self.app
            .global_shortcut()
            .unregister(shortcut)
            .map_err(|_| AppError::new("ShortcutCleanupFailed", "旧快捷键未能释放，请重试。", true))
    }
    fn activate(&self, action: Action, registration: Option<&Registration>) {
        self.routes[shortcut_manager::index(action)]
            .store(registration.map_or(0, |r| r.generation), Ordering::Release);
        if action == Action::PushToTalk {
            self.runtime
                .native_input()
                .set_hold_enabled(registration.is_some());
        }
    }
}
struct EditingCommit {
    input: session_runtime::NativeInput,
    id: Uuid,
    success: bool,
}
impl Drop for EditingCommit {
    fn drop(&mut self) {
        if self.success {
            self.input.end_edit(self.id, true);
        } else {
            self.input.release_commit(self.id);
        }
    }
}
pub(crate) async fn begin_edit(app: &tauri::AppHandle) -> Result<Uuid> {
    let state = controls(app)?;
    if !state.shortcut_available {
        return Err(AppError::new(
            "ControlsUnavailable",
            "快捷键服务尚未就绪。",
            true,
        ));
    }
    runtime(app)?.begin_shortcut_edit().await
}
pub(crate) fn capture_shortcut(
    app: &tauri::AppHandle,
    id: Uuid,
    capture_id: Uuid,
    active: bool,
) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if active {
            crate::shortcut_capture::start(app, runtime(app)?.native_input(), id, capture_id)
        } else {
            crate::shortcut_capture::stop(app, id, Some(capture_id))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, id, capture_id, active);
        Err(AppError::invalid("原生按键录入目前仅支持 macOS。"))
    }
}
pub(crate) fn edit_heartbeat(app: &tauri::AppHandle, id: Uuid, close: bool) -> Result<()> {
    let input = runtime(app)?.native_input();
    if close {
        #[cfg(target_os = "macos")]
        crate::shortcut_capture::stop(app, id, None)?;
        input.end_edit(id, false);
        Ok(())
    } else {
        input.renew_edit(id)
    }
}
pub(crate) async fn save_shortcut(
    app: &tauri::AppHandle,
    id: Uuid,
    action: Action,
    binding: Binding,
    expected: Preferences,
) -> Result<Status> {
    if action != Action::PushToTalk {
        return Err(AppError::invalid(
            "只需设置一个听写快捷键；取消请使用悬浮条。",
        ));
    }
    let state = controls(app)?;
    let runtime = runtime(app)?;
    let input = runtime.native_input();
    expected.replace(action, binding.clone())?;
    #[cfg(target_os = "macos")]
    crate::shortcut_capture::stop(app, id, None)?;
    input.commit_edit(id)?;
    let mut guard = EditingCommit {
        input,
        id,
        success: false,
    };
    let platform = Registrar {
        app: app.clone(),
        runtime: runtime.clone(),
        routes: state.routes.clone(),
    };
    let result = state
        .shortcuts
        .replace(
            &platform,
            action,
            binding,
            expected,
            move |expected, next| async move {
                runtime
                    .database(move |store| store.update_shortcuts(&expected, &next, crate::now()))
                    .await
            },
        )
        .await;
    guard.success = result.is_ok();
    let _ = app.emit("controls_changed", ());
    result?;
    status(app)
}
pub(crate) fn hud_ready(app: &tauri::AppHandle) -> Result<()> {
    controls(app)?.hud_ready.store(true, Ordering::Release);
    register(app)?;
    refresh(app);
    Ok(())
}
pub(crate) fn recording_status(app: &tauri::AppHandle) -> Result<RecordingStatus> {
    let state = controls(app)?;
    let (revision, session, audio_level, ui_locale, motion_policy, deliveries) = runtime(app)?
        .read(|s| {
            (
                s.revision,
                s.session.clone(),
                s.audio_level,
                s.settings.ui_locale.clone(),
                s.settings.motion_policy.clone(),
                s.recent_results
                    .iter()
                    .map(|r| (r.session_id, r.delivery_status.clone(), r.delivery_reason))
                    .collect::<Vec<_>>(),
            )
        })?;
    let notice = if session.session_id.is_none() {
        state.presentation.lock().unwrap().get().map(|mut notice| {
            if let Some((_, status, reason)) = deliveries
                .into_iter()
                .find(|(id, _, _)| *id == notice.session_id)
            {
                notice.delivery_status = Some(status);
                notice.delivery_reason = reason;
            }
            notice
        })
    } else {
        None
    };
    Ok(RecordingStatus {
        revision,
        session,
        audio_level,
        ui_locale,
        motion_policy,
        preview: state.preview.load(Ordering::Acquire),
        notice,
    })
}
pub(crate) fn preview(app: &tauri::AppHandle, show: bool) -> Result<()> {
    if !status(app)?.hud_available {
        return Err(AppError::new("HudUnavailable", "悬浮录音条不可用。", true));
    }
    let state = controls(app)?;
    if show {
        let snapshot = runtime(app)?.snapshot()?;
        if snapshot.capture_suspended || snapshot.session.session_id.is_some() {
            return Err(AppError::busy());
        }
        *state.presentation.lock().unwrap() = crate::recording_notice::Presentation::default();
    }
    state.preview.store(show, Ordering::Release);
    refresh(app);
    let _ = app.emit_to("hud", "recording_changed", ());
    Ok(())
}
pub(crate) fn notice_action(app: &tauri::AppHandle, id: Uuid, open: bool) -> Result<()> {
    let state = controls(app)?;
    let mut presentation = state.presentation.lock().unwrap();
    if presentation.get().is_none_or(|n| n.session_id != id)
        || runtime(app)?.snapshot()?.session.session_id.is_some()
    {
        return Ok(());
    }
    presentation.dismiss(id);
    drop(presentation);
    if open {
        let _ = app.emit_to("main", "show_recording_results", ());
        show_main(app);
    }
    refresh(app);
    Ok(())
}
/// The few snapshot fields the tray and HUD refresh read on the main thread.
struct Glance {
    revision: u64,
    session: SessionSnapshot,
    results: Vec<Uuid>,
    last_error: Option<AppError>,
    capture_suspended: bool,
    can_start: bool,
    ui_locale: String,
}
pub(crate) fn refresh(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<Controls>() else {
        return;
    };
    if state.queued.swap(true, Ordering::AcqRel) {
        return;
    }
    let handle = app.clone();
    if app
        .run_on_main_thread(move || {
            #[cfg(target_os = "macos")]
            crate::shortcut_capture::refresh();
            let state = handle.state::<Controls>();
            state.queued.store(false, Ordering::Release);
            let Ok(snapshot) = runtime(&handle).and_then(|r| {
                r.read(|s| Glance {
                    revision: s.revision,
                    session: s.session.clone(),
                    results: s.recent_results.iter().map(|r| r.session_id).collect(),
                    last_error: s.last_error.clone(),
                    capture_suspended: s.capture_suspended,
                    can_start: s.can_start,
                    ui_locale: s.settings.ui_locale.clone(),
                })
            }) else {
                return;
            };
            #[cfg(target_os = "macos")]
            crate::recording_sounds::sync(snapshot.session.session_id, snapshot.session.phase);
            let active = snapshot.session.session_id.is_some();
            let new_notice = state.presentation.lock().unwrap().sync(
                snapshot.session.session_id,
                &snapshot.results,
                snapshot.last_error.as_ref(),
                std::time::Instant::now(),
            );
            if new_notice {
                let app = handle.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(8)).await;
                    // Refresh evaluates the current receipt/active session; an old
                    // timer never directly hides a newer recording.
                    refresh(&app);
                });
            }
            if active || snapshot.capture_suspended {
                state.preview.store(false, Ordering::Release);
            }
            let recording = snapshot.session.phase == Phase::Recording;
            let finalizing = matches!(snapshot.session.phase, Phase::Cancelling | Phase::Finishing);
            let locale = Locale::from_preference(&snapshot.ui_locale);
            let mut previous_locale = state.locale.lock().unwrap();
            if *previous_locale != locale && state.localize(locale).is_ok() {
                *previous_locale = locale;
            }
            drop(previous_locale);
            let meeting = handle
                .try_state::<crate::meetings::Managed>()
                .and_then(|m| m.0.as_ref().ok().and_then(|s| s.recording_status()));
            let _ = state.meeting_pause.set_enabled(meeting.is_some());
            let _ = state.meeting_stop.set_enabled(meeting.is_some());
            crate::meeting_panel::sync(meeting.map(|(id, _, _)| id));
            let label = if let Some((_, paused, ms)) = meeting {
                meeting_label(locale, paused, ms)
            } else {
                status_label(locale, &snapshot.session, snapshot.can_start)
            };
            let mut previous_label = state.last_label.lock().unwrap();
            if *previous_label != label && state.label.set_text(&label).is_ok() {
                *previous_label = label;
            }
            drop(previous_label);
            let _ = state.start.set_enabled(snapshot.can_start);
            let _ = state.finish.set_enabled(recording);
            let _ = state.cancel.set_enabled(active && !finalizing);
            #[cfg(target_os = "macos")]
            crate::recording_panel::visible(
                !snapshot.capture_suspended
                    && (active
                        || state.preview.load(Ordering::Acquire)
                        || state.presentation.lock().unwrap().get().is_some()),
            );
            let _ = handle.emit_to("hud", "recording_changed", snapshot.revision);
        })
        .is_err()
    {
        state.queued.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_status_follows_language_phase_readiness_and_five_minute_limit() {
        let mut session = speakmore_core::session::SessionController::default().snapshot(0);
        assert_eq!(status_label(Locale::English, &session, true), "Ready");
        assert_eq!(status_label(Locale::Chinese, &session, true), "已就绪");
        assert_eq!(
            status_label(Locale::English, &session, false),
            "Open SpeakMore to Get Ready"
        );
        assert_eq!(
            status_label(Locale::Chinese, &session, false),
            "请打开 SpeakMore 完成准备"
        );
        session.session_id = Some(Uuid::new_v4());
        for phase in [
            Phase::Preparing,
            Phase::Recording,
            Phase::FinalizingAudio,
            Phase::Transcribing,
            Phase::Polishing,
            Phase::DeliveryCheck,
            Phase::Submitting,
            Phase::Cancelling,
            Phase::Finishing,
        ] {
            session.phase = phase;
            let en = status_label(Locale::English, &session, false);
            let zh = status_label(Locale::Chinese, &session, false);
            assert!(en.is_ascii() && !en.is_empty());
            assert!(!zh.is_ascii());
            assert_ne!(en, "Ready");
            assert_eq!(en, status_label(Locale::English, &session, true));
        }
        session.phase = Phase::Recording;
        for (elapsed, expected) in [
            (0, "00:00"),
            (61_999, "01:01"),
            (300_000, "05:00"),
            (u64::MAX, "05:00"),
        ] {
            session.elapsed_ms = elapsed;
            assert_eq!(
                status_label(Locale::English, &session, false),
                format!("Listening {expected} / 05:00")
            );
        }
    }
    #[test]
    fn meeting_tray_labels_follow_language() {
        assert_eq!(
            meeting_label(Locale::English, false, 61_999),
            "Recording Meeting 01:01"
        );
        assert_eq!(
            meeting_label(Locale::English, true, 3_600_000),
            "Meeting Paused 60:00"
        );
        assert_eq!(meeting_label(Locale::Chinese, false, 0), "会议录音中 00:00");
        assert_eq!(meeting_label(Locale::Chinese, true, 0), "会议已暂停 00:00");
        for (zh, en) in [MEETING_PAUSE, MEETING_STOP] {
            assert!(en.is_ascii() && !zh.is_ascii());
        }
    }
}
