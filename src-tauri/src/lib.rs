mod applications;
#[cfg(target_os = "macos")]
mod caption_panel;
mod clipboard;
mod delivery;
mod desktop_controls;
#[cfg(target_os = "macos")]
mod fn_shortcut;
mod history_export;
mod meeting_panel;
mod meetings;
#[cfg(feature = "model-check")]
mod model_check;
mod model_import;
mod model_registry;
mod native_locale;
#[cfg(target_os = "macos")]
mod native_menus;
mod overview;
mod permissions;
mod providers;
mod recording_notice;
#[cfg(target_os = "macos")]
mod recording_panel;
#[cfg(target_os = "macos")]
mod recording_sounds;
mod session_runtime;
#[cfg(target_os = "macos")]
mod shortcut_capture;
mod shortcut_manager;
mod storage_startup;
mod target;
#[cfg(all(target_os = "macos", feature = "target-audit"))]
mod target_audit;
#[cfg(target_os = "macos")]
mod workspace_lifecycle;
use speakmore_core::{
    error::{AppError, Result},
    session::StartMode,
    storage::Store,
    types::*,
};
use std::{
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{Emitter, Manager, State, WebviewWindow};
use uuid::Uuid;

struct Backend {
    storage: storage_startup::Recovery,
    runtime: Result<session_runtime::Handle>,
    models: Result<speakmore_models::ModelManager>,
    providers: Result<providers::Service>,
    exit_deadline: OnceLock<Instant>,
}
/// Close admission once. Every quit path (tray, Cmd+Q, Dock, logout, restart)
/// shares one bounded drain budget; the flag reports whether this call began it.
fn begin_exit(app: &tauri::AppHandle) -> Option<(session_runtime::Handle, Instant, bool)> {
    let state = app.try_state::<Backend>()?;
    let runtime = state.runtime.as_ref().ok()?.clone();
    let mut first = false;
    let deadline = *state.exit_deadline.get_or_init(|| {
        first = true;
        Instant::now() + Duration::from_secs(3)
    });
    if first {
        if let Some(Ok(meetings)) = app.try_state::<meetings::Managed>().map(|m| &m.inner().0) {
            meetings.shutdown();
        }
        runtime.shutdown();
    }
    Some((runtime, deadline, first))
}
fn exit_drained(app: &tauri::AppHandle, runtime: &session_runtime::Handle) -> bool {
    runtime
        .read(|s| {
            s.worker.phase == speakmore_worker_supervisor::Phase::Stopped
                && s.session.session_id.is_none()
        })
        // No published view means no worker was ever started.
        .unwrap_or(true)
        && !app
            .try_state::<meetings::Managed>()
            .is_some_and(|m| m.0.as_ref().is_ok_and(|m| m.busy()))
}
async fn drain_exit(app: &tauri::AppHandle, runtime: &session_runtime::Handle, deadline: Instant) {
    while Instant::now() < deadline && !exit_drained(app, runtime) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
#[tauri::command]
fn get_storage_status(
    window: WebviewWindow,
    state: State<'_, Backend>,
) -> Result<storage_startup::Status> {
    main_only(&window)?;
    Ok(state.storage.status.clone())
}
#[tauri::command]
async fn reveal_recovery_data(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, Backend>,
) -> Result<()> {
    main_only(&window)?;
    state.storage.reveal(&app).await
}
#[tauri::command]
fn restart_after_storage_error(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, Backend>,
) -> Result<()> {
    main_only(&window)?;
    if state.storage.status.error.is_none() || state.runtime.is_ok() {
        return Err(AppError::new(
            "RecoveryUnavailable",
            "当前没有需要重启恢复的存储问题。",
            false,
        ));
    }
    app.restart()
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
fn main_only(window: &WebviewWindow) -> Result<()> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err(AppError::new(
            "Forbidden",
            "当前窗口没有此操作权限。",
            false,
        ))
    }
}
async fn with_store<T: Send + 'static>(
    state: &Backend,
    task: impl FnOnce(&mut Store) -> Result<T> + Send + 'static,
) -> Result<T> {
    state
        .runtime
        .as_ref()
        .map_err(Clone::clone)?
        .database(task)
        .await
}
/// Read-only store access: never re-reads the session view or flashes readiness.
async fn query_store<T: Send + 'static>(
    state: &Backend,
    task: impl FnOnce(&Store) -> Result<T> + Send + 'static,
) -> Result<T> {
    state
        .runtime
        .as_ref()
        .map_err(Clone::clone)?
        .query(task)
        .await
}
#[tauri::command]
async fn get_snapshot(
    window: WebviewWindow,
    state: State<'_, Backend>,
) -> Result<session_runtime::Snapshot> {
    main_only(&window)?;
    state.runtime.as_ref().map_err(Clone::clone)?.snapshot()
}
#[tauri::command]
async fn get_providers(
    window: WebviewWindow,
    state: State<'_, Backend>,
) -> Result<providers::View> {
    main_only(&window)?;
    state.providers.as_ref().map_err(Clone::clone)?.view().await
}
#[tauri::command]
async fn save_provider(
    window: WebviewWindow,
    state: State<'_, Backend>,
    input: providers::SaveInput,
) -> Result<()> {
    main_only(&window)?;
    state
        .providers
        .as_ref()
        .map_err(Clone::clone)?
        .save(input)
        .await
}
#[tauri::command]
async fn test_provider(
    window: WebviewWindow,
    state: State<'_, Backend>,
    provider_id: Uuid,
    revision: i64,
) -> Result<()> {
    main_only(&window)?;
    state
        .providers
        .as_ref()
        .map_err(Clone::clone)?
        .test(provider_id, revision)
        .await
}
#[tauri::command]
fn cancel_provider_test(window: WebviewWindow, state: State<'_, Backend>) -> Result<()> {
    main_only(&window)?;
    state
        .providers
        .as_ref()
        .map_err(Clone::clone)?
        .cancel_test();
    Ok(())
}
#[tauri::command]
async fn select_provider(
    window: WebviewWindow,
    state: State<'_, Backend>,
    expected_revision: i64,
    provider_id: Option<Uuid>,
) -> Result<()> {
    main_only(&window)?;
    state
        .providers
        .as_ref()
        .map_err(Clone::clone)?
        .select(expected_revision, provider_id)
        .await
}
#[tauri::command]
async fn delete_provider(
    window: WebviewWindow,
    state: State<'_, Backend>,
    expected_revision: i64,
    provider_id: Uuid,
) -> Result<()> {
    main_only(&window)?;
    state
        .providers
        .as_ref()
        .map_err(Clone::clone)?
        .delete(expected_revision, provider_id)
        .await
}
#[tauri::command]
async fn retry_provider_cleanup(window: WebviewWindow, state: State<'_, Backend>) -> Result<()> {
    main_only(&window)?;
    state
        .providers
        .as_ref()
        .map_err(Clone::clone)?
        .retry_cleanup()
        .await
}
#[tauri::command]
async fn discard_invalid_provider(
    window: WebviewWindow,
    state: State<'_, Backend>,
    expected_revision: i64,
    recovery_id: Uuid,
) -> Result<()> {
    main_only(&window)?;
    state
        .providers
        .as_ref()
        .map_err(Clone::clone)?
        .discard_invalid(expected_revision, recovery_id)
        .await
}
#[tauri::command]
fn get_recording_controls(
    window: WebviewWindow,
    app: tauri::AppHandle,
) -> Result<desktop_controls::Status> {
    main_only(&window)?;
    desktop_controls::status(&app)
}
#[tauri::command]
async fn begin_shortcut_edit(window: WebviewWindow, app: tauri::AppHandle) -> Result<Uuid> {
    main_only(&window)?;
    desktop_controls::begin_edit(&app).await
}
#[tauri::command]
fn shortcut_edit_heartbeat(
    window: WebviewWindow,
    app: tauri::AppHandle,
    edit_id: Uuid,
    close: bool,
) -> Result<()> {
    main_only(&window)?;
    desktop_controls::edit_heartbeat(&app, edit_id, close)
}
#[tauri::command]
async fn set_shortcut_capture(
    window: WebviewWindow,
    app: tauri::AppHandle,
    edit_id: Uuid,
    capture_id: Uuid,
    active: bool,
) -> Result<()> {
    main_only(&window)?;
    desktop_controls::capture_shortcut(&app, edit_id, capture_id, active)
}
#[tauri::command]
async fn save_recording_shortcut(
    window: WebviewWindow,
    app: tauri::AppHandle,
    edit_id: Uuid,
    action: speakmore_core::shortcuts::Action,
    binding: speakmore_core::shortcuts::Binding,
    expected: speakmore_core::shortcuts::Preferences,
) -> Result<desktop_controls::Status> {
    main_only(&window)?;
    desktop_controls::save_shortcut(&app, edit_id, action, binding, expected).await
}
#[tauri::command]
async fn retry_recording_shortcut(
    window: WebviewWindow,
    app: tauri::AppHandle,
) -> Result<desktop_controls::Status> {
    main_only(&window)?;
    desktop_controls::register(&app)
}
#[tauri::command]
fn get_recording_status(
    window: WebviewWindow,
    app: tauri::AppHandle,
) -> Result<desktop_controls::RecordingStatus> {
    recording_window(&window)?;
    desktop_controls::recording_status(&app)
}
#[tauri::command]
fn recording_hud_ready(window: WebviewWindow, app: tauri::AppHandle) -> Result<()> {
    if window.label() != "hud" {
        return Err(AppError::new(
            "Forbidden",
            "当前窗口没有此操作权限。",
            false,
        ));
    }
    desktop_controls::hud_ready(&app)
}
#[tauri::command]
fn recording_notice_action(
    window: WebviewWindow,
    app: tauri::AppHandle,
    session_id: Uuid,
    open: bool,
) -> Result<()> {
    recording_window(&window)?;
    desktop_controls::notice_action(&app, session_id, open)
}
fn recording_window(window: &WebviewWindow) -> Result<()> {
    if matches!(window.label(), "hud" | "main") {
        Ok(())
    } else {
        main_only(window)
    }
}
#[tauri::command]
fn recording_control(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, Backend>,
    session_id: Uuid,
    cancel: bool,
) -> Result<()> {
    recording_window(&window)?;
    state
        .runtime
        .as_ref()
        .map_err(Clone::clone)?
        .native_stop(session_id, cancel)?;
    desktop_controls::refresh(&app);
    Ok(())
}
#[tauri::command]
fn preview_recording_panel(window: WebviewWindow, app: tauri::AppHandle, show: bool) -> Result<()> {
    if show {
        main_only(&window)?;
    } else {
        recording_window(&window)?;
    }
    desktop_controls::preview(&app, show)
}
#[tauri::command]
async fn start_session(
    window: WebviewWindow,
    state: State<'_, Backend>,
    request_id: Uuid,
    mode: StartMode,
) -> Result<Uuid> {
    main_only(&window)?;
    // Hold lifecycles must come from native key-down/key-up events, not a WebView.
    if mode != StartMode::Toggle {
        return Err(AppError::invalid("主窗口使用点击开始和结束听写。"));
    }
    state
        .runtime
        .as_ref()
        .map_err(Clone::clone)?
        .start(request_id, mode)
        .await
}
#[tauri::command]
async fn stop_session(
    window: WebviewWindow,
    state: State<'_, Backend>,
    session_id: Uuid,
) -> Result<()> {
    main_only(&window)?;
    state
        .runtime
        .as_ref()
        .map_err(Clone::clone)?
        .stop(session_id, false)
        .await
}
#[tauri::command]
async fn cancel_session(
    window: WebviewWindow,
    state: State<'_, Backend>,
    session_id: Uuid,
) -> Result<()> {
    main_only(&window)?;
    state
        .runtime
        .as_ref()
        .map_err(Clone::clone)?
        .stop(session_id, true)
        .await
}
#[tauri::command]
async fn select_model(
    window: WebviewWindow,
    state: State<'_, Backend>,
    model_id: Option<String>,
    expected_revision: i64,
) -> Result<()> {
    main_only(&window)?;
    state
        .runtime
        .as_ref()
        .map_err(Clone::clone)?
        .select_model(model_id, expected_revision)
        .await
}
#[tauri::command]
async fn reload_model(window: WebviewWindow, state: State<'_, Backend>) -> Result<()> {
    main_only(&window)?;
    state
        .runtime
        .as_ref()
        .map_err(Clone::clone)?
        .reload_model()
        .await
}
#[tauri::command]
async fn clear_recent_results(window: WebviewWindow, state: State<'_, Backend>) -> Result<()> {
    main_only(&window)?;
    state
        .runtime
        .as_ref()
        .map_err(Clone::clone)?
        .clear_results()
        .await
}
#[tauri::command]
async fn enable_polishing(
    window: WebviewWindow,
    state: State<'_, Backend>,
    expected_revision: i64,
) -> Result<Settings> {
    main_only(&window)?;
    let settings = state
        .providers
        .as_ref()
        .map_err(Clone::clone)?
        .enable_polishing(expected_revision)
        .await?;
    let _ = window.emit("settings_changed", settings.revision);
    Ok(settings)
}
#[tauri::command]
async fn update_settings(
    window: WebviewWindow,
    state: State<'_, Backend>,
    expected_revision: i64,
    patch: SettingsPatch,
) -> Result<Settings> {
    main_only(&window)?;
    if patch.polish_enabled == Some(false)
        || patch.processing_policy.as_deref() == Some("local_only")
    {
        if let Ok(providers) = &state.providers {
            providers.cancel_test();
        }
    }
    if patch.polish_enabled == Some(true) {
        return Err(AppError::new(
            "ProviderNotReady",
            "请先完成 AI 服务与许可配置。",
            false,
        ));
    }
    let retention = patch.history_days.is_some() || patch.history_limit.is_some();
    let updated = with_store(&state, move |store| {
        store.update_settings(expected_revision, patch, now())
    })
    .await?;
    let _ = window.emit("settings_changed", updated.revision);
    if retention {
        let _ = window.emit("history_changed", ());
    }
    Ok(updated)
}

fn audio_error(error: speakmore_audio::error::AudioError) -> AppError {
    use speakmore_audio::error::AudioError;
    let message = match error {
        AudioError::NoInputDevice => "未找到麦克风，请连接后刷新。",
        AudioError::PermissionDenied => "请在系统隐私设置中允许 SpeakMore 使用麦克风。",
        AudioError::DeviceBusy | AudioError::Busy => "麦克风正在使用中，请稍后重试。",
        AudioError::UnsupportedAudioFormat => "此麦克风的音频格式暂不受支持，请选择其他设备。",
        AudioError::DeviceUnavailable => "录音设备已断开或不可用，请重新选择。",
        AudioError::TooShort => "录音太短，请说完一句话后再结束。",
        AudioError::PrepareTimeout => "麦克风启动超时，请检查权限与设备后重试。",
        AudioError::Incomplete | AudioError::DrainTimeout => {
            "音频采集未完成，请检查麦克风连接后重新录音。"
        }
        AudioError::MemoryUnavailable => "可用内存不足，请关闭一些应用后重试。",
        AudioError::Cancelled => "录音已取消。",
        _ => "无法读取录音设备，请刷新后重试。",
    };
    AppError::new(&error.to_string(), message, true)
}

#[tauri::command]
async fn list_input_devices(
    window: WebviewWindow,
) -> Result<Vec<speakmore_audio::device::InputDevice>> {
    main_only(&window)?;
    tauri::async_runtime::spawn_blocking(speakmore_audio::device::list_inputs)
        .await
        .map_err(|_| AppError::new("AudioThreadUnavailable", "无法读取录音设备，请重试。", true))?
        .map_err(audio_error)
}

#[tauri::command]
async fn microphone_permission_action(
    window: WebviewWindow,
    state: State<'_, Backend>,
    request: bool,
) -> Result<()> {
    main_only(&window)?;
    let snapshot = state.runtime.as_ref().map_err(Clone::clone)?.snapshot()?;
    if snapshot.session.session_id.is_some() || snapshot.capture_suspended {
        return Err(AppError::busy());
    }
    permissions::microphone_action(&window, request).await
}

#[tauri::command]
async fn open_accessibility_settings(window: WebviewWindow) -> Result<()> {
    main_only(&window)?;
    permissions::accessibility_settings(&window).await
}

#[tauri::command]
async fn check_accessibility_permission(window: WebviewWindow) -> Result<bool> {
    main_only(&window)?;
    permissions::check_accessibility(&window).await
}

#[tauri::command]
async fn restart_to_apply_permissions(
    window: WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, Backend>,
) -> Result<()> {
    main_only(&window)?;
    let runtime = state.runtime.as_ref().map_err(Clone::clone)?;
    // Reject active recording/model/configuration work, then close admission
    // and drain here: a restart ignores prevent_exit in ExitRequested.
    let _reservation = runtime.reserve_settings_operation().await?;
    if let Some((runtime, deadline, _)) = begin_exit(&app) {
        drain_exit(&app, &runtime, deadline).await;
    }
    app.request_restart();
    Ok(())
}

#[tauri::command]
async fn set_input_device(
    window: WebviewWindow,
    state: State<'_, Backend>,
    expected_revision: i64,
    device_id: Option<String>,
) -> Result<Settings> {
    main_only(&window)?;
    let updated = state
        .runtime
        .as_ref()
        .map_err(Clone::clone)?
        .set_device(device_id, expected_revision)
        .await?;
    let _ = window.emit("settings_changed", updated.revision);
    Ok(updated)
}

#[tauri::command]
async fn upsert_dictionary(
    window: WebviewWindow,
    state: State<'_, Backend>,
    expected_revision: i64,
    input: DictionaryInput,
) -> Result<DictionaryEntry> {
    main_only(&window)?;
    let entry = with_store(&state, move |store| {
        store.upsert_dictionary(expected_revision, input, now())
    })
    .await?;
    let _ = window.emit("settings_changed", ());
    Ok(entry)
}
#[tauri::command]
async fn delete_dictionary(
    window: WebviewWindow,
    state: State<'_, Backend>,
    expected_revision: i64,
    entry_id: Uuid,
) -> Result<()> {
    main_only(&window)?;
    with_store(&state, move |store| {
        store.delete_dictionary(entry_id, expected_revision, now())
    })
    .await?;
    let _ = window.emit("settings_changed", ());
    Ok(())
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct WritingRules {
    revision: i64,
    items: Vec<speakmore_core::writing::Rule>,
}
fn writing_rules(store: &speakmore_core::storage::Store) -> Result<WritingRules> {
    Ok(WritingRules {
        revision: store.settings()?.revision,
        items: store.writing_rules()?,
    })
}
#[tauri::command]
async fn list_writing_rules(
    window: WebviewWindow,
    state: State<'_, Backend>,
) -> Result<WritingRules> {
    main_only(&window)?;
    query_store(&state, writing_rules).await
}
#[tauri::command]
async fn upsert_writing_rule(
    window: WebviewWindow,
    state: State<'_, Backend>,
    expected_revision: i64,
    rule: speakmore_core::writing::Rule,
) -> Result<WritingRules> {
    main_only(&window)?;
    let next = with_store(&state, move |store| {
        store.upsert_writing_rule(expected_revision, rule, now())?;
        writing_rules(store)
    })
    .await?;
    let _ = window.emit("settings_changed", next.revision);
    Ok(next)
}
#[tauri::command]
async fn delete_writing_rule(
    window: WebviewWindow,
    state: State<'_, Backend>,
    expected_revision: i64,
    id: Uuid,
) -> Result<WritingRules> {
    main_only(&window)?;
    let next = with_store(&state, move |store| {
        store.delete_writing_rule(expected_revision, id, now())?;
        writing_rules(store)
    })
    .await?;
    let _ = window.emit("settings_changed", next.revision);
    Ok(next)
}

#[tauri::command]
async fn list_app_rules(
    window: WebviewWindow,
    state: State<'_, Backend>,
) -> Result<applications::Snapshot> {
    main_only(&window)?;
    query_store(&state, applications::snapshot).await
}
#[tauri::command]
async fn choose_app_for_rule(
    window: WebviewWindow,
    selections: State<'_, applications::Selections>,
) -> Result<Option<applications::Choice>> {
    main_only(&window)?;
    Ok(applications::choose(&window)
        .await?
        .map(|(id, name)| selections.put(id, name)))
}
#[tauri::command]
async fn upsert_app_rule(
    window: WebviewWindow,
    state: State<'_, Backend>,
    selections: State<'_, applications::Selections>,
    input: applications::Update,
) -> Result<applications::Snapshot> {
    main_only(&window)?;
    if input.selection_id.is_some() == input.app_identity.is_some() {
        return Err(AppError::invalid("请选择应用或已有规则。"));
    }
    let choice = input
        .selection_id
        .map(|id| selections.resolve(id))
        .transpose()?;
    let token = input.selection_id;
    let updated = with_store(&state, move |store| {
        let rule = if let Some(choice) = choice {
            applications::selected_rule(choice, input.manual_only, input.history_excluded)
        } else {
            let identity =
                speakmore_core::app_rules::macos_identity(input.app_identity.as_deref().unwrap())?;
            let mut existing = store
                .app_rules()?
                .into_iter()
                .find(|r| r.platform == "macos" && r.app_identity == identity)
                .ok_or_else(|| {
                    AppError::new("NotFound", "该应用规则已被移除，请刷新后重新选择。", true)
                })?;
            existing.manual_only = input.manual_only;
            existing.history_excluded = input.history_excluded;
            existing
        };
        store.upsert_app_rule(input.expected_revision, rule, now())?;
        applications::snapshot(store)
    })
    .await?;
    if let Some(token) = token {
        selections.consume(token);
    }
    let _ = window.emit("settings_changed", ());
    Ok(updated)
}
#[tauri::command]
async fn delete_app_rule(
    window: WebviewWindow,
    state: State<'_, Backend>,
    expected_revision: i64,
    app_identity: String,
) -> Result<applications::Snapshot> {
    main_only(&window)?;
    let updated = with_store(&state, move |store| {
        store.delete_app_rule(expected_revision, &app_identity, now())?;
        applications::snapshot(store)
    })
    .await?;
    let _ = window.emit("settings_changed", ());
    Ok(updated)
}
#[tauri::command]
async fn list_history(
    window: WebviewWindow,
    state: State<'_, Backend>,
    query: String,
    cursor: Option<String>,
) -> Result<HistoryPage> {
    main_only(&window)?;
    query_store(&state, move |store| {
        store.history(&query, cursor.as_deref())
    })
    .await
}
#[tauri::command]
async fn delete_history(
    window: WebviewWindow,
    state: State<'_, Backend>,
    ids: Vec<Uuid>,
) -> Result<usize> {
    main_only(&window)?;
    let count = with_store(&state, move |store| store.delete_history(&ids)).await?;
    let _ = window.emit("history_changed", ());
    Ok(count)
}
#[tauri::command]
async fn clear_history(window: WebviewWindow, state: State<'_, Backend>) -> Result<usize> {
    main_only(&window)?;
    let count = with_store(&state, Store::clear_history).await?;
    let _ = window.emit("history_changed", ());
    Ok(count)
}
#[tauri::command]
async fn export_history(
    window: WebviewWindow,
    state: State<'_, Backend>,
    scope: speakmore_core::history_export::Scope,
    format: speakmore_core::history_export::Format,
) -> Result<Option<usize>> {
    main_only(&window)?;
    scope.validate()?;
    let _guard = history_export::ExportGuard::acquire()?;
    let Some(path) = history_export::choose(&window, format).await? else {
        return Ok(None);
    };
    // The storage thread only renders; file writes and fsync happen elsewhere.
    let (bytes, count) = query_store(&state, move |store| {
        history_export::render(store, &scope, format)
    })
    .await?;
    history_export::save(path, bytes).await?;
    Ok(Some(count))
}
#[tauri::command]
async fn copy_result(
    window: WebviewWindow,
    state: State<'_, Backend>,
    result_id: Uuid,
    variant: String,
) -> Result<()> {
    main_only(&window)?;
    let text = state
        .runtime
        .as_ref()
        .map_err(Clone::clone)?
        .result_text(result_id, variant)
        .await?;
    state
        .runtime
        .as_ref()
        .map_err(Clone::clone)?
        .copy_text(text)
        .await
}

fn model_error(error: speakmore_models::ModelError) -> AppError {
    AppError::new(&format!("{error:?}"), &error.to_string(), error.retryable())
}
fn model_manager(state: &Backend) -> Result<speakmore_models::ModelManager> {
    state.models.clone()
}
#[tauri::command]
async fn list_models(
    window: WebviewWindow,
    state: State<'_, Backend>,
) -> Result<speakmore_models::ModelsSnapshot> {
    main_only(&window)?;
    Ok(model_manager(&state)?.snapshot())
}
#[tauri::command]
async fn download_model(
    window: WebviewWindow,
    state: State<'_, Backend>,
    model_id: String,
) -> Result<Uuid> {
    main_only(&window)?;
    state
        .runtime
        .as_ref()
        .map_err(Clone::clone)?
        .download_model(model_id)
        .await
}

#[tauri::command]
async fn import_model(
    window: WebviewWindow,
    state: State<'_, Backend>,
    model_id: String,
) -> Result<Option<Uuid>> {
    main_only(&window)?;
    let manager = model_manager(&state)?;
    let runtime = state.runtime.as_ref().map_err(Clone::clone)?;
    let english = runtime.snapshot()?.settings.ui_locale == "en";
    let Some(source) = model_import::choose(&window, manager, model_id.clone(), english).await?
    else {
        return Ok(None);
    };
    runtime.import_model(model_id, source).await.map(Some)
}

#[tauri::command]
async fn cancel_model_download(
    window: WebviewWindow,
    state: State<'_, Backend>,
    job_id: Uuid,
) -> Result<()> {
    main_only(&window)?;
    model_manager(&state)?
        .cancel_download(job_id)
        .map_err(model_error)
}
#[tauri::command]
async fn delete_model(
    window: WebviewWindow,
    state: State<'_, Backend>,
    model_id: String,
) -> Result<()> {
    main_only(&window)?;
    state
        .runtime
        .as_ref()
        .map_err(Clone::clone)?
        .delete_model(model_id)
        .await
}

fn worker_configuration(app: &tauri::App) -> Result<speakmore_worker_supervisor::Configuration> {
    use speakmore_worker_supervisor::{Backend, Configuration, Executable};
    let path = if cfg!(debug_assertions) {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/debug")
            .join(if cfg!(windows) {
                "speakmore-asr-worker.exe"
            } else {
                "speakmore-asr-worker"
            })
    } else {
        app.path()
            .resource_dir()
            .map_err(|_| AppError::new("WorkerUnavailable", "无法定位语音引擎。", false))?
            .join("workers")
            .join(if cfg!(windows) {
                "speakmore-asr-worker.exe"
            } else {
                "speakmore-asr-worker"
            })
    };
    let sensevoice = cfg!(all(target_os = "macos", target_arch = "aarch64")).then(|| Executable {
        path: path.with_file_name("speakmore-sensevoice-worker"),
        backend: Backend::Cpu,
    });
    Ok(Configuration {
        sensevoice,
        cpu: Executable {
            path: path.clone(),
            backend: Backend::Cpu,
        },
        accelerated: if cfg!(target_os = "macos") {
            Some(Executable {
                path,
                backend: Backend::Metal,
            })
        } else {
            None
        },
    })
}

pub fn run() {
    let context = tauri::generate_context!();
    #[cfg(feature = "model-check")]
    assert_eq!(
        context.config().identifier,
        model_check::IDENTIFIER,
        "model-check cannot run with a production or unrelated application identity"
    );
    #[cfg(feature = "model-audio-check")]
    {
        assert_eq!(option_env!("VITE_PUBLIC_AUDIO_CHECK"), Some("1"));
        assert!(
            context.config().app.windows.iter().any(|window| {
                window.label == "main"
                    && window.title == "SpeakMore · Public audio fixture · No microphone"
            }),
            "public audio checks require an explicit fixture window title"
        );
    }
    #[cfg(all(target_os = "macos", feature = "target-audit"))]
    if context.config().identifier == target_audit::IDENTIFIER {
        target_audit::run(context);
        return;
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            #[cfg(target_os = "macos")]
            if let Ok(cache) = app.path().app_cache_dir() {
                target::diagnostics::configure(cache);
            }
            let (store, storage) = storage_startup::open(app);
            #[cfg(feature = "model-check")]
            let store = store.and_then(|mut store| {
                model_check::prepare(&mut store)?;
                Ok(store)
            });
            let startup_result = store.as_ref().map(|_| ()).map_err(Clone::clone);
            let shortcuts = store
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|s| s.shortcuts());
            let store = Arc::new(Mutex::new(store));
            let models = (|| -> Result<speakmore_models::ModelManager> {
                startup_result.clone()?;
                use speakmore_models::catalog::{TrustKey, TrustedCatalog};
                let trust: TrustKey =
                    serde_json::from_slice(include_bytes!("../../resources/model-trust.json"))
                        .map_err(|_| model_error(speakmore_models::ModelError::UntrustedCatalog))?;
                let catalog = TrustedCatalog::verify(
                    include_bytes!("../../resources/model-catalog.signed.json"),
                    &trust,
                )
                .map_err(model_error)?;
                let baseline = TrustedCatalog::verify(
                    include_bytes!("../../resources/model-catalog.baseline.signed.json"),
                    &trust,
                )
                .map_err(model_error)?;
                catalog.preserves(&baseline).map_err(model_error)?;
                #[cfg(feature = "model-check")]
                let catalog = model_check::catalog(&catalog).map_err(model_error)?;
                let root = app
                    .path()
                    .app_local_data_dir()
                    .map_err(|_| model_error(speakmore_models::ModelError::Storage))?;
                let progress_app = app.handle().clone();
                let registry_app = app.handle().clone();
                let registry = Arc::new(model_registry::SqliteModels {
                    store: store.clone(),
                    changed: Box::new(move || {
                        let _ = registry_app.emit("readiness_changed", ());
                    }),
                });
                speakmore_models::ModelManager::open(
                    root.join("models"),
                    catalog,
                    registry,
                    Arc::new(move |revision| {
                        let _ = progress_app.emit("models_changed", revision);
                    }),
                )
                .map_err(model_error)
            })();
            if let Ok(manager) = &models {
                let manager = manager.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = manager.initialize().await;
                });
            }
            let session_app = app.handle().clone();
            let delivery_app = app.handle().clone();
            let runtime = startup_result
                .and_then(|_| worker_configuration(app))
                .and_then(|config| {
                    session_runtime::Handle::launch(
                        store,
                        models.clone(),
                        config,
                        Arc::new(move |revision| {
                            let _ = session_app.emit_to("main", "session_changed", revision);
                            desktop_controls::refresh(&session_app);
                        }),
                        Arc::new(move |update| {
                            let _ =
                                delivery_app.emit_to("main", "history_delivery_changed", update);
                        }),
                    )
                });
            #[cfg(target_os = "macos")]
            if let Ok(runtime) = &runtime {
                workspace_lifecycle::setup(runtime.native_input());
            }
            app.manage(applications::Selections::default());
            let provider_app = app.handle().clone();
            let providers = runtime.as_ref().map_err(Clone::clone).and_then(|runtime| {
                providers::Service::new(
                    runtime.clone(),
                    Arc::new(move || {
                        let _ = provider_app.emit_to("main", "providers_changed", ());
                    }),
                )
            });
            let meeting_service = runtime.as_ref().map_err(Clone::clone).and_then(|r| {
                meetings::Service::new(
                    app.handle().clone(),
                    r.clone(),
                    models.clone()?,
                    worker_configuration(app)?,
                )
            });
            app.manage(meetings::Managed(meeting_service));
            let _ = meetings::captions::create_window(app.handle());
            app.manage(Backend {
                storage,
                runtime,
                models,
                providers,
                exit_deadline: OnceLock::new(),
            });
            let _ = meeting_panel::create(app.handle());
            if let Ok(service) = &app.state::<meetings::Managed>().0 {
                service.resume_saved_queue();
            }
            // A menu/HUD failure must not prevent access to the main window.
            if desktop_controls::setup(app.handle(), shortcuts).is_err() {
                eprintln!("SpeakMore: native recording controls unavailable");
            }
            if let Ok(service) = &app.state::<meetings::Managed>().0 {
                meetings::experience::restore_shortcuts(service);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            meetings::audio::meeting_audio_apps,
            meetings::advanced::meeting_transcript_update,
            meetings::advanced::meeting_versions,
            meetings::advanced::meeting_restore,
            meetings::advanced::meeting_retention,
            meetings::advanced::meeting_model_choose,
            meetings::notion::meeting_notion_connection,
            meetings::notion::meeting_notion_save,
            meetings::notion::meeting_notion_disconnect,
            meetings::notion::meeting_notion_status,
            meetings::notion::meeting_notion_send,
            meetings::catalog::meeting_models,
            meetings::moss_runtime::meeting_moss_runtime_download,
            meetings::moss_runtime::meeting_moss_runtime_remove,
            meetings::catalog::meeting_model_download,
            meetings::catalog::meeting_model_remove,
            meetings::captions::captions_view,
            meetings::captions::captions_translation,
            meetings::captions::captions_recap,
            meetings::captions::captions_control,
            meetings::captions::captions_appearance,
            meetings::captions::captions_window,
            meetings::captions::captions_start,
            meetings::captions::captions_copy,
            meetings::captions::captions_paste,
            meetings::captions::captions_export,
            meetings::captions::captions_archives,
            meetings::captions::captions_archive_open,
            meetings::captions::captions_archive_delete,
            meetings::assistant::assistant_view,
            meetings::assistant::assistant_ask,
            meetings::assistant::assistant_cancel,
            meetings::assistant::assistant_clear,
            meetings::experience::meeting_note_update,
            meetings::experience::meeting_item_verify,
            meetings::experience::meeting_prepare,
            meetings::experience::experience_shortcuts,
            meetings::experience::experience_shortcuts_view,
            meetings::captions::captions_panel_size,
            meetings::captions::captions_reply_start,
            meetings::captions::captions_reply_control,
            meetings::captions::captions_reply_view,
            meetings::captions::captions_insight,
            meetings::experience::meeting_tasks_export,
            meetings::experience::meeting_capture_image,
            meetings::experience::meeting_note_image,
            meetings::experience::meeting_note_add,
            meetings::experience::meeting_note_remove,
            meetings::experience::meeting_context_save,
            meetings::experience::meeting_regenerate_affected,
            meetings::experience::meeting_insight,
            meetings::experience::meeting_research,
            meetings::meeting_activity,
            meetings::meeting_recording_view,
            meetings::meeting_recording_control,
            meetings::meeting_recording_note,
            meetings::meeting_recording_window,
            meetings::meeting_list,
            meetings::summary::meeting_summary_settings,
            meetings::summary::meeting_summary_choose,
            meetings::meeting_get,
            meetings::workspace::meeting_preferences,
            meetings::workspace::meeting_preferences_save,
            meetings::workspace::meeting_template_choose,
            meetings::workspace::meeting_speaker_rename,
            meetings::workspace::meeting_search,
            meetings::workspace::meeting_tasks,
            meetings::workspace::meeting_task_toggle,
            meetings::workspace::meeting_task_update,
            meetings::meeting_resume,
            meetings::meeting_start,
            meetings::meeting_control,
            meetings::meeting_process,
            meetings::meeting_pick,
            meetings::meeting_import,
            meetings::meeting_text_download,
            meetings::meeting_text_cancel,
            meetings::meeting_text_remove,
            meetings::meeting_update,
            meetings::meeting_delete,
            meetings::meeting_export,
            meetings::meeting_copy,
            meetings::meeting_audio,
            get_providers,
            save_provider,
            test_provider,
            cancel_provider_test,
            select_provider,
            delete_provider,
            retry_provider_cleanup,
            discard_invalid_provider,
            get_storage_status,
            reveal_recovery_data,
            restart_after_storage_error,
            list_writing_rules,
            upsert_writing_rule,
            delete_writing_rule,
            list_app_rules,
            choose_app_for_rule,
            upsert_app_rule,
            delete_app_rule,
            get_recording_controls,
            begin_shortcut_edit,
            set_shortcut_capture,
            shortcut_edit_heartbeat,
            save_recording_shortcut,
            retry_recording_shortcut,
            get_recording_status,
            recording_hud_ready,
            recording_notice_action,
            recording_control,
            preview_recording_panel,
            get_snapshot,
            overview::overview_activity,
            start_session,
            stop_session,
            cancel_session,
            select_model,
            reload_model,
            clear_recent_results,
            list_models,
            download_model,
            import_model,
            cancel_model_download,
            delete_model,
            list_input_devices,
            microphone_permission_action,
            open_accessibility_settings,
            check_accessibility_permission,
            restart_to_apply_permissions,
            set_input_device,
            update_settings,
            enable_polishing,
            upsert_dictionary,
            delete_dictionary,
            list_history,
            delete_history,
            export_history,
            clear_history,
            copy_result
        ])
        .build(context)
        .expect("Unable to start SpeakMore desktop runtime")
        .run(|app, event| match event {
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } if label == "main" => {
                api.prevent_close();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            // A restart ignores prevent_exit; RunEvent::Exit drains that path.
            tauri::RunEvent::ExitRequested { api, code, .. }
                if code != Some(tauri::RESTART_EXIT_CODE) =>
            {
                if let Some((runtime, deadline, true)) = begin_exit(app) {
                    api.prevent_exit();
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        drain_exit(&app, &runtime, deadline).await;
                        app.exit(code.unwrap_or(0));
                    });
                }
            }
            tauri::RunEvent::Exit => {
                #[cfg(target_os = "macos")]
                workspace_lifecycle::destroy();
                #[cfg(target_os = "macos")]
                fn_shortcut::destroy();
                #[cfg(target_os = "macos")]
                shortcut_capture::destroy();
                // Cmd+Q, Dock quit and logout reach applicationWillTerminate without
                // ExitRequested, and restarts ignore prevent_exit. Finish the shared
                // bounded drain before exiting; a completed drain returns at once.
                if let Some((runtime, deadline, _)) = begin_exit(app) {
                    while Instant::now() < deadline && !exit_drained(app, &runtime) {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                meeting_panel::destroy(app);
                #[cfg(target_os = "macos")]
                recording_panel::destroy(app);
                #[cfg(target_os = "macos")]
                caption_panel::destroy(app);
            }
            _ => {}
        });
}
