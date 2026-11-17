mod actor;
pub(crate) use actor::spec as model_spec;
mod capture;
mod db;
mod native_input;
#[cfg(feature = "model-audio-check")]
mod public_audio;
mod readiness;
pub(crate) use native_input::{Interruption, NativeInput};
pub use readiness::ReadinessCode;

use serde::Serialize;
use speakmore_core::{
    error::{AppError, Result},
    session::{SessionSnapshot, StartMode},
    storage::Store,
    types::*,
};
use speakmore_models::ModelManager;
use speakmore_worker_supervisor::{Configuration, Snapshot as WorkerSnapshot};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

pub type SharedStore = Arc<Mutex<Result<Store>>>;
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub writing_context: Option<speakmore_core::writing::Context>,
    pub writing_rule: Option<String>,
    pub revision: u64,
    pub settings: Settings,
    pub session: SessionSnapshot,
    pub dictionary: Vec<DictionaryEntry>,
    pub recent_results: Vec<ResultRecord>,
    pub usage_today: u32,
    pub can_start: bool,
    pub capture_suspended: bool,
    pub shortcut_needs_release: bool,
    pub readiness_reason: String,
    pub readiness_code: ReadinessCode,
    pub readiness_detail_code: Option<String>,
    pub readiness_action: ReadinessAction,
    pub permissions: crate::permissions::Status,
    pub desktop: bool,
    pub worker: WorkerSnapshot,
    pub model_operation: Option<String>,
    pub model_target_id: Option<String>,
    pub model_name: Option<String>,
    pub audio_level: f32,
    pub last_error: Option<AppError>,
    pub storage_warning: Option<AppError>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessAction {
    None,
    Wait,
    Models,
    Microphone,
    Device,
    Storage,
}
struct Client {
    clipboard: Result<Arc<crate::clipboard::Service>>,
    native: NativeInput,
    sender: mpsc::Sender<actor::Command>,
    published: Arc<Mutex<Result<Snapshot>>>,
    stopping: Arc<AtomicBool>,
}
impl Drop for Client {
    fn drop(&mut self) {
        self.native
            .interrupt(native_input::Interruption::Shutdown, true);
        self.stopping.store(true, Ordering::Release);
    }
}
#[derive(Clone)]
pub struct Handle(Arc<Client>);
impl Handle {
    pub fn launch(
        store: SharedStore,
        models: Result<ModelManager>,
        config: Configuration,
        notify: Arc<dyn Fn(u64) + Send + Sync>,
        notify_delivery: Arc<dyn Fn(crate::delivery::DeliveryUpdate) + Send + Sync>,
    ) -> Result<Self> {
        #[cfg(not(feature = "model-audio-check"))]
        let source = Box::new(speakmore_audio::capture::AudioService::default());
        #[cfg(feature = "model-audio-check")]
        let source = Box::new(public_audio::Source::default());
        Self::launch_with_source(store, models, config, notify, notify_delivery, source)
    }
    fn launch_with_source(
        store: SharedStore,
        models: Result<ModelManager>,
        config: Configuration,
        notify: Arc<dyn Fn(u64) + Send + Sync>,
        notify_delivery: Arc<dyn Fn(crate::delivery::DeliveryUpdate) + Send + Sync>,
        source: Box<dyn capture::Source>,
    ) -> Result<Self> {
        let (sender, commands) = mpsc::channel(64);
        let published = Arc::new(Mutex::new(Err(AppError::new(
            "Initializing",
            "正在准备本地服务…",
            true,
        ))));
        let stopping = Arc::new(AtomicBool::new(false));
        let native = NativeInput::default();
        let clipboard = crate::clipboard::Service::native().map(Arc::new);
        let actor = actor::Actor::new(
            store,
            models,
            config,
            source,
            actor::ControlPlane {
                published: published.clone(),
                stopping: stopping.clone(),
                notify,
                notify_delivery,
                native: native.clone(),
                clipboard: clipboard.clone(),
            },
        )?;
        std::thread::Builder::new()
            .name("speakmore-session".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                match runtime {
                    Ok(runtime) => runtime.block_on(actor.run(commands)),
                    Err(_) => actor.fail_runtime(),
                }
            })
            .map_err(|_| unavailable())?;
        Ok(Self(Arc::new(Client {
            clipboard,
            native,
            sender,
            published,
            stopping,
        })))
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        self.0.published.lock().map_err(|_| unavailable())?.clone()
    }
    /// Borrow the published snapshot for hot paths that need a few fields.
    /// `f` must not take other locks.
    pub fn read<T>(&self, f: impl FnOnce(&Snapshot) -> T) -> Result<T> {
        let published = self.0.published.lock().map_err(|_| unavailable())?;
        published.as_ref().map(f).map_err(Clone::clone)
    }
    pub(crate) fn clipboard_service(&self) -> Result<Arc<crate::clipboard::Service>> {
        self.0.clipboard.clone()
    }
    pub(crate) async fn copy_text(&self, text: String) -> Result<()> {
        self.0
            .clipboard
            .as_ref()
            .map_err(Clone::clone)?
            .copy(text)
            .await
    }
    pub(crate) fn native_input(&self) -> NativeInput {
        self.0.native.clone()
    }
    pub(crate) fn native_stop(&self, id: Uuid, cancel: bool) -> Result<()> {
        let snapshot = self.snapshot()?;
        if snapshot.session.session_id == Some(id) {
            self.0.native.stop(id, snapshot.revision, cancel);
        }
        Ok(())
    }
    fn send(&self, command: actor::Command) -> Result<()> {
        if self.0.stopping.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        self.0
            .sender
            .try_send(command)
            .map_err(|_| AppError::busy())
    }
    pub async fn database<T: Send + 'static>(
        &self,
        task: impl FnOnce(&mut Store) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (reply, response) = oneshot::channel();
        self.send(actor::Command::Database(Box::new(move |store| {
            let result = store.and_then(task);
            Box::new(move |actor| actor.respond(reply, result))
        })))?;
        response.await.map_err(|_| unavailable())?
    }
    /// Read-only store access for UI queries. Unlike `database`, it neither
    /// re-reads the session view nor briefly reports configuration activity.
    pub async fn query<T: Send + 'static>(
        &self,
        task: impl FnOnce(&Store) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (reply, response) = oneshot::channel();
        self.send(actor::Command::Query(Box::new(move |store| {
            let result = store.and_then(|store| task(store));
            Box::new(move |actor| actor.respond(reply, result))
        })))?;
        response.await.map_err(|_| unavailable())?
    }
    pub async fn start(&self, request_id: Uuid, mode: StartMode) -> Result<Uuid> {
        self.start_with_origin(request_id, mode, None).await
    }
    pub(crate) fn start_from_menu(
        &self,
    ) -> impl std::future::Future<Output = Result<Uuid>> + Send + 'static {
        let handle = self.clone();
        #[cfg(target_os = "macos")]
        let origin = Some(self.0.native.native_origin(true));
        #[cfg(not(target_os = "macos"))]
        let origin = None;
        async move {
            handle
                .start_with_origin(Uuid::new_v4(), StartMode::Toggle, origin)
                .await
        }
    }
    async fn start_with_origin(
        &self,
        request_id: Uuid,
        mode: StartMode,
        origin: Option<native_input::Press>,
    ) -> Result<Uuid> {
        let control_revision = origin
            .as_ref()
            .map_or_else(|| self.0.native.control_revision(), |p| p.control_revision);
        if self.0.native.editing() {
            return Err(AppError::busy());
        }
        let lifecycle = self.0.native.lifecycle();
        if lifecycle.blocked() {
            return Err(interrupted());
        }
        let (reply, response) = oneshot::channel();
        self.send(actor::Command::Start {
            request_id,
            mode,
            epoch: origin.as_ref().map_or(lifecycle.epoch, |p| p.epoch),
            control_revision,
            origin,
            reply,
        })?;
        response.await.map_err(|_| unavailable())?
    }
    pub(crate) async fn begin_shortcut_edit(&self) -> Result<Uuid> {
        let (reply, response) = oneshot::channel();
        self.send(actor::Command::EditShortcuts(reply))?;
        response.await.map_err(|_| unavailable())?
    }
    /// RAII reservation owned by native provider work. A dropped caller/response
    /// releases the weak actor reservation without relying on a mailbox message.
    pub(crate) async fn reserve_settings_operation(&self) -> Result<Arc<()>> {
        let (reply, response) = oneshot::channel();
        self.send(actor::Command::ReserveSettingsOperation(reply))?;
        response.await.map_err(|_| unavailable())?
    }
    pub(crate) fn set_background_interrupt(
        &self,
        value: Option<(
            Arc<std::sync::atomic::AtomicBool>,
            Arc<std::sync::atomic::AtomicBool>,
        )>,
    ) -> Result<()> {
        self.send(actor::Command::BackgroundInterrupt(value))
    }
    pub(crate) async fn reserve_background(
        &self,
        cancel: Arc<std::sync::atomic::AtomicBool>,
        yielded: Arc<std::sync::atomic::AtomicBool>,
    ) -> Result<Arc<()>> {
        let (reply, response) = oneshot::channel();
        self.send(actor::Command::ReserveBackground(cancel, yielded, reply))?;
        response.await.map_err(|_| unavailable())?
    }
    pub(crate) fn install_polisher(
        &self,
        access: crate::providers::session::SessionAccess,
    ) -> Result<()> {
        self.send(actor::Command::InstallPolisher(access))
    }
    pub async fn stop(&self, session_id: Uuid, cancel: bool) -> Result<()> {
        let (reply, response) = oneshot::channel();
        self.send(actor::Command::Stop {
            session_id,
            cancel,
            reply,
        })?;
        response.await.map_err(|_| unavailable())?
    }
    pub async fn select_model(
        &self,
        model_id: Option<String>,
        expected_revision: i64,
    ) -> Result<()> {
        let (reply, response) = oneshot::channel();
        self.send(actor::Command::Select {
            model_id,
            expected_revision,
            reply,
        })?;
        response.await.map_err(|_| unavailable())?
    }
    pub async fn reload_model(&self) -> Result<()> {
        let (reply, response) = oneshot::channel();
        self.send(actor::Command::Reload(reply))?;
        response.await.map_err(|_| unavailable())?
    }
    pub async fn set_device(
        &self,
        device_id: Option<String>,
        expected_revision: i64,
    ) -> Result<Settings> {
        let (reply, response) = oneshot::channel();
        self.send(actor::Command::Device {
            device_id,
            expected_revision,
            reply,
        })?;
        response.await.map_err(|_| unavailable())?
    }
    pub async fn download_model(&self, model_id: String) -> Result<Uuid> {
        let (reply, response) = oneshot::channel();
        self.send(actor::Command::Download { model_id, reply })?;
        response.await.map_err(|_| unavailable())?
    }
    pub async fn import_model(
        &self,
        model_id: String,
        source: speakmore_models::PreparedImport,
    ) -> Result<Uuid> {
        let (reply, response) = oneshot::channel();
        self.send(actor::Command::Import {
            model_id,
            source,
            reply,
        })?;
        response.await.map_err(|_| unavailable())?
    }
    pub async fn delete_model(&self, model_id: String) -> Result<()> {
        let (reply, response) = oneshot::channel();
        self.send(actor::Command::Delete { model_id, reply })?;
        response.await.map_err(|_| unavailable())?
    }
    pub async fn clear_results(&self) -> Result<()> {
        let (reply, response) = oneshot::channel();
        self.send(actor::Command::ClearResults(reply))?;
        response.await.map_err(|_| unavailable())?
    }
    pub async fn result_text(&self, id: Uuid, variant: String) -> Result<String> {
        let (reply, response) = oneshot::channel();
        self.send(actor::Command::ResultText { id, variant, reply })?;
        response.await.map_err(|_| unavailable())?
    }
    pub fn shutdown(&self) {
        self.0
            .native
            .interrupt(native_input::Interruption::Shutdown, true);
        self.0.stopping.store(true, Ordering::Release);
    }
}

fn interrupted() -> AppError {
    AppError::new(
        "SessionInterrupted",
        "系统状态发生变化，听写已中断。恢复后请重新开始。",
        true,
    )
}
fn unavailable() -> AppError {
    AppError::new(
        "SessionUnavailable",
        "本地听写服务不可用，请重新启动。",
        true,
    )
}
