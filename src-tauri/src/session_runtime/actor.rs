use super::{
    capture::{Capture, Source},
    db,
    readiness::{Blocker, ReadinessCode},
    ReadinessAction, SharedStore, Snapshot,
};
use speakmore_core::{
    error::{AppError, Result},
    session::{Phase, SessionController, StartMode},
    types::*,
};
use speakmore_models::{ModelLease, ModelManager};
use speakmore_worker_supervisor::{
    Configuration, Event, ModelSpec, Phase as WorkerPhase, Supervisor, Transcription,
};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

/// A successful input-device probe keeps the start path ready this long.
const INPUT_CHECK_TTL: Duration = Duration::from_secs(10);

type Reply<T> = oneshot::Sender<Result<T>>;
pub(super) struct ControlPlane {
    pub clipboard: Result<Arc<crate::clipboard::Service>>,
    pub published: Arc<Mutex<Result<Snapshot>>>,
    pub stopping: Arc<AtomicBool>,
    pub notify: Arc<dyn Fn(u64) + Send + Sync>,
    pub notify_delivery: Arc<dyn Fn(crate::delivery::DeliveryUpdate) + Send + Sync>,
    pub native: super::NativeInput,
}
pub(super) enum Command {
    InstallPolisher(crate::providers::session::SessionAccess),
    Database(db::Task),
    Query(db::Task),
    Start {
        request_id: Uuid,
        mode: StartMode,
        epoch: u64,
        control_revision: u64,
        origin: Option<super::native_input::Press>,
        reply: Reply<Uuid>,
    },
    EditShortcuts(Reply<Uuid>),
    ReserveSettingsOperation(Reply<Arc<()>>),
    BackgroundInterrupt(Option<(Arc<AtomicBool>, Arc<AtomicBool>)>),
    ReserveBackground(Arc<AtomicBool>, Arc<AtomicBool>, Reply<Arc<()>>),
    Stop {
        session_id: Uuid,
        cancel: bool,
        reply: Reply<()>,
    },
    Select {
        model_id: Option<String>,
        expected_revision: i64,
        reply: Reply<()>,
    },
    Reload(Reply<()>),
    Device {
        device_id: Option<String>,
        expected_revision: i64,
        reply: Reply<Settings>,
    },
    Download {
        model_id: String,
        reply: Reply<Uuid>,
    },
    Import {
        model_id: String,
        source: speakmore_models::PreparedImport,
        reply: Reply<Uuid>,
    },
    Delete {
        model_id: String,
        reply: Reply<()>,
    },
    ClearResults(Reply<()>),
    ResultText {
        id: Uuid,
        variant: String,
        reply: Reply<String>,
    },
}
pub(super) enum Completed {
    WritingContext {
        id: Uuid,
        context: Option<speakmore_core::writing::Context>,
    },
    Polish {
        id: Uuid,
        result: crate::providers::session::Resolution,
    },
    InputCheck {
        selected: Option<String>,
        result: speakmore_audio::error::Result<()>,
    },
    Database(db::Finished),
    Lease {
        operation: Uuid,
        result: Result<ModelLease>,
    },
    Device {
        operation: Uuid,
        result: Result<()>,
        device_id: Option<String>,
        expected_revision: i64,
        reply: Reply<Settings>,
    },
    Deleted {
        operation: Uuid,
        result: Result<()>,
        reply: Reply<()>,
    },
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Verifying,
    Loading,
    Committing,
    Unloading,
    Retiring,
    Restoring,
}
struct ModelWork {
    id: Uuid,
    target: Option<String>,
    previous: Option<String>,
    candidate: Option<ModelLease>,
    stage: Stage,
    reply: Option<Reply<()>>,
    failure: Option<AppError>,
    reload: bool,
    restore_attempted: bool,
}
struct Current {
    writing_context: speakmore_core::writing::Context,
    app_identity: Option<String>,
    app_policy: Arc<speakmore_core::app_rules::Watch>,
    target: Option<Arc<crate::target::Lease>>,
    target_generation: u64,
    delivery: Option<Arc<crate::delivery::Attempt>>,
    delivery_intent: bool,
    target_check: Option<crate::target::Check>,
    polish_access: Option<crate::providers::session::SessionAccess>,
    polish_watch: Arc<speakmore_core::processing::Watch>,
    polish_job: Option<PolishJob>,
    polish_result: Option<crate::providers::session::Resolution>,
    finalizing_result: bool,
    generated_record: Option<ResultRecord>,
    storage_ready: bool,
    manual_reason: speakmore_core::delivery::ManualReason,
    id: Uuid,
    frozen: db::View,
    model_id: String,
    model_sha: String,
    capture: Option<Box<dyn Capture>>,
    awaiting_worker: bool,
    persisting: bool,
    duration_ms: u64,
    control_revision: u64,
}
struct MemoryResult {
    record: ResultRecord,
    expires: Instant,
}
struct PolishJob {
    cancel: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for PolishJob {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}
pub(super) struct Actor {
    writing_context: Option<speakmore_core::writing::Context>,
    writing_rule: Option<String>,
    polisher: Option<crate::providers::session::SessionAccess>,
    clipboard: Result<Arc<crate::clipboard::Service>>,
    delivery_tx: mpsc::Sender<crate::delivery::Event>,
    delivery_rx: mpsc::Receiver<crate::delivery::Event>,
    targets: Option<crate::target::Service>,
    permissions: crate::permissions::Status,
    last_permission_check: Instant,
    input_check: Option<(Option<String>, speakmore_audio::error::Result<()>, Instant)>,
    input_check_pending: bool,
    native: super::NativeInput,
    lifecycle: super::native_input::Lifecycle,
    retiring_for_interrupt: bool,
    interrupt_teardown: bool,
    shortcut_needs_release: bool,
    shortcut_editing: bool,
    observed_press: Option<Uuid>,
    native_session: Option<Uuid>,
    models: Result<ModelManager>,
    config: Configuration,
    worker: Supervisor,
    loaded: Option<ModelLease>,
    model_work: Option<ModelWork>,
    config_operation: Option<Uuid>,
    settings_operation: Option<std::sync::Weak<()>>,
    background_interrupt: Option<(Arc<AtomicBool>, Arc<AtomicBool>)>,
    deferred_toggle: Option<(super::native_input::Press, bool)>,
    kernel: SessionController,
    current: Option<Current>,
    source: Box<dyn Source>,
    db: SyncSender<db::Job>,
    db_pending: usize,
    db_quiet: usize,
    db_overflow: VecDeque<db::Job>,
    view: Option<db::View>,
    store_error: Option<AppError>,
    completed_tx: mpsc::Sender<Completed>,
    completed: mpsc::Receiver<Completed>,
    published: Arc<Mutex<Result<Snapshot>>>,
    stopping: Arc<AtomicBool>,
    notify: Arc<dyn Fn(u64) + Send + Sync>,
    notify_delivery: Arc<dyn Fn(crate::delivery::DeliveryUpdate) + Send + Sync>,
    origin: Instant,
    last_tick: Instant,
    last_publish: Instant,
    day_check: Instant,
    revision: u64,
    published_state: Vec<u8>,
    dirty: bool,
    replies: Vec<Box<dyn FnOnce() + Send>>,
    results: VecDeque<MemoryResult>,
    last_error: Option<AppError>,
    storage_warning: Option<AppError>,
    initial_model_attempted: bool,
    last_models_revision: u64,
    shutting_down: bool,
}
impl Actor {
    pub(super) fn new(
        store: SharedStore,
        models: Result<ModelManager>,
        config: Configuration,
        source: Box<dyn Source>,
        control: ControlPlane,
    ) -> Result<Self> {
        let ControlPlane {
            clipboard,
            published,
            stopping,
            notify,
            notify_delivery,
            native,
        } = control;
        let (completed_tx, completed) = mpsc::channel(64);
        let (delivery_tx, delivery_rx) = mpsc::channel(2);
        let db = db::spawn(store, completed_tx.clone())?;
        let worker = Supervisor::new(config.clone()).map_err(worker_error)?;
        Ok(Self {
            writing_context: None,
            writing_rule: None,
            polisher: None,
            clipboard,
            delivery_tx,
            delivery_rx,
            targets: crate::target::Service::native(native.clone()),
            permissions: source.permissions(),
            last_permission_check: Instant::now(),
            input_check: None,
            input_check_pending: false,
            native,
            lifecycle: Default::default(),
            retiring_for_interrupt: false,
            interrupt_teardown: false,
            shortcut_needs_release: false,
            shortcut_editing: false,
            observed_press: None,
            native_session: None,
            models,
            config,
            worker,
            loaded: None,
            model_work: None,
            config_operation: None,
            settings_operation: None,
            background_interrupt: None,
            deferred_toggle: None,
            kernel: SessionController::default(),
            current: None,
            source,
            db,
            db_pending: 0,
            db_quiet: 0,
            db_overflow: VecDeque::new(),
            view: None,
            store_error: None,
            completed_tx,
            completed,
            published,
            stopping,
            notify,
            notify_delivery,
            origin: Instant::now(),
            last_tick: Instant::now(),
            last_publish: Instant::now(),
            day_check: Instant::now(),
            revision: 0,
            published_state: Vec::new(),
            dirty: true,
            replies: vec![],
            results: VecDeque::new(),
            last_error: None,
            storage_warning: None,
            initial_model_attempted: false,
            last_models_revision: 0,
            shutting_down: false,
        })
    }
    pub(super) fn fail_runtime(self) {
        if let Ok(mut snapshot) = self.published.lock() {
            *snapshot = Err(super::unavailable());
        }
        (self.notify)(1);
    }
    pub(super) async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        self.refresh_db();
        // Observe the model registry before the first idle period elapses.
        self.tick();
        let native = self.native.clone();
        loop {
            self.observe_lifecycle();
            if self.stopping.load(Ordering::Acquire) && !self.shutting_down {
                self.shutting_down = true;
                if let Some(id) = self.current.as_ref().map(|c| c.id) {
                    self.stop(id, true);
                }
                self.worker.shutdown();
                self.dirty = true;
            }
            // Cancel before polling late audio/worker completions on shutdown.
            // A full command mailbox still cannot starve worker deadlines.
            if self.last_tick.elapsed() >= self.tick_period() {
                self.tick();
            }
            self.pump_db();
            let drained = self.shutting_down
                && self.worker.snapshot().phase == WorkerPhase::Stopped
                && !self.source.busy()
                && self.db_pending == 0
                && self.db_quiet == 0
                && self.current.is_none();
            if drained {
                let reply = self
                    .model_work
                    .take()
                    .and_then(|mut work| work.reply.take());
                self.loaded.take();
                if let Some(reply) = reply {
                    self.respond(reply, Err(super::unavailable()));
                }
                self.dirty = true;
            }
            if self.dirty {
                self.publish();
            }
            for reply in std::mem::take(&mut self.replies) {
                reply();
            }
            if drained {
                break;
            }
            let next_tick = tokio::time::Instant::from_std(self.last_tick + self.tick_period());
            tokio::select! {
                biased;
                _ = native.changed() => self.tick(),
                command = commands.recv(), if !self.shutting_down => {
                    match command { Some(command) => self.command(command), None => { self.stopping.store(true, Ordering::Release); } }
                }
                Some(completed) = self.completed.recv() => self.complete(completed),
                _ = tokio::time::sleep_until(next_tick) => self.tick(),
            }
        }
    }
    /// 50 Hz only while something time-critical is in flight. Idle native
    /// input and lifecycle changes wake the actor directly.
    fn tick_period(&self) -> Duration {
        let active = self.current.is_some()
            || self.model_work.is_some()
            || self.shutting_down
            || self.stopping.load(Ordering::Acquire)
            || self.retiring_for_interrupt
            || self.deferred_toggle.is_some()
            || !self.db_overflow.is_empty()
            || self.db_pending > 0
            || (!self.initial_model_attempted && self.view.is_some())
            || self.source.busy()
            || !matches!(
                self.worker.snapshot().phase,
                WorkerPhase::Ready
                    | WorkerPhase::Dormant
                    | WorkerPhase::Stopped
                    | WorkerPhase::Unavailable
                    | WorkerPhase::Paused
            );
        Duration::from_millis(if active { 20 } else { 250 })
    }
    pub(super) fn respond<T: Send + 'static>(&mut self, reply: Reply<T>, result: Result<T>) {
        self.replies.push(Box::new(move || {
            let _ = reply.send(result);
        }));
        self.dirty = true;
    }
    fn now_ms(&self) -> u64 {
        self.origin.elapsed().as_millis() as u64
    }
    fn queue_db(&mut self, task: db::Task) {
        self.db_pending += 1;
        self.dirty = true;
        self.enqueue_db((task, true));
    }
    /// Work that cannot change the published view (reads, history row
    /// transitions): no view re-read and no configuration-busy readiness.
    fn queue_db_quiet(&mut self, task: db::Task) {
        self.db_quiet += 1;
        self.enqueue_db((task, false));
    }
    // Never runs a callback synchronously: callers may hold NativeInput or be
    // mid-transition. Overflow keeps order and is retried by the run loop.
    fn enqueue_db(&mut self, job: db::Job) {
        use std::sync::mpsc::TrySendError;
        let job = if self.db_overflow.is_empty() {
            match self.db.try_send(job) {
                Ok(()) => return,
                Err(TrySendError::Full(job) | TrySendError::Disconnected(job)) => job,
            }
        } else {
            job
        };
        self.db_overflow.push_back(job);
    }
    fn pump_db(&mut self) {
        use std::sync::mpsc::TrySendError;
        for _ in 0..self.db_overflow.len() {
            let Some(job) = self.db_overflow.pop_front() else {
                break;
            };
            match self.db.try_send(job) {
                Ok(()) => {}
                Err(TrySendError::Full(job)) => {
                    self.db_overflow.push_front(job);
                    break;
                }
                Err(TrySendError::Disconnected((task, refresh))) => {
                    let applied = task(Err(db::unavailable()));
                    self.db_finished(refresh);
                    applied(self);
                }
            }
        }
    }
    fn db_finished(&mut self, refresh: bool) {
        if refresh {
            self.db_pending = self.db_pending.saturating_sub(1);
        } else {
            self.db_quiet = self.db_quiet.saturating_sub(1);
        }
        self.dirty = true;
    }
    fn refresh_db(&mut self) {
        self.queue_db(Box::new(|_| Box::new(|_| {})));
    }
    fn config_idle(&self) -> Result<()> {
        if self.native.editing() {
            return Err(AppError::busy());
        }
        if self.capture_suspended() {
            return Err(super::interrupted());
        }
        if self.current.is_some()
            || self.source.busy()
            || self.model_work.is_some()
            || self.config_operation.is_some()
            || self.settings_operation_busy()
            || self.db_pending != 0
        {
            Err(AppError::busy())
        } else {
            Ok(())
        }
    }
    fn revision_matches(&self, expected: i64) -> Result<()> {
        let view = self.view.as_ref().ok_or_else(db::unavailable)?;
        if expected != view.settings.revision {
            Err(AppError::conflict())
        } else {
            Ok(())
        }
    }
    fn manager(&self) -> Result<ModelManager> {
        self.models.clone()
    }
    fn command(&mut self, command: Command) {
        self.observe_lifecycle();
        match command {
            Command::InstallPolisher(access) => self.polisher = Some(access),
            Command::BackgroundInterrupt(value) => self.background_interrupt = value,
            Command::ReserveBackground(cancel, yielded, reply) => {
                let result = self.config_idle().map(|_| {
                    let lease = Arc::new(());
                    self.settings_operation = Some(Arc::downgrade(&lease));
                    self.background_interrupt = Some((cancel, yielded));
                    lease
                });
                self.respond(reply, result);
            }
            Command::ReserveSettingsOperation(reply) => {
                let result = self.config_idle().map(|_| {
                    let lease = Arc::new(());
                    self.settings_operation = Some(Arc::downgrade(&lease));
                    self.background_interrupt = None;
                    lease
                });
                self.respond(reply, result);
            }
            Command::EditShortcuts(reply) => {
                let result = self.config_idle().and_then(|_| self.native.begin_edit());
                // This command publishes editing immediately. Remember that
                // state even if begin/end both happen before the next tick.
                if result.is_ok() {
                    self.shortcut_editing = true;
                }
                self.respond(reply, result);
            }
            // Caller-supplied work is bounded; internal persistence never is.
            Command::Database(task) | Command::Query(task) if self.db_overflow.len() >= 32 => {
                task(Err(AppError::busy()))(self)
            }
            Command::Database(task) => self.queue_db(task),
            Command::Query(task) => self.queue_db_quiet(task),
            Command::Start {
                request_id,
                mode,
                epoch,
                control_revision,
                origin,
                reply,
            } => {
                let result = self.start(request_id, mode, epoch, control_revision, origin);
                self.respond(reply, result);
            }
            Command::Stop {
                session_id,
                cancel,
                reply,
            } => {
                self.stop(session_id, cancel);
                self.respond(reply, Ok(()));
            }
            Command::Select {
                model_id,
                expected_revision,
                reply,
            } => {
                if let Err(error) = self
                    .config_idle()
                    .and_then(|_| self.revision_matches(expected_revision))
                {
                    self.respond(reply, Err(error));
                } else {
                    self.select(model_id, Some(reply), false);
                }
            }
            Command::Reload(reply) => {
                if let Err(error) = self.config_idle() {
                    self.respond(reply, Err(error));
                } else if let Some(id) = self
                    .view
                    .as_ref()
                    .and_then(|v| v.settings.active_model_id.clone())
                {
                    self.select(Some(id), Some(reply), true);
                } else {
                    self.respond(
                        reply,
                        Err(AppError::new("NotReady", "请先选择已安装模型。", true)),
                    );
                }
            }
            Command::Device {
                device_id,
                expected_revision,
                reply,
            } => self.set_device(device_id, expected_revision, reply),
            Command::Download { model_id, reply } => {
                let result = self.config_idle().and_then(|_| {
                    self.manager()?
                        .start_download(&model_id)
                        .map_err(super::super::model_error)
                });
                self.respond(reply, result);
            }
            Command::Import {
                model_id,
                source,
                reply,
            } => {
                let result = self.config_idle().and_then(|_| {
                    self.manager()?
                        .start_import(&model_id, source)
                        .map_err(super::super::model_error)
                });
                self.respond(reply, result);
            }
            Command::Delete { model_id, reply } => {
                // Current model is explicitly unloaded first via select_model(null).
                let result = self.config_idle().and_then(|_| {
                    if self
                        .loaded
                        .as_ref()
                        .is_some_and(|m| m.bundle.model.installation_id() == model_id)
                    {
                        return Err(AppError::new(
                            "ModelInUse",
                            "请先停用当前模型，再移除文件。",
                            true,
                        ));
                    }
                    self.manager()
                });
                match result {
                    Ok(manager) => {
                        let operation = Uuid::new_v4();
                        self.config_operation = Some(operation);
                        let done = self.completed_tx.clone();
                        tokio::spawn(async move {
                            let result = manager
                                .delete(&model_id)
                                .await
                                .map_err(super::super::model_error);
                            let _ = done
                                .send(Completed::Deleted {
                                    operation,
                                    result,
                                    reply,
                                })
                                .await;
                        });
                    }
                    Err(error) => self.respond(reply, Err(error)),
                }
            }
            Command::ClearResults(reply) => {
                self.results.clear();
                self.respond(reply, Ok(()));
            }
            Command::ResultText { id, variant, reply } => {
                self.expire_results();
                if !["raw", "polished"].contains(&variant.as_str()) {
                    self.respond(reply, Err(AppError::invalid("结果版本无效。")));
                } else if let Some(record) = self
                    .results
                    .iter()
                    .find(|r| r.record.session_id == id)
                    .map(|r| r.record.clone())
                {
                    self.respond(
                        reply,
                        Ok(if variant == "polished" {
                            record.polished_text.unwrap_or(record.raw_text)
                        } else {
                            record.raw_text
                        }),
                    );
                } else {
                    self.queue_db_quiet(Box::new(move |store| {
                        let result = store
                            .and_then(|s| s.find_result(id))
                            .and_then(|record| {
                                record.ok_or_else(|| {
                                    AppError::new("ResultExpired", "结果已过期或被删除。", false)
                                })
                            })
                            .map(|r| {
                                if variant == "polished" {
                                    r.polished_text.unwrap_or(r.raw_text)
                                } else {
                                    r.raw_text
                                }
                            });
                        Box::new(move |actor| actor.respond(reply, result))
                    }));
                }
            }
        }
        self.dirty = true;
    }
    fn set_device(
        &mut self,
        device_id: Option<String>,
        expected_revision: i64,
        reply: Reply<Settings>,
    ) {
        if let Err(error) = self
            .config_idle()
            .and_then(|_| self.revision_matches(expected_revision))
        {
            self.respond(reply, Err(error));
            return;
        }
        if device_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 1024 || id.chars().any(char::is_control))
        {
            self.respond(reply, Err(AppError::invalid("录音设备标识无效。")));
            return;
        }
        let operation = Uuid::new_v4();
        self.config_operation = Some(operation);
        let done = self.completed_tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = if let Some(id) = &device_id {
                speakmore_audio::device::list_inputs()
                    .map_err(super::super::audio_error)
                    .and_then(|devices| {
                        let device = devices.iter().find(|d| d.id == *id).ok_or_else(|| {
                            super::super::audio_error(
                                speakmore_audio::error::AudioError::DeviceUnavailable,
                            )
                        })?;
                        if device.configuration.is_none() {
                            return Err(super::super::audio_error(
                                speakmore_audio::error::AudioError::UnsupportedAudioFormat,
                            ));
                        }
                        Ok(())
                    })
            } else {
                Ok(())
            };
            let _ = done.blocking_send(Completed::Device {
                operation,
                result,
                device_id,
                expected_revision,
                reply,
            });
        });
    }
    fn select(&mut self, target: Option<String>, reply: Option<Reply<()>>, reload: bool) {
        let previous = self
            .view
            .as_ref()
            .and_then(|v| v.settings.active_model_id.clone());
        if !reload
            && target.is_some()
            && target == previous
            && target == self.worker.snapshot().model_id
            && self.worker.snapshot().phase == WorkerPhase::Ready
        {
            if let Some(reply) = reply {
                self.respond(reply, Ok(()));
            }
            return;
        }
        let id = Uuid::new_v4();
        self.model_work = Some(ModelWork {
            id,
            target: target.clone(),
            previous,
            candidate: None,
            stage: if target.is_some() {
                Stage::Verifying
            } else {
                Stage::Unloading
            },
            reply,
            failure: None,
            reload,
            restore_attempted: false,
        });
        // Automatic reload after an interruption must not erase why the
        // recording was cancelled. Explicit model operations may clear it.
        if self
            .model_work
            .as_ref()
            .is_some_and(|work| work.reply.is_some())
        {
            self.last_error = None;
        }
        match target {
            Some(model_id) => match self.manager() {
                Ok(manager) => {
                    let done = self.completed_tx.clone();
                    tokio::spawn(async move {
                        let result = manager
                            .lease(&model_id)
                            .await
                            .map_err(super::super::model_error);
                        let _ = done
                            .send(Completed::Lease {
                                operation: id,
                                result,
                            })
                            .await;
                    });
                }
                Err(error) => self.finish_model(Err(error)),
            },
            None => self.worker.shutdown(),
        }
        self.dirty = true;
    }
    fn finish_model(&mut self, result: Result<()>) {
        if let Err(error) = &result {
            self.last_error = Some(error.clone());
            // A failed lease may invalidate the installed row and clear its selection.
            if !self.shutting_down {
                self.refresh_db();
            }
        }
        if let Some(mut work) = self.model_work.take() {
            if let Some(reply) = work.reply.take() {
                self.respond(reply, result);
            }
        }
        self.dirty = true;
    }
    fn commit_model(&mut self) {
        let Some(work) = self.model_work.as_mut() else {
            return;
        };
        work.stage = Stage::Committing;
        let operation = work.id;
        let target = work.target.clone();
        let previous = work.previous.clone();
        let digest = work
            .candidate
            .as_ref()
            .map(|lease| lease.bundle.model.bundle_digest());
        self.queue_db(Box::new(move |store| {
            let result = store.and_then(|s| {
                s.activate_model(
                    target.as_deref(),
                    digest.as_deref(),
                    previous.as_deref(),
                    super::super::now(),
                )
            });
            Box::new(move |actor| {
                if actor.retiring_for_interrupt {
                    return;
                }
                if actor.model_work.as_ref().is_none_or(|w| w.id != operation) {
                    return;
                }
                match result {
                    Ok(_) => {
                        let candidate = actor.model_work.as_mut().unwrap().candidate.take();
                        actor.loaded = candidate;
                        actor.finish_model(Ok(()));
                    }
                    Err(error) => {
                        let work = actor.model_work.as_mut().unwrap();
                        work.failure = Some(error);
                        work.stage = Stage::Retiring;
                        actor.worker.shutdown();
                    }
                }
            })
        }));
    }
    fn complete(&mut self, completed: Completed) {
        self.observe_lifecycle();
        match completed {
            Completed::WritingContext { id, context } => {
                if let Some(current) = self
                    .current
                    .as_mut()
                    .filter(|c| c.id == id && !c.finalizing_result)
                {
                    if let Some(context) = context {
                        current.writing_context = context.clone();
                        self.writing_rule = speakmore_core::writing::matching(
                            &current.frozen.writing_rules,
                            &context,
                        )
                        .map(|r| r.display_name.clone());
                        self.writing_context = Some(context);
                        self.dirty = true;
                    }
                }
            }
            Completed::Polish { id, result } => {
                if let Some(current) = self
                    .current
                    .as_mut()
                    .filter(|c| c.id == id && c.polish_job.is_some())
                {
                    current.polish_job.take();
                    current.polish_result = Some(result);
                    self.complete_generation();
                }
            }
            Completed::InputCheck { selected, result } => {
                self.input_check_pending = false;
                let unchanged = self.input_check.as_ref().is_some_and(|(id, old, at)| {
                    *id == selected && *old == result && at.elapsed() < INPUT_CHECK_TTL
                });
                self.input_check = Some((selected, result, Instant::now()));
                if unchanged {
                    return;
                }
            }
            Completed::Database(finished) => {
                self.db_finished(finished.view.is_some());
                match finished.view {
                    Some(Ok(view)) => {
                        self.view = Some(view);
                        self.store_error = None;
                        self.check_target_policy();
                    }
                    Some(Err(error)) => self.store_error = Some(error),
                    None => {}
                }
                (finished.applied)(self);
            }
            Completed::Lease { operation, result } => {
                if !self
                    .model_work
                    .as_ref()
                    .is_some_and(|w| w.id == operation && w.stage == Stage::Verifying)
                    || self.shutting_down
                    || self.capture_suspended()
                {
                    return;
                }
                match result {
                    Ok(lease) => {
                        let spec = spec(&lease);
                        let work = self.model_work.as_mut().unwrap();
                        work.candidate = Some(lease);
                        work.stage = Stage::Loading;
                        let result = if work.reload
                            && self.worker.snapshot().phase != WorkerPhase::Dormant
                            && self.worker.snapshot().phase != WorkerPhase::Stopped
                        {
                            self.worker.reload().map_err(worker_error)
                        } else {
                            if self.worker.snapshot().phase == WorkerPhase::Stopped {
                                self.worker = Supervisor::new(self.config.clone()).unwrap();
                            }
                            spec.and_then(|s| self.worker.set_model(s).map_err(worker_error))
                        };
                        if let Err(error) = result {
                            self.finish_model(Err(error));
                        }
                    }
                    Err(error)
                        if self.loaded_id() == self.model_work.as_ref().unwrap().target
                            && matches!(
                                error.code.as_str(),
                                "SizeMismatch"
                                    | "HashMismatch"
                                    | "NotInstalled"
                                    | "UnsupportedModel"
                            ) =>
                    {
                        // The running worker still maps the invalidated bundle.
                        // Reap it before releasing that lease; never restore it.
                        let work = self.model_work.as_mut().unwrap();
                        work.failure = Some(error);
                        work.stage = Stage::Retiring;
                        self.worker.shutdown();
                    }
                    Err(error) => self.finish_model(Err(error)),
                }
            }
            Completed::Device {
                operation,
                result,
                device_id,
                expected_revision,
                reply,
            } => {
                if self.config_operation != Some(operation) {
                    self.respond(reply, Err(AppError::busy()));
                    return;
                }
                if let Err(error) = result {
                    self.config_operation = None;
                    self.respond(reply, Err(error));
                } else {
                    self.queue_db(Box::new(move |store| {
                        let result = store.and_then(|s| {
                            s.set_input_device(expected_revision, device_id, super::super::now())
                        });
                        Box::new(move |actor| {
                            actor.config_operation = None;
                            actor.respond(reply, result);
                        })
                    }));
                }
            }
            Completed::Deleted {
                operation,
                result,
                reply,
            } => {
                // The manager has finished its registry write. Observe that
                // revision now so the next tick does not enqueue a duplicate
                // refresh after this command has already acknowledged success.
                if let Ok(manager) = &self.models {
                    self.last_models_revision = manager.snapshot().revision;
                }
                // Keep configuration reserved until the published view contains
                // the registry change. A completed file operation alone does not
                // mean the next model command can safely start.
                self.queue_db(Box::new(move |store| {
                    let result = result.and_then(|_| store.map(|_| ()));
                    Box::new(move |actor| {
                        if actor.config_operation == Some(operation) {
                            actor.config_operation = None;
                        }
                        let result =
                            result.and_then(|()| actor.store_error.clone().map_or(Ok(()), Err));
                        actor.respond(reply, result);
                    })
                }));
            }
        }
        self.dirty = true;
    }
    fn start(
        &mut self,
        request_id: Uuid,
        mode: StartMode,
        epoch: u64,
        control_revision: u64,
        origin: Option<super::native_input::Press>,
    ) -> Result<Uuid> {
        self.check_permissions();
        self.observe_lifecycle();
        if self.capture_suspended()
            || self.lifecycle.epoch != epoch
            || self.native.control_revision() != control_revision
            || origin
                .as_ref()
                .is_some_and(|p| p.epoch != epoch || p.at.elapsed() >= Duration::from_secs(1))
        {
            return Err(super::interrupted());
        }
        let ready = self.readiness().is_none();
        let possible_target = origin.as_ref().is_some_and(|p| !p.manual_only)
            && self.permissions.accessibility
            && self.targets.is_some()
            && self
                .view
                .as_ref()
                .is_some_and(|v| v.settings.delivery_mode != "manual");
        let id = self
            .kernel
            .start(request_id, mode, self.now_ms(), ready, possible_target)?;
        // The kernel may return a cached completed request ID. It must never reopen a microphone.
        if self.current.is_some() || self.kernel.snapshot(self.now_ms()).session_id != Some(id) {
            return Ok(id);
        }
        let frozen = self.view.as_ref().ok_or_else(db::unavailable)?.clone();
        let app_identity = origin.as_ref().and_then(|p| p.app_identity.clone());
        let app_policy = frozen.app_policies.watch(if origin.is_some() {
            app_identity.as_deref()
        } else {
            Some("com.speakmore.desktop")
        });
        let lease = self
            .loaded
            .as_ref()
            .ok_or_else(|| AppError::new("NotReady", "模型尚未就绪。", true))?;
        let model = lease
            .bundle
            .model
            .artifacts
            .iter()
            .find(|a| a.role == "model")
            .unwrap();
        use speakmore_core::delivery::ManualReason;
        let manual_reason = if frozen.settings.delivery_mode == "manual"
            || app_policy.manual_only()
            || origin.as_ref().is_some_and(|p| p.manual_only)
        {
            ManualReason::UserRequested
        } else if origin.is_none() {
            ManualReason::MainWindow
        } else if !self.permissions.accessibility {
            ManualReason::AccessibilityRequired
        } else {
            ManualReason::CompatibilityUnverified
        };
        let target = if manual_reason == ManualReason::CompatibilityUnverified {
            self.targets.as_ref().map(|targets| {
                Arc::new(targets.begin(
                    id,
                    origin.as_ref().unwrap().target_generation,
                    origin.as_ref().unwrap().at,
                ))
            })
        } else {
            None
        };
        let manual_reason =
            if manual_reason == ManualReason::CompatibilityUnverified && target.is_none() {
                ManualReason::TargetUnavailable
            } else {
                manual_reason
            };
        let context = speakmore_core::writing::Context {
            app_identity: app_identity
                .clone()
                .or_else(|| origin.is_none().then(|| "com.speakmore.desktop".into())),
            app_name: origin.is_none().then(|| "SpeakMore".into()),
            ..Default::default()
        };
        self.writing_context = if frozen.settings.polish_enabled {
            Some(context.clone())
        } else {
            None
        };
        self.writing_rule = speakmore_core::writing::matching(&frozen.writing_rules, &context)
            .map(|r| r.display_name.clone());
        if frozen.settings.polish_enabled {
            if let Some(press) = origin.as_ref().cloned() {
                let sender = self.completed_tx.clone();
                let native = self.native.clone();
                tokio::spawn(async move {
                    let context = tokio::task::spawn_blocking(move || {
                        crate::target::writing_context(
                            native,
                            press.target_generation,
                            press.app_identity,
                            press.at,
                        )
                    })
                    .await
                    .ok()
                    .flatten();
                    let _ = sender.send(Completed::WritingContext { id, context }).await;
                });
            }
        }
        self.current = Some(Current {
            writing_context: context,
            polish_access: self.polisher.clone(),
            polish_watch: frozen.processing.watch(crate::providers::session::is_cloud(
                frozen.provider.as_deref(),
            )),
            polish_job: None,
            polish_result: None,
            finalizing_result: false,
            generated_record: None,
            storage_ready: false,
            app_identity,
            app_policy,
            target,
            target_generation: origin.map_or(0, |v| v.target_generation),
            delivery: None,
            delivery_intent: false,
            target_check: None,
            manual_reason,
            id,
            frozen: frozen.clone(),
            model_id: lease.bundle.model.installation_id(),
            model_sha: model.sha256.clone(),
            capture: None,
            awaiting_worker: false,
            persisting: false,
            duration_ms: 0,
            control_revision,
        });
        self.last_error = None;
        let native = self.native.clone();
        let capture = native.while_current(epoch, control_revision, || {
            self.source.start(
                id,
                speakmore_audio::capture::Selection {
                    device_id: frozen.settings.input_device_id,
                    ..Default::default()
                },
            )
        });
        match capture {
            Some(Ok(handle)) => self.current.as_mut().unwrap().capture = Some(handle),
            Some(Err(error)) => {
                self.session_error(id, super::super::audio_error(error));
                self.finish_session();
            }
            None => {
                self.session_error(id, super::interrupted());
                self.finish_session();
                return Err(super::interrupted());
            }
        }
        self.dirty = true;
        Ok(id)
    }
    fn stop(&mut self, id: Uuid, cancel: bool) {
        let changed = if cancel {
            self.kernel.cancel(id)
        } else {
            self.kernel.stop(id, self.now_ms())
        };
        if !changed {
            return;
        }
        if cancel || self.kernel.snapshot(self.now_ms()).phase == Phase::Finishing {
            if let Some(current) = self.current.as_mut().filter(|c| c.id == id) {
                if let Some(job) = &current.polish_job {
                    job.cancel.cancel();
                }
                if let Some(attempt) = &current.delivery {
                    attempt.cancel();
                }
                if let Some(target) = current.target.take() {
                    target.end();
                }
            }
        }
        if let Some(current) = self.current.as_ref().filter(|c| c.id == id) {
            let phase = self.kernel.snapshot(self.now_ms()).phase;
            if let Some(capture) = &current.capture {
                if cancel || phase == Phase::Finishing {
                    capture.cancel();
                } else {
                    capture.stop();
                }
            }
            if current.awaiting_worker {
                self.worker.cancel(id);
            }
        }
        self.dirty = true;
        self.finish_session();
    }
    fn session_error(&mut self, id: Uuid, error: AppError) {
        self.last_error = Some(error);
        self.stop(id, true);
        self.dirty = true;
    }
    fn finish_session(&mut self) {
        let Some(current) = self.current.as_ref() else {
            return;
        };
        if current.capture.is_some()
            || current.awaiting_worker
            || current.persisting
            || current.finalizing_result
            || current.polish_job.is_some()
            || current.target_check.is_some()
            || current.delivery.is_some()
            || self.source.busy()
        {
            return;
        }
        let id = current.id;
        if self.kernel.snapshot(self.now_ms()).phase != Phase::Finishing {
            self.kernel.advance(id, Phase::Finishing);
        }
        if self.kernel.finish(id) {
            self.current = None;
            // The input was just in use; a long session must not leave a
            // stale-check gap that rejects the next press.
            if let Some((_, Ok(()), at)) = self.input_check.as_mut() {
                *at = Instant::now();
            }
            self.dirty = true;
        }
    }
    fn audio_event(&mut self, event: speakmore_audio::capture::CaptureEvent) {
        self.check_permissions();
        self.observe_lifecycle();
        use speakmore_audio::capture::CaptureEvent;
        match event {
            CaptureEvent::Ready {
                session_id,
                started_at,
                ..
            } => {
                // An opened capture proves the selected input during a session.
                if let Some(current) = self.current.as_ref().filter(|c| c.id == session_id) {
                    let selected = current.frozen.settings.input_device_id.clone();
                    self.input_check = Some((selected, Ok(()), Instant::now()));
                }
                if self.current.as_ref().is_some_and(|c| c.id == session_id)
                    && !self.kernel.mic_ready(
                        session_id,
                        started_at
                            .saturating_duration_since(self.origin)
                            .as_millis() as u64,
                    )
                {
                    if let Some(handle) = self.current.as_ref().and_then(|c| c.capture.as_ref()) {
                        handle.cancel();
                    }
                }
            }
            CaptureEvent::Finished {
                session_id, result, ..
            } => {
                if self.current.as_ref().is_none_or(|c| c.id != session_id) {
                    return;
                }
                self.current.as_mut().unwrap().capture.take();
                let phase = self.kernel.snapshot(self.now_ms()).phase;
                if matches!(phase, Phase::Cancelling | Phase::Finishing) {
                    self.finish_session();
                    return;
                }
                match result {
                    Ok(audio) => {
                        if phase == Phase::Recording {
                            self.kernel.stop(session_id, self.now_ms());
                        }
                        if !self.kernel.advance(session_id, Phase::Transcribing) {
                            self.session_error(session_id, super::unavailable());
                        } else {
                            let current = self.current.as_mut().unwrap();
                            current.duration_ms = audio.duration_ms;
                            let request = Transcription {
                                session_id,
                                language: current.frozen.settings.language.clone(),
                                dictionary_revision: current.frozen.settings.revision,
                                dictionary_terms: current.frozen.terms.clone(),
                                samples: audio.samples,
                            };
                            if request.samples.len() < 4000 {
                                self.session_error(
                                    session_id,
                                    super::super::audio_error(
                                        speakmore_audio::error::AudioError::TooShort,
                                    ),
                                );
                            } else {
                                match self.worker.transcribe(request) {
                                    Ok(()) => self.current.as_mut().unwrap().awaiting_worker = true,
                                    Err(error) => {
                                        self.session_error(session_id, worker_error(error))
                                    }
                                }
                            }
                        }
                    }
                    Err(error) => {
                        if !matches!(
                            error,
                            speakmore_audio::error::AudioError::Cancelled
                                | speakmore_audio::error::AudioError::TooShort
                        ) {
                            self.input_check = None;
                        }
                        self.session_error(session_id, super::super::audio_error(error))
                    }
                }
                self.finish_session();
            }
        }
        self.dirty = true;
    }
    fn worker_event(&mut self, event: Event) {
        self.check_permissions();
        self.observe_lifecycle();
        match event {
            Event::ModelReady { model_id, .. } => {
                if self.shutting_down || self.capture_suspended() {
                    return;
                }
                self.model_ready(&model_id);
            }
            Event::ModelFailed { error, .. } => {
                if let Some(work) = self.model_work.as_mut() {
                    work.failure = Some(worker_error(error));
                } else {
                    self.last_error = Some(worker_error(error));
                }
            }
            Event::Completed(result) => {
                let Some(current) = self.current.as_mut().filter(|c| c.id == result.session_id)
                else {
                    return;
                };
                current.awaiting_worker = false;
                if self.kernel.snapshot(self.now_ms()).phase != Phase::Transcribing {
                    self.finish_session();
                    return;
                }
                // Only admission is linearized against interruption. Acceptance
                // queues work and must not run while NativeInput is locked.
                let admitted = self
                    .native
                    .while_current(
                        self.lifecycle.epoch,
                        self.current.as_ref().unwrap().control_revision,
                        || (),
                    )
                    .is_some();
                if admitted {
                    self.generated(result);
                } else {
                    self.observe_lifecycle();
                    self.finish_session();
                }
            }
            Event::Failed { session_id, error } => {
                if let Some(current) = self.current.as_mut().filter(|c| c.id == session_id) {
                    current.awaiting_worker = false;
                    self.session_error(session_id, worker_error(error));
                }
                self.finish_session();
            }
            Event::Cancelled { session_id } => {
                if let Some(current) = self.current.as_mut().filter(|c| c.id == session_id) {
                    current.awaiting_worker = false;
                    self.kernel.cancel(session_id);
                }
                self.finish_session();
            }
            Event::InputAccepted { .. } | Event::Progress { .. } => {}
        }
        self.dirty = true;
    }
    fn loaded_id(&self) -> Option<String> {
        self.loaded
            .as_ref()
            .map(|lease| lease.bundle.model.installation_id())
    }
    fn model_ready(&mut self, model_id: &str) {
        let Some(work) = self.model_work.as_ref() else {
            return;
        };
        if work.stage == Stage::Loading && work.target.as_deref() == Some(model_id) {
            self.commit_model();
        } else if matches!(work.stage, Stage::Loading | Stage::Restoring)
            && self.loaded_id().as_deref() == Some(model_id)
        {
            // The supervisor rolls back to the model it actually ran, which
            // is the loaded lease, not necessarily the persisted selection.
            let error = work.failure.clone().unwrap_or_else(|| {
                AppError::new("ModelLoadFailed", "模型加载失败，已恢复之前的模型。", true)
            });
            self.finish_model(Err(error));
        }
    }
    fn generated(&mut self, result: speakmore_worker_supervisor::Completed) {
        let current = self.current.as_mut().unwrap();
        current.persisting = true;
        current.finalizing_result = true;
        self.kernel.advance(
            current.id,
            if current.frozen.settings.polish_enabled {
                Phase::Polishing
            } else {
                Phase::DeliveryCheck
            },
        );
        let delivery_reason = Some(current.manual_reason);
        let id = current.id;
        let record = ResultRecord {
            needs_review: result
                .quality_flags
                .contains(&speakmore_protocol::QualityFlag::NeedsReview),
            session_id: id,
            raw_text: result.text,
            polished_text: None,
            model_id: current.model_id.clone(),
            model_sha256: current.model_sha.clone(),
            dictionary_revision: current.frozen.settings.revision,
            language: result.language,
            duration_ms: current.duration_ms,
            asr_ms: result.asr_ms,
            polish_ms: None,
            polish_outcome: "off".into(),
            polish_reason: None,
            delivery_status: "ready_for_copy".into(),
            delivery_reason,
            app_identity: current.app_identity.clone(),
            created_at: super::super::now(),
        };
        current.generated_record = Some(record.clone());
        if current.frozen.settings.polish_enabled {
            if let Some(access) = current.polish_access.clone() {
                let input = crate::providers::session::Input {
                    settings: current.frozen.settings.clone(),
                    record: current.frozen.provider.clone(),
                    watch: current.polish_watch.clone(),
                    raw: record.raw_text.clone(),
                    writing_context: current.writing_context.clone(),
                    writing_prompt: speakmore_core::writing::matching(
                        &current.frozen.writing_rules,
                        &current.writing_context,
                    )
                    .map(|r| r.prompt.clone()),
                };
                let cancel = tokio_util::sync::CancellationToken::new();
                let task_cancel = cancel.clone();
                let sender = self.completed_tx.clone();
                let task = tokio::spawn(async move {
                    let result = access.run(input, task_cancel).await;
                    let _ = sender.send(Completed::Polish { id, result }).await;
                });
                current.polish_job = Some(PolishJob { cancel, task });
            } else {
                current.polish_result = Some(crate::providers::session::Resolution::original(
                    speakmore_core::processing::Reason::ProviderUnavailable,
                    true,
                ));
            }
        }
        let history_at_start = current.frozen.settings.history_enabled;
        let history_guard = current.frozen.history_guard;
        let app_policy = current.app_policy.clone();
        let quota_day = current.frozen.quota_day.clone();
        self.results.push_front(MemoryResult {
            record: record.clone(),
            expires: Instant::now() + Duration::from_secs(900),
        });
        self.results.truncate(10);
        self.queue_db(Box::new(move |store| {
            let result = store.and_then(|store| {
                let history_at_start = history_at_start && store.history_guard() == history_guard;
                let excluded = app_policy.history_excluded();
                let history_expected =
                    history_at_start && store.settings()?.history_enabled && !excluded;
                match store.persist_generated(&record, &quota_day, history_at_start, excluded) {
                    Ok(_) => Ok(None),
                    Err(_) => {
                        // Preserve a valid generated result independently of history/cleanup.
                        // A committed usage event is never counted a second time.
                        if !store.usage_recorded(id)? {
                            let _ = store.persist_generated(&record, &quota_day, false, false);
                        }
                        if !store.usage_recorded(id)? {
                            return Err(db::unavailable());
                        }
                        Ok(Some(AppError::new(
                            if history_expected {
                                "HistoryNotSaved"
                            } else {
                                "LocalCleanupIncomplete"
                            },
                            if history_expected {
                                "结果未保存到历史，仍可从最近结果复制。"
                            } else {
                                "本地清理未完成，结果仍可复制。"
                            },
                            true,
                        )))
                    }
                }
            });
            Box::new(move |actor| {
                let allow_delivery = result.as_ref().is_ok_and(|warning| warning.is_none());
                match result {
                    Ok(warning) => actor.storage_warning = warning,
                    Err(error) => {
                        actor.storage_warning = Some(AppError::new(
                            "UsageNotSaved",
                            "本地使用记录未保存，文字仍可从最近结果复制。",
                            true,
                        ));
                        actor.last_error = Some(error);
                    }
                }
                if let Some(current) = actor.current.as_mut().filter(|c| c.id == id) {
                    current.persisting = false;
                    current.storage_ready = allow_delivery;
                }
                actor.complete_generation();
            })
        }));
    }
    fn warn_storage(&mut self, warning: AppError) {
        // Later receipt/refinement failures must not hide the usage failure
        // that explains incomplete local statistics. Text remains in memory.
        if self
            .storage_warning
            .as_ref()
            .is_none_or(|current| current.code != "UsageNotSaved")
        {
            self.storage_warning = Some(warning);
        }
    }
    fn complete_generation(&mut self) {
        use speakmore_core::processing::Reason;
        let cancelled =
            self.kernel.snapshot(self.now_ms()).phase == Phase::Cancelling || self.shutting_down;
        let native = self.native.clone();
        let Some(current) = self
            .current
            .as_mut()
            .filter(|c| c.finalizing_result && !c.persisting && c.polish_job.is_none())
        else {
            return;
        };
        current.finalizing_result = false;
        let id = current.id;
        if let Some(mut resolution) = current.polish_result.take() {
            if cancelled {
                let elapsed = resolution.elapsed_ms;
                resolution =
                    crate::providers::session::Resolution::original(Reason::Cancelled, false);
                resolution.elapsed_ms = elapsed;
            } else if current.polish_watch.revoked() {
                let elapsed = resolution.elapsed_ms;
                resolution =
                    crate::providers::session::Resolution::original(Reason::PolicyRevoked, false);
                resolution.elapsed_ms = elapsed;
            }
            let record = current.generated_record.as_mut().unwrap();
            let elapsed = resolution.elapsed_ms;
            if native
                .accept_refinement(id, self.lifecycle.epoch, current.control_revision, || {
                    resolution.apply(record)
                })
                .is_none()
            {
                let mut fallback =
                    crate::providers::session::Resolution::original(Reason::Cancelled, false);
                fallback.elapsed_ms = elapsed;
                fallback.apply(record);
                self.kernel.cancel(id);
            }
            let record = record.clone();
            if let Some(memory) = self.results.iter_mut().find(|r| r.record.session_id == id) {
                memory.record = record.clone();
            }
            current.persisting = true;
            let guard = current.frozen.history_guard;
            let policy = current.app_policy.clone();
            self.queue_db_quiet(Box::new(move |store| {
                let result = store
                    .and_then(|s| s.transition_polish(&record, guard, policy.history_excluded()));
                Box::new(move |actor| {
                    if let Some(current) = actor.current.as_mut().filter(|c| c.id == id) {
                        current.persisting = false;
                        current.storage_ready &= result.is_ok();
                    }
                    if result.is_err() {
                        actor.warn_storage(AppError::new(
                            "RefinementNotSaved",
                            "整理结果未保存到历史，仍可从最近结果复制。",
                            true,
                        ));
                    }
                    actor.check_result_target();
                })
            }));
        } else {
            self.check_result_target();
        }
        self.dirty = true;
    }
    fn check_result_target(&mut self) {
        use speakmore_core::delivery::ManualReason;
        if self.kernel.snapshot(self.now_ms()).phase == Phase::Cancelling || self.shutting_down {
            self.finish_session();
            return;
        }
        let Some(current) = self.current.as_mut() else {
            return;
        };
        self.kernel.advance(current.id, Phase::DeliveryCheck);
        let Some(record) = current.generated_record.as_ref() else {
            return;
        };
        let reason = if current.app_policy.manual_only() {
            ManualReason::UserRequested
        } else if record
            .polished_text
            .as_ref()
            .unwrap_or(&record.raw_text)
            .chars()
            .count()
            > speakmore_core::MAX_AUTO_PASTE_CHARS
        {
            ManualReason::TextTooLong
        } else if let Some(target) = current.target.as_ref() {
            match target.recheck() {
                Ok(check) => {
                    current.target_check = Some(check);
                    self.dirty = true;
                    return;
                }
                Err(reason) => reason,
            }
        } else {
            current.manual_reason
        };
        self.result_target_checked(Some(reason));
    }
    fn poll_target_check(&mut self) {
        let Some(current) = self.current.as_mut() else {
            return;
        };
        let Some(answer) = current.target_check.as_ref().and_then(|check| check.poll()) else {
            return;
        };
        current.target_check.take();
        let reason = match answer {
            Ok(()) => current.target.as_ref().map_or(
                Some(speakmore_core::delivery::ManualReason::TargetUnavailable),
                |target| target.qualified().err(),
            ),
            Err(reason) => Some(reason),
        };
        self.result_target_checked(reason);
    }
    fn result_target_checked(&mut self, reason: Option<speakmore_core::delivery::ManualReason>) {
        use speakmore_core::delivery::{ManualReason, Status};
        let Some(current) = self.current.as_mut() else {
            return;
        };
        let reason = if current.app_policy.manual_only() {
            Some(ManualReason::UserRequested)
        } else {
            reason
        };
        if reason.is_some() {
            current.target.take();
        }
        let id = current.id;
        current.persisting = true;
        if let Some(record) = self.results.iter_mut().find(|r| r.record.session_id == id) {
            record.record.delivery_reason = reason;
        }
        self.queue_db_quiet(Box::new(move |store| {
            let saved = store.and_then(|s| {
                s.transition_delivery(id, Status::ReadyForCopy, Status::ReadyForCopy, reason)
            });
            Box::new(move |actor| {
                let mut automatic = false;
                if let Some(current) = actor.current.as_mut().filter(|c| c.id == id) {
                    current.persisting = false;
                    automatic = reason.is_none() && current.storage_ready && saved.is_ok();
                }
                if saved.is_err() {
                    actor.warn_storage(AppError::new(
                        "DeliveryStatusNotSaved",
                        "交付状态未保存，文字仍可从最近结果复制。",
                        true,
                    ));
                }
                if automatic {
                    actor.prepare_delivery(id);
                }
                actor.finish_session();
            })
        }));
        self.dirty = true;
    }
    fn prepare_delivery(&mut self, id: Uuid) {
        use speakmore_core::delivery::Status;
        if self.shutting_down
            || self.store_error.is_some()
            || self.kernel.snapshot(self.now_ms()).phase != Phase::DeliveryCheck
        {
            return;
        }
        let Some(current) = self
            .current
            .as_mut()
            .filter(|c| c.id == id && c.delivery.is_none())
        else {
            return;
        };
        let Some(target) = current.target.clone() else {
            return;
        };
        let invalid = target.qualified().err();
        let attempt = crate::delivery::Attempt::new(
            id,
            target,
            current.app_policy.clone(),
            self.native.clone(),
            self.lifecycle.epoch,
            current.control_revision,
            current.target_generation,
        );
        current.delivery = Some(attempt.clone());
        if let Some(reason) = invalid {
            self.finish_delivery(id, crate::delivery::Outcome::ready(Some(reason), None));
            return;
        }
        current.persisting = true;
        self.queue_db_quiet(Box::new(move |store| {
            let saved = store.and_then(|s| {
                s.transition_delivery(id, Status::ReadyForCopy, Status::Uncertain, None)
            });
            Box::new(move |actor| {
                if matches!(saved, Ok(true)) {
                    (actor.notify_delivery)(crate::delivery::DeliveryUpdate {
                        session_id: id,
                        delivery_status: Status::Uncertain,
                        delivery_reason: None,
                    });
                }
                let Some(current) = actor
                    .current
                    .as_mut()
                    .filter(|c| c.id == id && c.delivery.is_some())
                else {
                    return;
                };
                current.persisting = false;
                current.delivery_intent = saved.is_ok();
                if let Err(error) = saved {
                    actor.finish_delivery(id, crate::delivery::Outcome::ready(None, Some(error)));
                    return;
                }
                if !attempt.live() || actor.shutting_down || actor.store_error.is_some() {
                    actor.finish_delivery(
                        id,
                        crate::delivery::Outcome::ready(attempt.target.qualified().err(), None),
                    );
                    return;
                }
                // Recent results can be cleared meanwhile; the session keeps its own copy.
                let Some(text) = actor
                    .current
                    .as_ref()
                    .filter(|c| c.id == id)
                    .and_then(|c| c.generated_record.as_ref())
                    .map(|r| {
                        r.polished_text
                            .clone()
                            .unwrap_or_else(|| r.raw_text.clone())
                    })
                else {
                    actor.finish_delivery(id, crate::delivery::Outcome::ready(None, None));
                    return;
                };
                let reservation = actor
                    .clipboard
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|c| c.reserve());
                match reservation {
                    Ok(reservation) => {
                        if let Some(result) =
                            actor.results.iter_mut().find(|r| r.record.session_id == id)
                        {
                            result.record.delivery_status = Status::Uncertain.as_str().into();
                        }
                        tokio::spawn(crate::delivery::run(
                            reservation,
                            text,
                            attempt,
                            actor.delivery_tx.clone(),
                        ));
                    }
                    Err(error) => actor
                        .finish_delivery(id, crate::delivery::Outcome::ready(None, Some(error))),
                }
                actor.dirty = true;
            })
        }));
    }
    fn delivery_events(&mut self) {
        for _ in 0..2 {
            let Ok(event) = self.delivery_rx.try_recv() else {
                break;
            };
            match event {
                crate::delivery::Event::Claim { session, reply } => {
                    let live = !reply.is_closed()
                        && self.current.as_ref().is_some_and(|c| {
                            c.id == session
                                && c.delivery_intent
                                && c.delivery.as_ref().is_some_and(|a| a.live())
                        });
                    let claimed = live && self.kernel.claim_submission(session, true);
                    let _ = reply.send(claimed);
                    self.dirty |= claimed;
                }
                crate::delivery::Event::Finished { session, outcome } => {
                    self.finish_delivery(session, outcome)
                }
            }
        }
    }
    fn finish_delivery(&mut self, id: Uuid, outcome: crate::delivery::Outcome) {
        use speakmore_core::delivery::Status;
        let Some(current) = self
            .current
            .as_mut()
            .filter(|c| c.id == id && c.delivery.is_some())
        else {
            return;
        };
        current.delivery.take().unwrap().cancel();
        if let Some(target) = current.target.take() {
            target.end();
        }
        let expected = if current.delivery_intent {
            Status::Uncertain
        } else {
            Status::ReadyForCopy
        };
        current.persisting = true;
        if let Some(result) = self.results.iter_mut().find(|r| r.record.session_id == id) {
            result.record.delivery_status = outcome.status.as_str().into();
            result.record.delivery_reason = outcome.reason;
        }
        if let Some(error) = outcome.error {
            self.last_error = Some(error);
        }
        self.queue_db_quiet(Box::new(move |store| {
            let saved = store
                .and_then(|s| s.transition_delivery(id, expected, outcome.status, outcome.reason));
            Box::new(move |actor| {
                if matches!(saved, Ok(true)) {
                    (actor.notify_delivery)(crate::delivery::DeliveryUpdate {
                        session_id: id,
                        delivery_status: outcome.status,
                        delivery_reason: outcome.reason,
                    });
                }
                if saved.is_err() {
                    actor.warn_storage(AppError::new(
                        "DeliveryReceiptNotSaved",
                        "交付状态未保存，已保留本次结果；请检查目标应用。",
                        true,
                    ));
                }
                if let Some(current) = actor.current.as_mut().filter(|c| c.id == id) {
                    current.persisting = false;
                }
                actor.finish_session();
            })
        }));
        self.dirty = true;
    }
    fn expire_results(&mut self) {
        let before = self.results.len();
        self.results.retain(|r| r.expires > Instant::now());
        if before != self.results.len() {
            self.dirty = true;
        }
    }
    fn settings_operation_busy(&self) -> bool {
        self.settings_operation
            .as_ref()
            .is_some_and(|lease| lease.strong_count() > 0)
    }
    fn capture_suspended(&self) -> bool {
        self.lifecycle.blocked()
            || self.retiring_for_interrupt
            || self.native.lifecycle() != self.lifecycle
    }
    fn observe_lifecycle(&mut self) {
        self.apply_lifecycle(self.native.lifecycle());
    }
    fn apply_lifecycle(&mut self, lifecycle: super::native_input::Lifecycle) {
        let interrupted = lifecycle.epoch != self.lifecycle.epoch;
        let teardown = lifecycle.teardown_after(self.lifecycle.epoch);
        if lifecycle != self.lifecycle {
            self.lifecycle = lifecycle;
            self.dirty = true;
        }
        if !interrupted {
            return;
        }
        self.retiring_for_interrupt = true;
        // Display sleep only ends input; the loaded worker and lease survive.
        // Model work in flight is always reset by the full path.
        self.interrupt_teardown |= teardown || self.model_work.is_some();
        self.native_session = None;
        self.observed_press = None;
        self.kernel.key_released();
        if let Some(id) = self.current.as_ref().map(|c| c.id) {
            // Text generated before the interruption retains its single usage
            // transaction. An unfinished capture/inference cannot create text.
            if !self.current.as_ref().unwrap().persisting {
                self.last_error = Some(super::interrupted());
            }
            self.stop(id, true);
        }
        // shutdown suppresses the supervisor's usual automatic crash recovery.
        // Do not release either model lease before the child/pipes are reaped.
        if self.interrupt_teardown {
            self.worker.shutdown();
        }
    }
    fn finish_interruption(&mut self) {
        if !self.retiring_for_interrupt
            || self.source.busy()
            || self
                .current
                .as_ref()
                .is_some_and(|c| c.capture.is_some() || c.awaiting_worker)
            || (self.interrupt_teardown && self.worker.snapshot().phase != WorkerPhase::Stopped)
        {
            return;
        }
        self.native.acknowledge_quiesced(self.lifecycle.epoch);
        self.retiring_for_interrupt = false;
        self.dirty = true;
        if !std::mem::take(&mut self.interrupt_teardown) {
            return;
        }
        if let Some(mut work) = self.model_work.take() {
            if let Some(reply) = work.reply.take() {
                self.respond(reply, Err(super::interrupted()));
            }
        }
        self.loaded.take();
        self.initial_model_attempted = false;
        // The normal initial-selection path waits for all pending database work,
        // obtains a new verified lease and requires actual Ready after resume.
    }
    fn native_tick(&mut self) {
        let super::native_input::InputSnapshot {
            press,
            toggle,
            stop,
            lifecycle,
            require_release,
            editing,
        } = self.native.take();
        if self.shortcut_editing != editing {
            self.shortcut_editing = editing;
            self.dirty = true;
        }
        if self.shortcut_needs_release != require_release {
            self.shortcut_needs_release = require_release;
            self.dirty = true;
        }
        self.apply_lifecycle(lifecycle);
        if let Some((id, cancel)) = stop {
            self.stop(id, cancel);
        }
        let mut toggle = toggle;
        if toggle.is_some() && self.settings_operation_busy() {
            if let Some((cancel, yielded)) = &self.background_interrupt {
                yielded.store(true, Ordering::Release);
                cancel.store(true, Ordering::Release);
                self.deferred_toggle = toggle.take();
            }
        }
        if !self.settings_operation_busy() && toggle.is_none() {
            toggle = self.deferred_toggle.take().and_then(|(old, multiple)| {
                if old.at.elapsed() >= Duration::from_secs(1)
                    || old.target_generation != self.native.target_generation()
                {
                    return None;
                }
                #[cfg(target_os = "macos")]
                {
                    let mut fresh = self.native.native_origin(false);
                    fresh.cycle = old.cycle;
                    Some((fresh, multiple))
                }
                #[cfg(not(target_os = "macos"))]
                {
                    Some((old, multiple))
                }
            });
        }
        if self
            .deferred_toggle
            .as_ref()
            .is_some_and(|(p, _)| p.at.elapsed() >= Duration::from_secs(1))
        {
            self.deferred_toggle = None;
            self.last_error = Some(AppError::new(
                "BackgroundYield",
                "后台任务已暂停，请再按一次快捷键开始听写。",
                true,
            ));
            self.dirty = true;
        }
        if let Some((origin, multiple)) =
            toggle.filter(|(p, _)| p.at.elapsed() < Duration::from_secs(1))
        {
            if !self.shutting_down
                && !self.stopping.load(Ordering::Acquire)
                && !lifecycle.blocked()
                && !editing
                && origin.epoch == lifecycle.epoch
                && origin.control_revision == self.native.control_revision()
            {
                let session = self.kernel.snapshot(self.now_ms());
                if let Some(id) = session.session_id {
                    if matches!(session.phase, Phase::Preparing | Phase::Recording) {
                        self.stop(id, false);
                    }
                } else if !multiple {
                    if let Err(error) = self.start(
                        origin.cycle,
                        StartMode::Toggle,
                        origin.epoch,
                        origin.control_revision,
                        Some(origin),
                    ) {
                        self.last_error = Some(error);
                        self.dirty = true;
                    }
                }
            }
        }
        let cycle = press.as_ref().map(|p| p.cycle);
        if cycle == self.observed_press {
            return;
        }
        // A release (including release+press between polls) belongs to the old
        // native session only. It cannot stop a newer click-to-record session.
        if let Some(id) = self.native_session.take() {
            self.stop(id, false);
        }
        self.kernel.key_released();
        self.observed_press = cycle;
        if !self.shutting_down && !self.stopping.load(Ordering::Acquire) {
            if let Some(press) = press.filter(|p| p.at.elapsed() < Duration::from_secs(1)) {
                match self.start(
                    press.cycle,
                    StartMode::Hold,
                    lifecycle.epoch,
                    press.control_revision,
                    Some(press),
                ) {
                    Ok(id) => self.native_session = Some(id),
                    Err(error) => {
                        self.last_error = Some(error);
                        self.dirty = true;
                    }
                }
            }
        }
    }
    fn check_target_policy(&self) {
        if self
            .view
            .as_ref()
            .is_some_and(|view| view.settings.delivery_mode == "manual")
            || self
                .current
                .as_ref()
                .is_some_and(|c| c.app_policy.manual_only())
        {
            if let Some(target) = self.current.as_ref().and_then(|c| c.target.as_ref()) {
                target.invalidate(speakmore_core::delivery::ManualReason::UserRequested);
            }
        }
    }
    fn tick(&mut self) {
        if self.settings_operation.is_some() && !self.settings_operation_busy() {
            self.settings_operation = None;
            self.dirty = true;
        }
        self.last_tick = Instant::now();
        if self.last_permission_check.elapsed() >= Duration::from_millis(250) {
            self.check_permissions();
        }
        self.check_input();
        self.check_target_policy();
        self.native_tick();
        self.delivery_events();
        self.poll_target_check();
        if self.kernel.tick(self.now_ms()) {
            if let Some(handle) = self.current.as_ref().and_then(|c| c.capture.as_ref()) {
                handle.stop();
            }
            self.dirty = true;
        }
        for _ in 0..4 {
            let event = self
                .current
                .as_ref()
                .and_then(|c| c.capture.as_ref())
                .map(|h| h.try_recv());
            match event {
                Some(Ok(event)) => self.audio_event(event),
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                    if let Some(current) = self.current.as_mut() {
                        current.capture.take();
                        let id = current.id;
                        self.session_error(id, super::unavailable());
                    }
                    break;
                }
                _ => break,
            }
        }
        let before = self.worker.snapshot();
        for event in self.worker.poll() {
            self.worker_event(event);
        }
        let after = self.worker.snapshot();
        if before.phase != after.phase
            || before.backend != after.backend
            || before.model_id != after.model_id
        {
            self.dirty = true;
        }
        if let Some(work) = self
            .model_work
            .as_ref()
            .filter(|_| !self.shutting_down && !self.capture_suspended())
        {
            match (work.stage, after.phase) {
                (Stage::Unloading, WorkerPhase::Stopped) => self.commit_model(),
                (
                    Stage::Loading | Stage::Restoring,
                    WorkerPhase::Unavailable | WorkerPhase::Paused,
                ) => {
                    let work = self.model_work.as_mut().unwrap();
                    work.stage = Stage::Retiring;
                    work.failure.get_or_insert_with(|| {
                        AppError::new("ModelLoadFailed", "模型未能加载，请重试。", true)
                    });
                    self.worker.shutdown();
                }
                (Stage::Retiring, WorkerPhase::Stopped) => {
                    self.worker = Supervisor::new(self.config.clone()).unwrap();
                    let loaded = self.loaded_id();
                    let work = self.model_work.as_mut().unwrap();
                    if !work.restore_attempted && loaded.is_some() && work.target != loaded {
                        work.restore_attempted = true;
                        work.stage = Stage::Restoring;
                        if let Err(error) = spec(self.loaded.as_ref().unwrap())
                            .and_then(|spec| self.worker.set_model(spec).map_err(worker_error))
                        {
                            self.loaded = None;
                            self.finish_model(Err(error));
                        }
                    } else {
                        // The reaped worker held nothing; keep no stale lease.
                        let error = work.failure.clone().unwrap_or_else(super::unavailable);
                        self.loaded = None;
                        self.finish_model(Err(error));
                    }
                }
                // ModelReady is never sent when the requested spec was already
                // running, and a rollback may land on neither expected model.
                (Stage::Loading | Stage::Restoring, WorkerPhase::Ready) => {
                    if let Some(model_id) = after.model_id.as_deref() {
                        self.model_ready(model_id);
                    }
                    if let Some(work) = self
                        .model_work
                        .as_mut()
                        .filter(|w| matches!(w.stage, Stage::Loading | Stage::Restoring))
                    {
                        work.stage = Stage::Retiring;
                        work.failure.get_or_insert_with(|| {
                            AppError::new("ModelLoadFailed", "模型未能加载，请重试。", true)
                        });
                        self.worker.shutdown();
                    }
                }
                _ => {}
            }
        }
        self.finish_session();
        self.finish_interruption();
        self.expire_results();
        if let Ok(manager) = &self.models {
            let state = manager.snapshot();
            if state.revision != self.last_models_revision {
                self.last_models_revision = state.revision;
                if !state.busy && !self.shutting_down {
                    self.refresh_db();
                }
                self.dirty = true;
            }
            if !state.busy
                && self.view.is_some()
                && self.db_pending == 0
                && !self.initial_model_attempted
                && !self.shutting_down
                && !self.capture_suspended()
                && self.model_work.is_none()
                && self.config_operation.is_none()
                && !self.settings_operation_busy()
                && self.current.is_none()
                && !self.source.busy()
            {
                self.initial_model_attempted = true;
                if let Some(id) = self
                    .view
                    .as_ref()
                    .and_then(|v| v.settings.active_model_id.clone())
                {
                    self.select(Some(id), None, false);
                }
            }
        }
        // Day rollover needs no 50 Hz formatting; back off while storage fails.
        if Instant::now() >= self.day_check && self.db_pending == 0 && !self.shutting_down {
            self.day_check = Instant::now()
                + Duration::from_secs(if self.store_error.is_some() { 10 } else { 1 });
            if self
                .view
                .as_ref()
                .is_some_and(|v| v.quota_day != chrono::Local::now().format("%Y-%m-%d").to_string())
            {
                self.refresh_db();
            }
        }
        if self.current.is_some() && self.last_publish.elapsed() >= Duration::from_millis(200) {
            self.dirty = true;
        }
    }
    fn check_permissions(&mut self) {
        self.last_permission_check = Instant::now();
        let next = self.source.permissions();
        if next == self.permissions {
            return;
        }
        if !next.accessibility {
            if let Some(target) = self.current.as_ref().and_then(|c| c.target.as_ref()) {
                target.invalidate(speakmore_core::delivery::ManualReason::AccessibilityRequired);
            }
        }
        if next.microphone != self.permissions.microphone {
            let revoked = self.permissions.microphone == crate::permissions::Microphone::Authorized
                && next.microphone != crate::permissions::Microphone::Authorized;
            let cancel_active = revoked && self.current.as_ref().is_some_and(|c| !c.persisting);
            self.native.permissions_changed(cancel_active);
            self.input_check = None;
            if cancel_active {
                self.observe_lifecycle();
                self.last_error = Some(AppError::new(
                    "MicrophonePermissionRevoked",
                    "麦克风权限已变化，本次听写已取消。请检查系统设置。",
                    true,
                ));
            }
        }
        self.permissions = next;
        self.dirty = true;
    }
    fn check_input(&mut self) {
        if self.current.is_none()
            && self
                .input_check
                .as_ref()
                .is_some_and(|(_, _, at)| at.elapsed() >= INPUT_CHECK_TTL)
        {
            self.input_check = None;
            self.dirty = true;
        }
        if self.shutting_down
            || self.capture_suspended()
            || self.current.is_some()
            || self.input_check_pending
            || self.permissions.microphone != crate::permissions::Microphone::Authorized
        {
            return;
        }
        let Some(view) = &self.view else {
            return;
        };
        let selected = view.settings.input_device_id.clone();
        if self
            .input_check
            .as_ref()
            .is_some_and(|(id, _, at)| *id == selected && at.elapsed() < Duration::from_secs(5))
        {
            return;
        }
        self.input_check_pending = true;
        let probe = self.source.device_probe(selected.clone());
        let sender = self.completed_tx.clone();
        // Exactly one device query may be in flight. Slow CoreAudio enumeration
        // cannot block capture cancellation or accumulate background queries.
        let failed_selected = selected.clone();
        if std::thread::Builder::new()
            .name("speakmore-device-check".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(probe))
                    .unwrap_or(Err(speakmore_audio::error::AudioError::ThreadUnavailable));
                let _ = sender.blocking_send(Completed::InputCheck { selected, result });
            })
            .is_err()
        {
            self.input_check_pending = false;
            self.input_check = Some((
                failed_selected,
                Err(speakmore_audio::error::AudioError::ThreadUnavailable),
                Instant::now(),
            ));
            self.dirty = true;
        }
    }
    fn readiness(&self) -> Option<Blocker> {
        use ReadinessCode::*;
        let block = |code, message: &str| Some(Blocker::new(code, message));
        if self.native.editing() {
            return block(ShortcutEditing, "正在设置快捷键，完成后可继续听写。");
        }
        if self.shutting_down {
            return block(ShuttingDown, "正在退出…");
        }
        if self.capture_suspended() {
            return block(
                if self.lifecycle.blocked() {
                    SystemSuspended
                } else {
                    RecoveringInterruption
                },
                if self.lifecycle.blocked() {
                    "系统休眠或用户会话未激活，暂不能听写。"
                } else {
                    "正在回收中断的听写，完成后重新检查模型…"
                },
            );
        }
        if self.current.is_some() || self.source.busy() {
            return block(DictationBusy, "当前听写尚未结束。");
        }
        if let Some(error) = &self.store_error {
            return Some(Blocker::error(StorageUnavailable, error));
        }
        if self.db_pending != 0 || self.config_operation.is_some() || self.settings_operation_busy()
        {
            return block(UpdatingConfiguration, "正在更新本地配置…");
        }
        if self.model_work.is_some() {
            return block(PreparingModel, "正在准备语音模型…");
        }
        let Some(view) = &self.view else {
            return block(ReadingSettings, "正在读取本地设置…");
        };
        match &self.models {
            Ok(manager) if manager.snapshot().busy => {
                return block(ModelFilesBusy, "正在处理模型文件，请稍候。")
            }
            Err(error) => return Some(Blocker::error(ModelServiceUnavailable, error)),
            _ => {}
        }
        let state = self.worker.snapshot();
        if state.phase != WorkerPhase::Ready {
            return block(
                match state.phase {
                    WorkerPhase::Starting | WorkerPhase::Loading | WorkerPhase::Recovering => {
                        EnginePreparing
                    }
                    WorkerPhase::Paused | WorkerPhase::Unavailable => EngineUnavailable,
                    _ => ModelRequired,
                },
                match state.phase {
                    WorkerPhase::Paused | WorkerPhase::Unavailable => {
                        "语音引擎暂不可用，请重新加载模型。"
                    }
                    WorkerPhase::Starting | WorkerPhase::Loading | WorkerPhase::Recovering => {
                        "语音引擎正在准备…"
                    }
                    _ => "请在设置中选择已安装的语音模型。",
                },
            );
        }
        if state.model_id != view.settings.active_model_id
            || self
                .loaded
                .as_ref()
                .map(|l| l.bundle.model.installation_id())
                != state.model_id
        {
            return block(ModelConfigurationMismatch, "模型配置尚未同步。");
        }
        use crate::permissions::Microphone as Mic;
        match self.permissions.microphone {
            Mic::NotDetermined => {
                return block(
                    if self.permissions.requesting_microphone {
                        MicrophonePromptPending
                    } else {
                        MicrophoneNotGranted
                    },
                    if self.permissions.requesting_microphone {
                        "请在系统弹窗中选择是否允许麦克风访问。"
                    } else {
                        "尚未授权麦克风，请先允许 SpeakMore 录音。"
                    },
                )
            }
            Mic::Denied => {
                return block(MicrophoneDenied, "麦克风访问已关闭，请在系统设置中开启。")
            }
            Mic::Restricted => {
                return block(
                    MicrophoneRestricted,
                    "麦克风访问受到系统限制，请检查设备管理或家长控制设置。",
                )
            }
            Mic::Unavailable => {
                return block(
                    MicrophoneStatusUnavailable,
                    "无法确认麦克风权限，请检查系统设置或重启应用。",
                )
            }
            Mic::Authorized => {}
        }
        match &self.input_check {
            Some((id, result, at))
                if *id == view.settings.input_device_id && at.elapsed() < INPUT_CHECK_TTL =>
            {
                if let Err(error) = result {
                    return Some(Blocker::error(
                        InputDeviceUnavailable,
                        &super::super::audio_error(*error),
                    ));
                }
            }
            _ => {
                return block(
                    CheckingInputDevice,
                    "正在检查所选麦克风。长时间未完成时，请检查设备连接并重启应用。",
                )
            }
        }
        Option::None
    }
    fn publish(&mut self) {
        self.last_publish = Instant::now();
        self.dirty = false;
        let mut snapshot = self.view.as_ref().ok_or_else(db::unavailable).map(|view| {
            let reason = self.readiness();
            Snapshot {
                writing_context: self.writing_context.clone(),
                writing_rule: self.writing_rule.clone(),
                revision: 0,
                settings: view.settings.clone(),
                session: self.kernel.snapshot(self.now_ms()),
                dictionary: view.dictionary.clone(),
                recent_results: self.results.iter().map(|r| r.record.clone()).collect(),
                usage_today: view.usage_today,
                can_start: reason.is_none(),
                capture_suspended: self.capture_suspended(),
                shortcut_needs_release: self.shortcut_needs_release,
                readiness_action: reason
                    .as_ref()
                    .map_or(ReadinessAction::None, |r| r.code.action()),
                readiness_code: reason.as_ref().map_or(ReadinessCode::Ready, |r| r.code),
                readiness_detail_code: reason.as_ref().and_then(|r| r.detail_code.clone()),
                readiness_reason: reason.map_or_else(|| "可以开始听写。".into(), |r| r.message),
                permissions: self.permissions,
                desktop: true,
                worker: self.worker.snapshot(),
                model_operation: self.model_work.as_ref().map(|w| {
                    match w.stage {
                        Stage::Verifying => "verifying",
                        Stage::Loading => "loading",
                        Stage::Committing => "saving",
                        Stage::Unloading => "unloading",
                        Stage::Retiring | Stage::Restoring => "recovering",
                    }
                    .into()
                }),
                model_target_id: self.model_work.as_ref().and_then(|w| w.target.clone()),
                model_name: self.loaded.as_ref().map(|l| l.bundle.model.label()),
                audio_level: self
                    .current
                    .as_ref()
                    .and_then(|c| c.capture.as_ref())
                    .map_or(0.0, |h| h.level().clamp(0.0, 1.0)),
                last_error: self.last_error.clone(),
                storage_warning: self.storage_warning.clone(),
            }
        });
        // Every notification makes the WebView and tray re-read everything.
        let state = serde_json::to_vec(&snapshot).unwrap_or_default();
        if !state.is_empty() && state == self.published_state {
            return;
        }
        self.published_state = state;
        self.revision += 1;
        if let Ok(snapshot) = snapshot.as_mut() {
            snapshot.revision = self.revision;
        }
        if let Ok(mut published) = self.published.lock() {
            *published = snapshot;
        }
        (self.notify)(self.revision);
    }
}
pub(crate) fn spec(lease: &ModelLease) -> Result<ModelSpec> {
    let artifact = |role: &str| {
        lease
            .bundle
            .model
            .artifacts
            .iter()
            .find(|a| a.role == role)
            .ok_or_else(|| AppError::invalid("模型附件不完整。"))
    };
    let model = artifact("model")?;
    let vad = artifact("vad")?;
    let path = |name: &str| {
        lease
            .bundle
            .directory
            .join(name)
            .to_str()
            .map(str::to_string)
            .ok_or_else(|| AppError::invalid("模型路径不可用。"))
    };
    let tokens = lease
        .bundle
        .model
        .artifacts
        .iter()
        .find(|a| a.role == "tokens")
        .map(|a| -> Result<_> {
            Ok(speakmore_protocol::ModelFile {
                path: path(&a.filename)?,
                sha256: a.sha256.clone(),
                size_bytes: a.size_bytes,
            })
        })
        .transpose()?;
    Ok(ModelSpec {
        installation_id: lease.bundle.model.installation_id(),
        engine_id: lease.bundle.model.engine_id.clone(),
        profile_id: lease.bundle.model.engine_profile.clone(),
        bundle_sha256: lease.bundle.model.bundle_digest(),
        tokens,
        auxiliary: lease
            .bundle
            .model
            .artifacts
            .iter()
            .filter(|a| !["model", "vad", "tokens"].contains(&a.role.as_str()))
            .map(|a| {
                Ok(speakmore_protocol::ModelAsset {
                    role: a.role.clone(),
                    file: speakmore_protocol::ModelFile {
                        path: path(&a.filename)?,
                        sha256: a.sha256.clone(),
                        size_bytes: a.size_bytes,
                    },
                })
            })
            .collect::<Result<Vec<_>>>()?,
        model_id: lease.bundle.model.model_id.clone(),
        path: path(&model.filename)?,
        sha256: model.sha256.clone(),
        size_bytes: model.size_bytes,
        vad_path: path(&vad.filename)?,
        vad_sha256: vad.sha256.clone(),
        vad_size_bytes: vad.size_bytes,
    })
}
fn worker_error(error: speakmore_worker_supervisor::Error) -> AppError {
    use speakmore_worker_supervisor::Error;
    let message = match error {
        Error::NoSpeech => "未检测到清晰语音，请重新录音。",
        Error::ResultTooLong => "转写结果超过长度上限，请分段录音。",
        Error::InferenceTimeout => "转写超时，请重试。",
        Error::Busy => "语音引擎正在处理，请稍候。",
        Error::ModelUnavailable | Error::LoadTimeout => "模型加载失败，请重新加载或选择其他模型。",
        _ => "语音引擎暂不可用，请重新加载模型。",
    };
    AppError::new(&error.to_string(), message, true)
}

#[cfg(all(test, feature = "session-fixtures"))]
#[path = "tests.rs"]
mod tests;
