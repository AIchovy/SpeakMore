pub mod activity;
pub mod advanced;
pub mod assistant;
pub mod audio;
pub mod captions;
pub mod catalog;
pub mod experience;
mod live_notes;
mod moss;
pub mod moss_runtime;
pub mod notion;
mod picker;
mod playback;
mod store;
pub mod summary;
mod text;
pub mod workspace;
use serde::{Deserialize, Serialize};
use speakmore_core::error::{AppError, Result};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use store::Meeting;
use tauri::{Emitter, Manager, State, WebviewWindow};
use uuid::Uuid;

pub struct Managed(pub Result<Service>);
#[derive(Clone)]
pub struct Service(Arc<Inner>);
struct Inner {
    root: PathBuf,
    text_root: PathBuf,
    workers: PathBuf,
    app: tauri::AppHandle,
    runtime: crate::session_runtime::Handle,
    models: speakmore_models::ModelManager,
    config: speakmore_worker_supervisor::Configuration,
    lock: Mutex<()>,
    listing: Mutex<store::Listing>,
    closing: AtomicBool,
    captions: Mutex<captions::StateData>,
    assistant: Mutex<assistant::StateData>,
    audio_hub: Mutex<std::sync::Weak<audio::Hub>>,
    media_lease: Mutex<std::sync::Weak<()>>,
    caption_active: Mutex<Option<Active>>,
    active: Mutex<Option<Active>>,
    processing: Mutex<Option<Active>>,
    processing_lease: Mutex<std::sync::Weak<()>>,
    processing_queue: Mutex<std::collections::VecDeque<Uuid>>,
    queue_running: AtomicBool,
    live_jobs: Mutex<std::collections::HashSet<Uuid>>,
    begin_lock: tokio::sync::Mutex<()>,
    download: Mutex<Option<Active>>,
    import: Mutex<Option<ImportChoice>>,
    downloaded: AtomicU64,
    download_error: Mutex<Option<String>>,
    download_model: Mutex<Option<String>>,
    download_phase: Mutex<Option<String>>,
    experience_shortcuts: Mutex<Vec<String>>,
    shortcut_error: Mutex<Option<String>>,
}
#[derive(Clone)]
struct Active {
    id: Uuid,
    meeting: Option<Uuid>,
    kind: String,
    cancel: Arc<AtomicBool>,
    yielded: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    elapsed: Arc<AtomicU64>,
    levels: Arc<[AtomicU64; 2]>,
    recording: Arc<Mutex<Option<RecordingMeta>>>,
}
#[derive(Clone)]
struct RecordingMeta {
    title: String,
    source: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingRecordingView {
    meeting_id: Uuid,
    title: String,
    source: String,
    paused: bool,
    stopping: bool,
    elapsed_ms: u64,
    levels: [f32; 2],
    ui_locale: String,
    motion_policy: String,
}
struct ImportChoice {
    id: Uuid,
    path: PathBuf,
    duration: f64,
    created: Instant,
}
struct JobGuard {
    service: Service,
    active: Active,
    _reservation: Arc<()>,
}
impl Drop for JobGuard {
    fn drop(&mut self) {
        if self.active.kind == "processing" {
            let mut slot = self.service.0.processing.lock().unwrap();
            if slot.as_ref().is_some_and(|a| a.id == self.active.id) {
                *slot = None;
            }
            drop(slot);
            let _ = advanced::cleanup(&self.service);
            self.service.changed();
            return;
        }
        if self.active.kind == "download" {
            let mut download = self.service.0.download.lock().unwrap();
            if download.as_ref().is_some_and(|a| a.id == self.active.id) {
                *download = None;
                *self.service.0.download_model.lock().unwrap() = None;
                *self.service.0.download_phase.lock().unwrap() = None;
            }
            drop(download);
            self.service.changed();
            return;
        }
        if self.active.kind == "captions" {
            // Persist the final caption lines while the job still counts as busy, so exit waits for it.
            assistant::flush(&self.service);
        }
        let mut active = self.service.0.active.lock().unwrap();
        if active.as_ref().is_some_and(|a| a.id == self.active.id) {
            *active = self.service.0.caption_active.lock().unwrap().take();
            if let Some(a) = active.as_ref() {
                let _ = self
                    .service
                    .0
                    .runtime
                    .set_background_interrupt(Some((a.cancel.clone(), a.yielded.clone())));
            }
        } else {
            let mut caption = self.service.0.caption_active.lock().unwrap();
            if caption.as_ref().is_some_and(|a| a.id == self.active.id) {
                *caption = None;
            }
        }
        drop(active);
        let _ = advanced::cleanup(&self.service);
        self.service.changed();
    }
}
fn err(code: &str, message: &str) -> AppError {
    AppError::new(code, message, true)
}
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(err(
            "Cancelled",
            "操作已停止，已保存的内容保留在会议记录中。",
        ))
    } else {
        Ok(())
    }
}
fn get(state: &Managed) -> Result<Service> {
    state.0.clone()
}

impl Service {
    pub fn new(
        app: tauri::AppHandle,
        runtime: crate::session_runtime::Handle,
        models: speakmore_models::ModelManager,
        config: speakmore_worker_supervisor::Configuration,
    ) -> Result<Self> {
        let base = app.path().app_local_data_dir().map_err(store::io)?;
        let workers = if cfg!(debug_assertions) {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .join("resources/workers")
        } else {
            app.path()
                .resource_dir()
                .map_err(store::io)?
                .join("workers")
        };
        let root = base.join("meetings");
        let text_root = base.join("models/meeting-text");
        std::fs::create_dir_all(&root).map_err(store::io)?;
        std::fs::create_dir_all(&text_root).map_err(store::io)?;
        let service = Self(Arc::new(Inner {
            root,
            text_root,
            workers,
            app,
            runtime,
            models,
            config,
            lock: Mutex::new(()),
            listing: Mutex::new(store::Listing::default()),
            closing: AtomicBool::new(false),
            captions: Mutex::new(captions::StateData::default()),
            assistant: Mutex::new(assistant::StateData::default()),
            audio_hub: Mutex::new(std::sync::Weak::new()),
            media_lease: Mutex::new(std::sync::Weak::new()),
            caption_active: Mutex::new(None),
            active: Mutex::new(None),
            processing: Mutex::new(None),
            processing_lease: Mutex::new(std::sync::Weak::new()),
            processing_queue: Mutex::new(Default::default()),
            queue_running: AtomicBool::new(false),
            live_jobs: Mutex::new(Default::default()),
            begin_lock: tokio::sync::Mutex::new(()),
            download: Mutex::new(None),
            import: Mutex::new(None),
            downloaded: AtomicU64::new(0),
            download_error: Mutex::new(None),
            download_model: Mutex::new(None),
            download_phase: Mutex::new(None),
            experience_shortcuts: Mutex::new(vec![]),
            shortcut_error: Mutex::new(None),
        }));
        if let Ok(p) = workspace::preferences(&service) {
            service.0.captions.lock().unwrap().view.translation_enabled =
                p.caption_translation_enabled;
        }
        let interrupted = [
            "recording",
            "paused",
            "preparing",
            "importing",
            "transcribing",
            "summarizing",
        ];
        for summary in service.summaries()? {
            if !interrupted.contains(&summary.state.as_str()) {
                continue;
            }
            let Ok(mut m) = store::read(&service.0.root, summary.id) else {
                continue;
            };
            if ["recording", "paused", "preparing"].contains(&m.state.as_str())
                && m.source != "import"
            {
                m.gaps.push(store::RecordingGap {
                    after_ms: m.duration_ms,
                    started_at: m.last_audio_at.unwrap_or(m.created_at),
                    ended_at: None,
                    reason: "应用退出或系统中断".into(),
                });
            }
            m.state = "interrupted".into();
            m.error = Some("上次任务中断，已保存的音频可继续整理。".into());
            m.revision += 1;
            if store::save(&service.0.root, &m).is_err() {
                eprintln!("SpeakMore: could not mark meeting {} interrupted", m.id);
            }
        }
        // Deleting expired audio and trashed meetings can take long; never block app startup.
        let work = service.clone();
        std::thread::spawn(move || {
            store::empty_trash(&work.0.root);
            let _ = advanced::cleanup(&work);
        });
        Ok(service)
    }
    pub fn resume_saved_queue(&self) {
        if let Ok(meetings) = self.summaries() {
            for m in meetings {
                if m.state == "queued" {
                    self.resume_processing(m.id);
                }
            }
        }
    }
    /// List fields only (see store::summarize); parsed without the meeting lock.
    fn summaries(&self) -> Result<Vec<Meeting>> {
        store::summaries(&self.0.root, &self.0.listing)
    }
    fn changed(&self) {
        crate::desktop_controls::refresh(&self.0.app);
        let _ = self
            .0
            .app
            .emit_to("meeting-hud", "meeting_recording_changed", ());
        let _ = self.0.app.emit_to("main", "meetings_changed", ());
    }
    async fn begin(&self, meeting: Option<Uuid>, kind: &str) -> Result<JobGuard> {
        let _serial = self.0.begin_lock.lock().await;
        let cancel = Arc::new(AtomicBool::new(false));
        let yielded = Arc::new(AtomicBool::new(false));
        let active = Active {
            id: Uuid::new_v4(),
            meeting,
            kind: kind.into(),
            cancel: cancel.clone(),
            yielded: yielded.clone(),
            paused: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
            elapsed: Arc::new(AtomicU64::new(if kind == "recording" {
                meeting
                    .and_then(|id| store::read(&self.0.root, id).ok())
                    .map_or(0, |m| m.duration_ms)
            } else {
                0
            })),
            levels: Arc::new([AtomicU64::new(0), AtomicU64::new(0)]),
            recording: Arc::new(Mutex::new(None)),
        };
        if kind == "processing" {
            if self.busy() {
                return Err(AppError::busy());
            }
            let reservation = self.0.runtime.reserve_background(cancel, yielded).await?;
            *self.0.processing_lease.lock().unwrap() = Arc::downgrade(&reservation);
            *self.0.processing.lock().unwrap() = Some(active.clone());
            self.changed();
            return Ok(JobGuard {
                service: self.clone(),
                active,
                _reservation: reservation,
            });
        }
        if kind == "download" {
            let current = self.0.active.lock().unwrap();
            if current.as_ref().is_some_and(|a| a.kind == "removing_model") {
                return Err(AppError::busy());
            }
            let mut download = self.0.download.lock().unwrap();
            if download.is_some() {
                return Err(AppError::busy());
            }
            *download = Some(active.clone());
            drop(download);
            drop(current);
            self.changed();
            return Ok(JobGuard {
                service: self.clone(),
                active,
                _reservation: Arc::new(()),
            });
        }
        if kind == "removing_model" && self.0.download.lock().unwrap().is_some() {
            return Err(AppError::busy());
        }
        let shared = {
            let active = self.0.active.lock().unwrap();
            let compatible = active.as_ref().is_some_and(|a| {
                (a.kind == "captions" && ["recording", "importing"].contains(&kind))
                    || (["recording", "importing"].contains(&a.kind.as_str()) && kind == "captions")
            });
            if compatible {
                self.0.media_lease.lock().unwrap().upgrade()
            } else {
                None
            }
        };
        let sharing = shared.is_some();
        // Choosing a file only shows a dialog; background work is preempted once an import starts.
        let preempted = if ["recording", "importing", "captions"].contains(&kind) && !sharing {
            let processing = self.0.processing.lock().unwrap();
            processing.as_ref().and_then(|p| {
                let lease = self.0.processing_lease.lock().unwrap().upgrade()?;
                p.yielded.store(true, Ordering::Release);
                p.cancel.store(true, Ordering::Release);
                Some(lease)
            })
        } else {
            None
        };
        let reservation = if let Some(lease) = shared {
            lease
        } else if let Some(lease) = preempted {
            self.0.runtime.set_background_interrupt(None)?;
            lease
        } else if kind == "choosing" {
            Arc::new(())
        } else if ["processing", "captions"].contains(&kind) {
            self.0
                .runtime
                .reserve_background(cancel.clone(), yielded.clone())
                .await?
        } else {
            self.0.runtime.reserve_settings_operation().await?
        };
        let mut current = self.0.active.lock().unwrap();
        if kind == "removing_model" && self.0.download.lock().unwrap().is_some() {
            return Err(AppError::busy());
        }
        if current.is_some() && !sharing {
            return Err(AppError::busy());
        }
        if sharing && self.0.caption_active.lock().unwrap().is_some() {
            return Err(AppError::busy());
        }
        if sharing && kind == "captions" {
            *self.0.caption_active.lock().unwrap() = Some(active.clone());
        } else {
            if sharing {
                *self.0.caption_active.lock().unwrap() = current.take();
            }
            *current = Some(active.clone());
        }
        if ["recording", "captions"].contains(&kind) {
            *self.0.media_lease.lock().unwrap() = Arc::downgrade(&reservation);
        }
        if sharing {
            let _ = self.0.runtime.set_background_interrupt(None);
        }
        drop(current);
        self.changed();
        Ok(JobGuard {
            service: self.clone(),
            active,
            _reservation: reservation,
        })
    }
    fn edit<T>(&self, id: Uuid, f: impl FnOnce(&mut Meeting) -> Result<T>) -> Result<T> {
        let _lock = self.0.lock.lock().map_err(store::io)?;
        let mut m = store::read(&self.0.root, id)?;
        let result = f(&mut m)?;
        m.revision += 1;
        store::save(&self.0.root, &m)?;
        self.changed();
        Ok(result)
    }
    fn fail(&self, id: Uuid, error: AppError) {
        let recording = self.is_recording(id);
        let _ = self.edit(id, |m| {
            if recording {
                if let Some(gap) = m
                    .gaps
                    .last_mut()
                    .filter(|g| g.ended_at.is_none() && g.after_ms == m.duration_ms)
                {
                    gap.reason = error.user_message.clone();
                } else {
                    m.gaps.push(store::RecordingGap {
                        after_ms: m.duration_ms,
                        started_at: m.last_audio_at.unwrap_or_else(crate::now),
                        ended_at: None,
                        reason: error.user_message.clone(),
                    });
                }
            }
            m.state = if error.code == "Cancelled" {
                "saved"
            } else {
                "failed"
            }
            .into();
            m.error = Some(error.user_message);
            Ok(())
        });
    }
    pub fn shutdown(&self) {
        self.0.closing.store(true, Ordering::Release);
        captions::stop_service(self);
        if let Some(a) = self.0.processing.lock().unwrap().as_ref() {
            a.cancel.store(true, Ordering::Release);
        }
        assistant::shutdown(self);
        if let Some(a) = self.0.download.lock().unwrap().as_ref() {
            a.cancel.store(true, Ordering::Release);
        }
        if let Some(a) = self.0.active.lock().unwrap().as_ref() {
            a.stop.store(true, Ordering::Release);
            if a.kind != "recording" {
                a.cancel.store(true, Ordering::Release);
            }
        }
    }
    pub fn busy(&self) -> bool {
        self.0.active.lock().unwrap().is_some()
            || self.0.processing.lock().unwrap().is_some()
            || !self.0.live_jobs.lock().unwrap().is_empty()
    }
    pub(super) fn meeting_busy(&self, id: Uuid) -> bool {
        self.0
            .active
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|a| a.meeting == Some(id))
            || self
                .0
                .processing
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|a| a.meeting == Some(id))
            || self.0.live_jobs.lock().unwrap().contains(&id)
    }
    fn is_recording(&self, id: Uuid) -> bool {
        self.0
            .active
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|a| a.kind == "recording" && a.meeting == Some(id))
    }
    async fn process(&self, id: Uuid) -> Result<()> {
        let guard = self.begin(Some(id), "processing").await?;
        let m = store::read(&self.0.root, id)?;
        if m.samples() < 4000 {
            return Err(err("MeetingEmpty", "没有可转写的音频，请录音或导入文件。"));
        }
        let needs_transcription = m.segments.is_empty() || m.processed_samples < m.samples();
        let snapshot = self.0.runtime.snapshot()?;
        let selected = m
            .model_id
            .clone()
            .or(snapshot.settings.active_model_id.clone())
            .or_else(|| (!needs_transcription).then(String::new))
            .ok_or_else(|| {
                err(
                    "ModelRequired",
                    "请在语音模型中下载并选择转写模型，再继续整理。",
                )
            })?;
        if needs_transcription && selected == "moss" {
            moss_runtime::python(self)?;
            if !catalog::ready(self, "moss") || !catalog::ready(self, "speaker") {
                return Err(err(
                    "ModelRequired",
                    "请在设置 → 语音模型中安装 MOSS 与说话人连续识别模型，再继续整理。",
                ));
            }
        }
        let lease = if !needs_transcription || selected == "moss" {
            None
        } else {
            Some(
                self.0
                    .models
                    .lease(&selected)
                    .await
                    .map_err(crate::model_error)?,
            )
        };
        let spec = lease
            .as_ref()
            .map(crate::session_runtime::model_spec)
            .transpose()?;
        self.edit(id, |m| {
            if guard.active.yielded.load(Ordering::Acquire) {
                return Err(AppError::busy());
            }
            check_cancel(&guard.active.cancel)?;
            m.state = if needs_transcription {
                "transcribing"
            } else {
                "summarizing"
            }
            .into();
            m.error = None;
            if needs_transcription {
                m.model_id = Some(selected);
            }
            Ok(())
        })?;
        let service = self.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let result = (|| {
                if needs_transcription {
                    if let Some(spec) = spec {
                        service.transcribe(id, spec, &guard.active.cancel, false)?;
                    } else {
                        moss::transcribe(&service, id, &guard.active.cancel, false)?;
                    }
                }
                drop(lease);
                check_cancel(&guard.active.cancel)?;
                let mut meeting = store::read(&service.0.root, id)?;
                if meeting.summary_follow_ai {
                    meeting.summary_provider_id =
                        summary::Preference::default().resolve(&service)?;
                }
                if meeting.segments.is_empty() {
                    return Err(err(
                        "MeetingNoSpeech",
                        "未检测到清晰语音。录音已保留，可检查音源后重试。",
                    ));
                }
                if meeting.summary_provider_id.is_none() && !text::available(&service.0.text_root) {
                    service.edit(id, |m| {
                        check_cancel(&guard.active.cancel)?;
                        m.state = "waiting_model".into();
                        m.error = Some("逐字稿已完成。下载本地整理模型后可生成纪要。".into());
                        Ok(())
                    })?;
                    return Ok(());
                }
                service.edit(id, |m| {
                    check_cancel(&guard.active.cancel)?;
                    m.state = "summarizing".into();
                    Ok(())
                })?;
                let considered_context = meeting.context_revision;
                let items =
                    text::background(|| summary::run(&service, &meeting, &guard.active.cancel))?;
                check_cancel(&guard.active.cancel)?;
                service.edit(id, |m| {
                    check_cancel(&guard.active.cancel)?;
                    if !m.items.is_empty() {
                        advanced::archive(&service, m)?;
                    }
                    let mut items = items;
                    live_notes::preserve_completed(&m.items, &mut items);
                    m.items = experience::preserve_edits(&m.items, items);
                    m.dirty_segments.clear();
                    m.summarized_context_revision = considered_context;
                    m.summary_stale = m.context_revision != considered_context;
                    m.state = "ready".into();
                    m.error = None;
                    Ok(())
                })?;
                Ok(())
            })();
            if let Err(e) = result {
                if !guard.active.yielded.load(Ordering::Acquire) {
                    service.fail(id, e);
                }
            }
            let yielded = guard.active.yielded.load(Ordering::Acquire);
            drop(guard);
            if yielded && !service.is_recording(id) {
                service.resume_processing(id);
            }
        });
        Ok(())
    }
    fn resume_processing(&self, id: Uuid) {
        if self.0.closing.load(Ordering::Acquire)
            || self.is_recording(id)
            || self
                .0
                .processing
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|a| a.meeting == Some(id))
        {
            return;
        }
        {
            let mut queue = self.0.processing_queue.lock().unwrap();
            if !queue.contains(&id) {
                queue.push_back(id);
            }
        }
        let _ = self.edit(id, |m| {
            m.state = "queued".into();
            m.error = None;
            Ok(())
        });
        if self.0.queue_running.swap(true, Ordering::AcqRel) {
            return;
        }
        let s = self.clone();
        let future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
            Box::pin(async move {
                while !s.0.closing.load(Ordering::Acquire) {
                    let next = s.0.processing_queue.lock().unwrap().front().copied();
                    let Some(next) = next else {
                        s.0.queue_running.store(false, Ordering::Release);
                        // A producer arriving between the empty check and flag reset gets a new worker.
                        let pending = s.0.processing_queue.lock().unwrap().front().copied();
                        if let Some(id) = pending {
                            s.resume_processing(id);
                        }
                        return;
                    };
                    if !s.busy() {
                        match s.process(next).await {
                            Ok(()) => {
                                s.0.processing_queue
                                    .lock()
                                    .unwrap()
                                    .retain(|id| *id != next);
                            }
                            Err(e) if e.code == "Busy" || s.is_recording(next) => {}
                            Err(e) => {
                                s.0.processing_queue
                                    .lock()
                                    .unwrap()
                                    .retain(|id| *id != next);
                                s.fail(next, e);
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                s.0.queue_running.store(false, Ordering::Release);
            });
        tauri::async_runtime::spawn(future);
    }
    fn transcribe(
        &self,
        id: Uuid,
        spec: speakmore_worker_supervisor::ModelSpec,
        cancel: &AtomicBool,
        live: bool,
    ) -> Result<()> {
        use speakmore_worker_supervisor::{Event, Phase, Supervisor, Transcription};
        let mut m = store::read(&self.0.root, id)?;
        if !live && m.processed_samples >= m.samples() {
            return Ok(());
        }
        let mut worker = Supervisor::new(self.0.config.clone())
            .map_err(|_| err("AsrUnavailable", "转写引擎不可用。"))?;
        let result = (|| {
            worker
                .set_model(spec)
                .map_err(|_| err("ModelLoad", "无法加载会议转写模型。"))?;
            let deadline = Instant::now() + Duration::from_secs(120);
            while worker.snapshot().phase != Phase::Ready {
                check_cancel(cancel)?;
                if Instant::now() > deadline {
                    return Err(err("ModelTimeout", "会议模型加载超时。"));
                }
                for e in worker.poll() {
                    if let Event::ModelFailed { .. } = e {
                        return Err(err("ModelLoad", "会议模型加载失败。"));
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let mut summary_checkpoint = m.processed_samples;
            while live || m.processed_samples < m.samples() {
                check_cancel(cancel)?;
                if live
                    && m.samples().saturating_sub(m.processed_samples) < 30 * store::RATE as usize
                {
                    std::thread::sleep(Duration::from_millis(200));
                    m = store::read(&self.0.root, id)?;
                    continue;
                }
                let from = m.processed_samples;
                let remaining = m.samples() - from;
                let mut pcm = store::samples(
                    &self.0.root,
                    &m,
                    from,
                    if remaining <= 30 * store::RATE as usize + 4000 {
                        remaining
                    } else {
                        30 * store::RATE as usize
                    },
                )?;
                // Prefer a quiet boundary near the end instead of cutting a word at 30s.
                if remaining > pcm.len() && pcm.len() > 20 * store::RATE as usize {
                    if let Some((offset, _)) = pcm[20 * store::RATE as usize..]
                        .chunks(1600)
                        .enumerate()
                        .map(|(i, s)| (i, s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32))
                        .filter(|(_, energy)| *energy < 0.0001)
                        .last()
                    {
                        pcm.truncate(20 * store::RATE as usize + (offset + 1) * 1600);
                    }
                }
                let count = pcm.len();
                if count < 4000 {
                    service_tail(self, id, from + count, cancel)?;
                    break;
                }
                let job = Uuid::new_v4();
                worker
                    .transcribe(Transcription {
                        session_id: job,
                        language: m.language.clone(),
                        dictionary_revision: 1,
                        dictionary_terms: workspace::preferences(self)?
                            .terms
                            .into_iter()
                            .map(|t| t.replacement)
                            .collect(),
                        samples: pcm,
                    })
                    .map_err(|_| err("AsrFailed", "无法开始会议转写。"))?;
                let deadline = Instant::now() + Duration::from_secs(180);
                let text = loop {
                    check_cancel(cancel)?;
                    if Instant::now() > deadline {
                        return Err(err("AsrTimeout", "本段转写超时，可继续处理已保存的录音。"));
                    }
                    let mut completed = None;
                    for e in worker.poll() {
                        match e {
                            Event::Completed(c) if c.session_id == job => {
                                completed = Some(c.text);
                            }
                            Event::Failed { session_id, error } if session_id == job => {
                                if error == speakmore_worker_supervisor::Error::NoSpeech {
                                    completed = Some(String::new());
                                } else {
                                    return Err(err("AsrFailed", "本段转写失败，可重试继续处理。"));
                                }
                            }
                            _ => {}
                        }
                    }
                    if let Some(text) = completed {
                        break text;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                };
                self.edit(id, |meeting| {
                    check_cancel(cancel)?;
                    if !text.trim().is_empty() {
                        meeting.segments.push(store::Segment {
                            id: format!("s{}", meeting.segments.len() + 1),
                            start_ms: from as u64 / 16,
                            end_ms: (from + count) as u64 / 16,
                            text: workspace::apply_terms(
                                &text,
                                &workspace::preferences(self)?.terms,
                            ),
                            speaker: None,
                            speaker_id: None,
                        });
                    }
                    meeting.processed_samples = from + count;
                    Ok(())
                })?;
                m = store::read(&self.0.root, id)?;
                if live
                    && m.processed_samples.saturating_sub(summary_checkpoint)
                        >= 30 * store::RATE as usize
                {
                    live_notes::summarize(self, id, summary_checkpoint, cancel)?;
                    summary_checkpoint = m.processed_samples;
                }
            }
            Ok(())
        })();
        if cancel.load(Ordering::Acquire) {
            drop(worker);
            return result;
        }
        worker.shutdown();
        let end = Instant::now() + Duration::from_secs(3);
        while worker.snapshot().phase != Phase::Stopped && Instant::now() < end {
            worker.poll();
            std::thread::sleep(Duration::from_millis(20));
        }
        result
    }
}
fn service_tail(s: &Service, id: Uuid, n: usize, cancel: &AtomicBool) -> Result<()> {
    s.edit(id, |m| {
        check_cancel(cancel)?;
        m.processed_samples = n;
        Ok(())
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct List {
    meetings: Vec<Meeting>,
    active: Option<ActiveView>,
    text_model_ready: bool,
    text_model_bytes: u64,
    downloaded_bytes: u64,
    download_error: Option<String>,
    download_model: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveView {
    id: Uuid,
    meeting_id: Option<Uuid>,
    kind: String,
    paused: bool,
    elapsed_ms: u64,
    levels: [f32; 2],
}
#[tauri::command]
pub async fn meeting_list(window: WebviewWindow, state: State<'_, Managed>) -> Result<List> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        // meeting.json is replaced atomically, so listing never waits for recording or edits.
        let meetings = s.summaries()?;
        let active =
            s.0.active
                .lock()
                .unwrap()
                .clone()
                .or_else(|| s.0.processing.lock().unwrap().clone())
                .or_else(|| s.0.download.lock().unwrap().clone())
                .as_ref()
                .map(|a| ActiveView {
                    id: a.id,
                    meeting_id: a.meeting,
                    kind: a.kind.clone(),
                    paused: a.paused.load(Ordering::Acquire),
                    elapsed_ms: a.elapsed.load(Ordering::Acquire),
                    levels: [
                        a.levels[0].load(Ordering::Acquire) as f32 / 1000.0,
                        a.levels[1].load(Ordering::Acquire) as f32 / 1000.0,
                    ],
                });
        Ok(List {
            meetings,
            active,
            text_model_ready: text::available(&s.0.text_root),
            text_model_bytes: text::MODEL_SIZE,
            downloaded_bytes: s.0.downloaded.load(Ordering::Acquire),
            download_error: s.0.download_error.lock().unwrap().clone(),
            download_model: s.0.download_model.lock().unwrap().clone(),
        })
    })
    .await
    .map_err(store::io)?
}
#[tauri::command]
pub async fn meeting_get(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
) -> Result<Meeting> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        if !s.0.root.join(id.to_string()).join("meeting.json").exists() {
            return Err(err("NotFound", "这场会议已被删除，请刷新列表。"));
        }
        let mut meeting = store::read(&s.0.root, id)?;
        experience::refresh_artifacts(&s, &mut meeting);
        Ok(meeting)
    })
    .await
    .map_err(store::io)?
}

#[tauri::command]
pub async fn meeting_start(
    window: WebviewWindow,
    state: State<'_, Managed>,
    title: String,
    language: String,
    source: Option<String>,
    app_pid: Option<i32>,
    model_id: Option<String>,
    template: Option<String>,
    project: Option<String>,
) -> Result<Uuid> {
    crate::main_only(&window)?;
    validate_language(&language)?;
    let s = get(&state)?;
    let source = source.unwrap_or_else(|| "microphone".into());
    audio::validate_recording_permission(&source, crate::permissions::read().microphone)?;
    let mut meeting = Meeting::new(title, &source, language);
    if source != "microphone" {
        // Listing shareable apps can wait on ScreenCaptureKit and its permission prompt.
        let work = s.clone();
        meeting.system_app =
            tauri::async_runtime::spawn_blocking(move || audio::scope_for(&work, app_pid))
                .await
                .map_err(store::io)??;
    }
    let preference = summary::preference(&s)?;
    meeting.summary_provider_id = preference.provider_id;
    meeting.summary_follow_ai = preference.follow_ai;
    meeting.model_id = model_id;
    meeting.template = template.unwrap_or(workspace::preferences(&s)?.default_template);
    workspace::template_instruction(&meeting.template)?;
    meeting.template_prompt = workspace::preferences(&s)?
        .template_rules
        .get(&meeting.template)
        .cloned()
        .unwrap_or_default();
    meeting.project = project
        .unwrap_or_default()
        .trim()
        .chars()
        .take(120)
        .collect();
    let id = meeting.id;
    let guard = s.begin(Some(id), "recording").await?;
    // Starting capture must succeed before persisting a meeting or reporting success.
    let capture_service = s.clone();
    let capture_source = source.clone();
    let capture = tauri::async_runtime::spawn_blocking(move || {
        audio::Capture::start(&capture_service, &capture_source, app_pid)
    })
    .await
    .map_err(store::io)??;
    assistant::attach_meeting(&s, &mut meeting)?;
    meeting.state = "preparing".into();
    store::save(&s.0.root, &meeting)?;
    captions::emit(&s);
    let work = s.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = audio::record(&work, &guard, &source, capture);
        let success = result.is_ok();
        if let Err(e) = result {
            work.fail(id, e);
        }
        drop(guard);
        if success && !work.0.closing.load(Ordering::Acquire) {
            work.resume_processing(id);
        }
    });
    Ok(id)
}

#[tauri::command]
pub async fn meeting_resume(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    source: String,
    app_pid: Option<i32>,
    reset_scope: Option<bool>,
) -> Result<()> {
    crate::main_only(&window)?;
    audio::validate_recording_permission(&source, crate::permissions::read().microphone)?;
    let s = get(&state)?;
    let meeting = store::read(&s.0.root, id)?;
    if meeting.audio_deleted || meeting.source == "import" {
        return Err(AppError::invalid("此记录不能继续录音。"));
    }
    let work = s.clone();
    let scope_source = source.clone();
    let (app_pid, system_app) = tauri::async_runtime::spawn_blocking(move || {
        let app_pid =
            if scope_source != "microphone" && app_pid.is_none() && !reset_scope.unwrap_or(false) {
                meeting
                    .system_app
                    .as_ref()
                    .map(|scope| audio::resume_scope(&work, scope))
                    .transpose()?
            } else {
                app_pid
            };
        let system_app = if scope_source == "microphone" {
            None
        } else {
            audio::scope_for(&work, app_pid)?
        };
        Ok::<_, AppError>((app_pid, system_app))
    })
    .await
    .map_err(store::io)??;
    let guard = s.begin(Some(id), "recording").await?;
    // A failed resume must leave the original meeting and its processing queue intact.
    let capture_service = s.clone();
    let capture_source = source.clone();
    let capture = tauri::async_runtime::spawn_blocking(move || {
        audio::Capture::start(&capture_service, &capture_source, app_pid)
    })
    .await
    .map_err(store::io)??;
    s.0.processing_queue
        .lock()
        .unwrap()
        .retain(|queued| *queued != id);
    s.edit(id, |m| {
        advanced::archive(&s, m)?;
        if m.gaps.last().is_none_or(|gap| gap.ended_at.is_some()) {
            m.gaps.push(store::RecordingGap {
                after_ms: m.duration_ms,
                started_at: m.last_audio_at.unwrap_or_else(crate::now),
                ended_at: None,
                reason: "继续同一场会议录音".into(),
            });
        }
        m.source = source.clone();
        m.system_app = system_app;
        m.state = "preparing".into();
        m.error = None;
        m.summary_stale = !m.items.is_empty();
        Ok(())
    })?;
    let work = s.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = audio::record(&work, &guard, &source, capture);
        let success = result.is_ok();
        if let Err(error) = result {
            work.fail(id, error);
        }
        drop(guard);
        if success {
            work.resume_processing(id);
        }
    });
    Ok(())
}

fn validate_language(l: &str) -> Result<()> {
    if ["auto", "zh", "en"].contains(&l) {
        Ok(())
    } else {
        Err(AppError::invalid("请选择中文、英文或自动识别。"))
    }
}
#[tauri::command]
pub fn meeting_control(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    action: String,
) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    let active =
        s.0.active
            .lock()
            .unwrap()
            .clone()
            .filter(|a| a.meeting == Some(id))
            .or_else(|| {
                s.0.processing
                    .lock()
                    .unwrap()
                    .clone()
                    .filter(|a| a.meeting == Some(id))
            });
    if action == "cancel" {
        let queued = {
            let mut queue = s.0.processing_queue.lock().unwrap();
            let present = queue.contains(&id);
            queue.retain(|queued| *queued != id);
            present
        };
        if let Some(a) = &active {
            a.cancel.store(true, Ordering::Release);
            a.yielded.store(false, Ordering::Release);
        }
        if queued && active.is_none() {
            return s.edit(id, |m| {
                m.state = "saved".into();
                m.error = None;
                Ok(())
            });
        }
    }
    let a = active.ok_or_else(AppError::busy)?;
    if a.meeting != Some(id) {
        return Err(AppError::conflict());
    }
    match action.as_str() {
        "stop" if a.kind == "recording" => a.stop.store(true, Ordering::Release),
        "pause" if a.kind == "recording" => a.paused.store(true, Ordering::Release),
        "resume" if a.kind == "recording" => a.paused.store(false, Ordering::Release),
        "cancel" => {
            a.cancel.store(true, Ordering::Release);
        }
        _ => return Err(AppError::invalid("当前操作不可用。")),
    };
    s.changed();
    Ok(())
}
#[tauri::command]
pub async fn meeting_process(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    store::read(&s.0.root, id)?;
    if s.is_recording(id) {
        return Err(AppError::busy());
    }
    s.resume_processing(id);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportView {
    choice_id: Uuid,
    name: String,
    duration_seconds: f64,
}
#[tauri::command]
pub async fn meeting_pick(
    window: WebviewWindow,
    state: State<'_, Managed>,
) -> Result<Option<ImportView>> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    let _guard = s.begin(None, "choosing").await?;
    let Some(path) = picker::open(&window).await? else {
        return Ok(None);
    };
    let meta = std::fs::symlink_metadata(&path).map_err(store::io)?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 2 * 1024 * 1024 * 1024 {
        return Err(err("ImportInvalid", "请选择不超过 2 GB 的音频文件。"));
    }
    let worker = s.0.workers.join("speakmore-meeting-decode");
    let source = path.clone();
    let duration = tauri::async_runtime::spawn_blocking(move || -> Result<f64> {
        let out = std::process::Command::new(worker)
            .arg("probe")
            .arg(source)
            .output()
            .map_err(store::io)?;
        if !out.status.success() {
            return Err(err(
                "ImportInvalid",
                "无法读取此音频，请使用 WAV、MP3、M4A 或 FLAC。",
            ));
        }
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(store::io)?;
        value["duration"]
            .as_f64()
            .filter(|d| d.is_finite() && *d > 0.0)
            .ok_or_else(|| err("ImportInvalid", "音频时长无效。"))
    })
    .await
    .map_err(store::io)??;
    let id = Uuid::new_v4();
    let name = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    *s.0.import.lock().unwrap() = Some(ImportChoice {
        id,
        path,
        duration,
        created: Instant::now(),
    });
    Ok(Some(ImportView {
        choice_id: id,
        name,
        duration_seconds: duration,
    }))
}
#[tauri::command]
pub async fn meeting_import(
    window: WebviewWindow,
    state: State<'_, Managed>,
    choice_id: Uuid,
    title: String,
    start_seconds: f64,
    length_seconds: f64,
    language: String,
    model_id: Option<String>,
    template: Option<String>,
) -> Result<Uuid> {
    crate::main_only(&window)?;
    validate_language(&language)?;
    let s = get(&state)?;
    let mut m = Meeting::new(title, "import", language);
    let preference = summary::preference(&s)?;
    m.summary_provider_id = preference.provider_id;
    m.summary_follow_ai = preference.follow_ai;
    m.model_id = model_id;
    m.template = template.unwrap_or(workspace::preferences(&s)?.default_template);
    workspace::template_instruction(&m.template)?;
    m.template_prompt = workspace::preferences(&s)?
        .template_rules
        .get(&m.template)
        .cloned()
        .unwrap_or_default();
    let id = m.id;
    let guard = s.begin(Some(id), "importing").await?;
    let source = {
        let mut choice = s.0.import.lock().unwrap();
        let c = choice
            .as_ref()
            .filter(|c| c.id == choice_id && c.created.elapsed() < Duration::from_secs(600))
            .ok_or_else(|| err("ImportExpired", "请选择要导入的文件。"))?;
        if !start_seconds.is_finite()
            || !length_seconds.is_finite()
            || start_seconds < 0.0
            || length_seconds < 0.25
            || length_seconds > 3600.0
            || start_seconds + length_seconds > c.duration + 0.01
        {
            return Err(AppError::invalid("请选择文件内最多 60 分钟的区间。"));
        }
        choice.take().unwrap().path
    };
    store::save(&s.0.root, &m)?;
    s.edit(id, |m| {
        m.state = "importing".into();
        Ok(())
    })?;
    let work = s.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = (|| {
            use std::io::Read;
            let temp = tempfile::NamedTempFile::new_in(&work.0.root).map_err(store::io)?;
            let mut child =
                std::process::Command::new(work.0.workers.join("speakmore-meeting-decode"))
                    .arg("decode")
                    .arg(source)
                    .arg(temp.path())
                    .arg(start_seconds.to_string())
                    .arg(length_seconds.to_string())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .map_err(store::io)?;
            let end = Instant::now() + Duration::from_secs(600);
            let status = loop {
                if guard.active.cancel.load(Ordering::Acquire) || Instant::now() > end {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(err("ImportStopped", "导入已停止，请重试。"));
                }
                if temp.as_file().metadata().map_err(store::io)?.len()
                    > store::IMPORT_LIMIT as u64 * 4
                {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(err("ImportInvalid", "解码音频超过 60 分钟。"));
                }
                if let Some(status) = child.try_wait().map_err(store::io)? {
                    break status;
                }
                std::thread::sleep(Duration::from_millis(40));
            };
            if !status.success() {
                return Err(err(
                    "ImportDecode",
                    "无法解码此音频文件，请尝试 WAV 或 M4A。",
                ));
            }
            let mut file = std::fs::File::open(temp.path()).map_err(store::io)?;
            let len = file.metadata().map_err(store::io)?.len();
            if len % 4 != 0 || len > store::IMPORT_LIMIT as u64 * 4 {
                return Err(store::io("length"));
            }
            let mut remaining = len as usize / 4;
            while remaining > 0 {
                check_cancel(&guard.active.cancel)?;
                let n = remaining.min(80000);
                let mut bytes = vec![0; n * 4];
                file.read_exact(&mut bytes).map_err(store::io)?;
                let pcm: Vec<f32> = bytes
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                    .collect();
                let _l = work.0.lock.lock().unwrap();
                let mut m = store::read(&work.0.root, id)?;
                store::append(&work.0.root, &mut m, &pcm)?;
                remaining -= n;
            }
            work.edit(id, |m| {
                m.state = "saved".into();
                Ok(())
            })?;
            Ok(())
        })();
        let success = result.is_ok();
        if let Err(e) = result {
            work.fail(id, e);
        }
        drop(guard);
        if success {
            work.resume_processing(id);
        }
    });
    Ok(id)
}

#[tauri::command]
pub async fn meeting_text_download(window: WebviewWindow, state: State<'_, Managed>) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    let guard = s.begin(None, "download").await?;
    *s.0.download_model.lock().unwrap() = Some("qwen3".into());
    s.0.downloaded.store(0, Ordering::Release);
    *s.0.download_error.lock().unwrap() = None;
    tauri::async_runtime::spawn(async move {
        if let Err(e) = text::download(&s, guard.active.cancel.clone()).await {
            *s.0.download_error.lock().unwrap() = Some(e.user_message);
        }
        drop(guard);
    });
    Ok(())
}
#[tauri::command]
pub fn meeting_text_cancel(window: WebviewWindow, state: State<'_, Managed>) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    if let Some(a) = s.0.download.lock().unwrap().as_ref() {
        if a.kind == "download" {
            a.cancel.store(true, Ordering::Release);
        }
    }
    Ok(())
}
#[tauri::command]
pub fn meeting_update(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    revision: u64,
    title: String,
    mut items: Vec<store::Item>,
) -> Result<Meeting> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    if s.meeting_busy(id) {
        return Err(AppError::busy());
    }
    s.edit(id, |m| {
        if revision != m.revision {
            return Err(AppError::conflict());
        }
        if title.trim().is_empty() || title.chars().count() > 120 {
            return Err(AppError::invalid("标题请填写 1–120 个字。"));
        }
        // Older live results may exceed current limits; only what the user changes is validated.
        for item in &mut items {
            item.evidence.truncate(20);
        }
        if items.len() > m.items.len().max(200) {
            return Err(err("SummaryInvalid", "纪要条目过多，请缩短输入后重试。"));
        }
        let changed: Vec<_> = items
            .iter()
            .enumerate()
            .filter(|(index, item)| m.items.get(*index) != Some(*item))
            .map(|(_, item)| item.clone())
            .collect();
        store::validate_items(&changed, &m.segments)?;
        advanced::archive(&s, m)?;
        m.title = title.trim().into();
        for (index, item) in items.iter_mut().enumerate() {
            item.kind = match item.status.as_str() {
                "proposal" | "pending" => "next",
                "decision" => "decision",
                "commitment" => "todo",
                "information" => "fact",
                _ => &item.kind,
            }
            .to_string();
            if m.items.get(index).is_none_or(|old| {
                old.text != item.text
                    || old.owner != item.owner
                    || old.due != item.due
                    || old.kind != item.kind
                    || old.status != item.status
                    || old.recipient != item.recipient
            }) {
                item.edited_from = m
                    .items
                    .get(index)
                    .map(|old| old.edited_from.clone().unwrap_or_else(|| old.text.clone()));
                item.edited = true;
                item.support = "review".into();
                item.reviewed = false;
            } else {
                item.edited |= m.items[index].edited;
                item.edited_from = m.items[index].edited_from.clone();
                item.reviewed = m.items[index].reviewed;
                item.support = m.items[index].support.clone();
            }
        }
        m.items = items;
        Ok(())
    })?;
    store::read(&s.0.root, id)
}
#[tauri::command]
pub async fn meeting_delete(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let trash = {
            let _l = s.0.lock.lock().map_err(store::io)?;
            if s.meeting_busy(id) {
                return Err(AppError::busy());
            }
            let meeting = store::read(&s.0.root, id)?;
            s.0.processing_queue
                .lock()
                .unwrap()
                .retain(|queued| *queued != id);
            assistant::remove(&s, meeting.assistant_session_id.unwrap_or(id))?;
            {
                let mut c = s.0.captions.lock().unwrap();
                if c.view.meeting_id == Some(id) {
                    c.view.meeting_id = None;
                }
            }
            let trash = store::trash(&s.0.root, id)?;
            let journal = s.0.root.join(format!("notion-{id}.json"));
            if journal.exists() {
                std::fs::remove_file(journal).map_err(store::io)?;
            }
            trash
        };
        captions::emit(&s);
        s.changed();
        // Hours of chunked audio can be thousands of files; remove them after the UI updates.
        std::thread::spawn(move || {
            let _ = std::fs::remove_dir_all(trash);
        });
        Ok(())
    })
    .await
    .map_err(store::io)?
}
#[tauri::command]
pub async fn meeting_export(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    format: String,
) -> Result<bool> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    let m = store::read(&s.0.root, id)?;
    let contents = match format.as_str() {
        "md" => m.text(),
        "json" => serde_json::to_string_pretty(&m).map_err(store::io)?,
        "txt" => m
            .segments
            .iter()
            .map(|s| {
                format!(
                    "[{}:{:02}] {}",
                    s.start_ms / 60000,
                    s.start_ms / 1000 % 60,
                    s.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => advanced::export(&m, &format)?,
    };
    let Some(path) = picker::save(&window, &format).await? else {
        return Ok(false);
    };
    use std::io::Write;
    let parent = path.parent().ok_or_else(|| store::io("path"))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(store::io)?;
    tmp.write_all(contents.as_bytes()).map_err(store::io)?;
    tmp.as_file().sync_all().map_err(store::io)?;
    tmp.persist(path).map_err(store::io)?;
    Ok(true)
}
#[tauri::command]
pub fn meeting_copy(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    transcript: bool,
) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    let m = store::read(&s.0.root, id)?;
    let value = if transcript {
        m.segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        m.text()
    };
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(value))
        .map_err(|_| err("Clipboard", "无法复制，请重试。"))
}
#[tauri::command]
pub async fn meeting_audio(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    start_ms: u64,
) -> Result<Vec<u8>> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let m = store::read(&s.0.root, id)?;
        playback::wav(&s.0.root, &m, start_ms)
    })
    .await
    .map_err(store::io)?
}

#[tauri::command]
pub async fn meeting_text_remove(window: WebviewWindow, state: State<'_, Managed>) -> Result<()> {
    crate::main_only(&window)?;
    let service = get(&state)?;
    let _guard = service.begin(None, "removing_model").await?;
    let path = text::model(&service.0.text_root);
    if path.exists() {
        std::fs::remove_file(path).map_err(store::io)?;
    }
    service.0.downloaded.store(0, Ordering::Release);
    service.changed();
    Ok(())
}

impl Service {
    pub(crate) fn recording_status(&self) -> Option<(Uuid, bool, u64)> {
        self.0
            .active
            .lock()
            .unwrap()
            .as_ref()
            .filter(|a| a.kind == "recording")
            .filter(|a| a.recording.lock().unwrap().is_some())
            .map(|a| {
                (
                    a.meeting.unwrap(),
                    a.paused.load(Ordering::Acquire),
                    a.elapsed.load(Ordering::Acquire),
                )
            })
    }
    pub(crate) fn menu_recording(&self, stop: bool) {
        if let Some(a) = self
            .0
            .active
            .lock()
            .unwrap()
            .as_ref()
            .filter(|a| a.kind == "recording")
        {
            if stop {
                a.stop.store(true, Ordering::Release);
            } else {
                a.paused.fetch_xor(true, Ordering::AcqRel);
            }
        }
        self.changed();
    }
}

fn meeting_recording_window_only(window: &WebviewWindow) -> Result<()> {
    if ["main", "meeting-hud"].contains(&window.label()) {
        Ok(())
    } else {
        Err(err("Forbidden", "当前窗口没有此操作权限。"))
    }
}

impl Service {
    fn recording_view(&self) -> Result<Option<MeetingRecordingView>> {
        let active = self.0.active.lock().unwrap().clone();
        let Some(a) = active.filter(|a| a.kind == "recording") else {
            return Ok(None);
        };
        let Some(meta) = a.recording.lock().unwrap().clone() else {
            return Ok(None);
        };
        let settings = self.0.runtime.snapshot()?.settings;
        Ok(Some(MeetingRecordingView {
            meeting_id: a.meeting.unwrap(),
            title: meta.title,
            source: meta.source,
            paused: a.paused.load(Ordering::Acquire),
            stopping: a.stop.load(Ordering::Acquire),
            elapsed_ms: a.elapsed.load(Ordering::Acquire),
            levels: [
                a.levels[0].load(Ordering::Acquire) as f32 / 1000.0,
                a.levels[1].load(Ordering::Acquire) as f32 / 1000.0,
            ],
            ui_locale: settings.ui_locale,
            motion_policy: settings.motion_policy,
        }))
    }
}

#[tauri::command]
pub fn meeting_recording_view(
    window: WebviewWindow,
    state: State<'_, Managed>,
) -> Result<Option<MeetingRecordingView>> {
    meeting_recording_window_only(&window)?;
    get(&state)?.recording_view()
}

#[tauri::command]
pub fn meeting_recording_control(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    action: String,
) -> Result<()> {
    meeting_recording_window_only(&window)?;
    let s = get(&state)?;
    let a = s
        .0
        .active
        .lock()
        .unwrap()
        .clone()
        .filter(|a| {
            a.kind == "recording" && a.meeting == Some(id) && a.recording.lock().unwrap().is_some()
        })
        .ok_or_else(|| {
            err(
                "MeetingRecordingEnded",
                "这场会议录音已结束，请打开会议查看。",
            )
        })?;
    if a.stop.load(Ordering::Acquire) {
        return Err(AppError::busy());
    }
    match action.as_str() {
        "pause" => a.paused.store(true, Ordering::Release),
        "resume" => a.paused.store(false, Ordering::Release),
        "stop" => a.stop.store(true, Ordering::Release),
        _ => return Err(AppError::invalid("当前操作不可用。")),
    }
    s.changed();
    Ok(())
}

#[tauri::command]
pub fn meeting_recording_note(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    text: String,
    kind: String,
    at_ms: u64,
) -> Result<u64> {
    meeting_recording_window_only(&window)?;
    let s = get(&state)?;
    let a = s
        .0
        .active
        .lock()
        .unwrap()
        .clone()
        .filter(|a| {
            a.kind == "recording" && a.meeting == Some(id) && a.recording.lock().unwrap().is_some()
        })
        .ok_or_else(|| {
            err(
                "MeetingRecordingEnded",
                "这场会议录音已结束，请打开会议查看。",
            )
        })?;
    s.edit(id, |m| {
        if a.stop.load(Ordering::Acquire)
            || a.cancel.load(Ordering::Acquire)
            || !["recording", "paused"].contains(&m.state.as_str())
        {
            return Err(err(
                "MeetingRecordingEnded",
                "这场会议录音已结束，请打开会议查看。",
            ));
        }
        experience::append_recording_note(m, text, kind, at_ms, a.elapsed.load(Ordering::Acquire))
    })
}

#[tauri::command]
pub fn meeting_recording_window(
    window: WebviewWindow,
    state: State<'_, Managed>,
    action: String,
    id: Option<Uuid>,
) -> Result<()> {
    meeting_recording_window_only(&window)?;
    let s = get(&state)?;
    if action == "ready" {
        if window.label() != "meeting-hud" {
            return Err(err("Forbidden", "当前窗口没有此操作权限。"));
        }
        return crate::meeting_panel::dispatch(&s.0.app, crate::meeting_panel::Action::Ready)
            .map_err(store::io);
    }
    let current = s.recording_view()?.ok_or_else(|| {
        err(
            "MeetingRecordingEnded",
            "这场会议录音已结束，请打开会议查看。",
        )
    })?;
    if id != Some(current.meeting_id) {
        return Err(AppError::conflict());
    }
    if action == "open" {
        crate::desktop_controls::show_main(&s.0.app);
        let _ =
            s.0.app
                .emit_to("main", "show_meeting_recording", current.meeting_id);
        return Ok(());
    }
    let action = match action.as_str() {
        "show" => crate::meeting_panel::Action::Show(current.meeting_id),
        "hide" => crate::meeting_panel::Action::Hide(current.meeting_id),
        "drag" => crate::meeting_panel::Action::Drag,
        "compose" => crate::meeting_panel::Action::Compose(current.meeting_id, true),
        "dismiss_note" => crate::meeting_panel::Action::Compose(current.meeting_id, false),
        _ => return Err(AppError::invalid("当前操作不可用。")),
    };
    crate::meeting_panel::dispatch(&s.0.app, action).map_err(|_| {
        err(
            "MeetingPanelUnavailable",
            "会议悬浮窗暂不可用，请在主窗口查看录音状态。",
        )
    })
}
#[tauri::command]
pub fn meeting_activity(
    window: WebviewWindow,
    state: State<'_, Managed>,
) -> Result<Option<ActiveView>> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    let result = s.0.active.lock().unwrap().as_ref().map(|a| ActiveView {
        id: a.id,
        meeting_id: a.meeting,
        kind: a.kind.clone(),
        paused: a.paused.load(Ordering::Acquire),
        elapsed_ms: a.elapsed.load(Ordering::Acquire),
        levels: [
            a.levels[0].load(Ordering::Acquire) as f32 / 1000.0,
            a.levels[1].load(Ordering::Acquire) as f32 / 1000.0,
        ],
    });
    Ok(result)
}
