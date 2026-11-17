//! Parent-side ASR supervision. A single background SessionController actor owns
//! this driver and calls `poll` at least every 20 ms. No method waits on pipe I/O.
//! Executable and model paths are native-only inputs resolved from packaged
//! resources / the trusted ModelManager, never deserialized from a WebView.
mod platform;
mod transport;

use serde::Serialize;
use speakmore_protocol::{Control, ModelFile, QualityFlag, BUILD_ID, MAX_SAMPLES, VERSION};
use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{Duration, Instant},
};
use transport::{Incoming, Process};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    Cpu,
    Metal,
    Vulkan,
}
impl Backend {
    fn name(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Metal => "metal",
            Self::Vulkan => "vulkan",
        }
    }
}
#[derive(Clone)]
pub struct Executable {
    pub path: PathBuf,
    pub backend: Backend,
}
#[derive(Clone)]
pub struct Configuration {
    pub cpu: Executable,
    pub accelerated: Option<Executable>,
    pub sensevoice: Option<Executable>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Dormant,
    Starting,
    Loading,
    Ready,
    Transcribing,
    Cancelling,
    Recovering,
    Unavailable,
    Paused,
    ShuttingDown,
    Stopped,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub phase: Phase,
    pub model_id: Option<String>,
    pub backend: Backend,
    pub consecutive_failures: u8,
    pub session_id: Option<Uuid>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Busy")]
    Busy,
    #[error("NotReady")]
    NotReady,
    #[error("InvalidRequest")]
    InvalidRequest,
    #[error("WorkerUnavailable")]
    WorkerUnavailable,
    #[error("WorkerProtocolError")]
    Protocol,
    #[error("WorkerStartTimeout")]
    StartTimeout,
    #[error("ModelLoadTimeout")]
    LoadTimeout,
    #[error("AudioTransferTimeout")]
    TransferTimeout,
    #[error("AsrTimeout")]
    InferenceTimeout,
    #[error("ModelUnavailable")]
    ModelUnavailable,
    #[error("NoSpeech")]
    NoSpeech,
    #[error("ResultTooLong")]
    ResultTooLong,
    #[error("AsrFailed")]
    AsrFailed,
}
type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, PartialEq, Eq)]
pub struct ModelSpec {
    /// Catalog installation identity used by settings and lifecycle events.
    pub installation_id: String,
    /// Logical model identity sent to the fixed native adapter.
    pub model_id: String,
    pub engine_id: String,
    pub profile_id: String,
    pub bundle_sha256: String,
    pub tokens: Option<ModelFile>,
    pub auxiliary: Vec<speakmore_protocol::ModelAsset>,
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub vad_path: String,
    pub vad_sha256: String,
    pub vad_size_bytes: u64,
}
impl ModelSpec {
    fn control(&self, request_id: Uuid) -> Control {
        Control::LoadModel {
            request_id,
            model_id: self.model_id.clone(),
            engine_id: self.engine_id.clone(),
            profile_id: self.profile_id.clone(),
            bundle_sha256: self.bundle_sha256.clone(),
            tokens: self.tokens.clone().map(Box::new),
            auxiliary: self.auxiliary.clone(),
            path: self.path.clone(),
            sha256: self.sha256.clone(),
            size_bytes: self.size_bytes,
            vad_path: self.vad_path.clone(),
            vad_sha256: self.vad_sha256.clone(),
            vad_size_bytes: self.vad_size_bytes,
        }
    }
}
pub struct Transcription {
    pub session_id: Uuid,
    pub language: String,
    pub dictionary_revision: i64,
    pub dictionary_terms: Vec<String>,
    pub samples: Vec<f32>,
}
#[derive(Debug)]
pub struct Completed {
    pub session_id: Uuid,
    pub text: String,
    pub language: String,
    pub asr_ms: u64,
    pub dictionary_revision: i64,
    pub prompt_terms_used: u32,
    pub count_omitted: u32,
    pub quality_flags: Vec<QualityFlag>,
}
#[derive(Debug)]
pub enum Event {
    ModelReady {
        model_id: String,
        backend: Backend,
    },
    ModelFailed {
        model_id: String,
        error: Error,
        restoring_previous: bool,
    },
    InputAccepted {
        session_id: Uuid,
    },
    Progress {
        session_id: Uuid,
        stage: String,
        progress: Option<f32>,
    },
    Completed(Completed),
    Failed {
        session_id: Uuid,
        error: Error,
    },
    Cancelled {
        session_id: Uuid,
    },
}
#[derive(Clone, Copy)]
enum CancelReason {
    User,
    Timeout(Error),
}
struct Job {
    id: Uuid,
    samples: u32,
    dictionary_revision: i64,
    terms: u32,
    started: Instant,
    deadline: Duration,
    accepted: bool,
    cancel: Option<(Instant, CancelReason)>,
    progress_times: VecDeque<Instant>,
}

pub struct Supervisor {
    config: Configuration,
    process: Option<Process>,
    phase: Phase,
    backend: Backend,
    cpu_fallback: bool,
    desired: Option<ModelSpec>,
    loaded: Option<ModelSpec>,
    rollback: Option<ModelSpec>,
    load: Option<(Uuid, Instant)>,
    hello_deadline: Option<Instant>,
    active: Option<Job>,
    recent_sessions: VecDeque<Uuid>,
    failures: u8,
    retry_at: Instant,
    shutting_down: Option<Instant>,
    terminal_after_reap: Option<Event>,
    events: Vec<Event>,
}
impl Supervisor {
    pub fn new(config: Configuration) -> Result<Self> {
        if config.cpu.backend != Backend::Cpu
            || !config.cpu.path.is_absolute()
            || config
                .sensevoice
                .as_ref()
                .is_some_and(|e| e.backend != Backend::Cpu || !e.path.is_absolute())
            || config
                .accelerated
                .as_ref()
                .is_some_and(|e| e.backend == Backend::Cpu || !e.path.is_absolute())
        {
            return Err(Error::InvalidRequest);
        }
        Ok(Self {
            config,
            process: None,
            phase: Phase::Dormant,
            backend: Backend::Cpu,
            cpu_fallback: false,
            desired: None,
            loaded: None,
            rollback: None,
            load: None,
            hello_deadline: None,
            active: None,
            recent_sessions: VecDeque::new(),
            failures: 0,
            retry_at: Instant::now(),
            shutting_down: None,
            terminal_after_reap: None,
            events: Vec::new(),
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            phase: self.phase,
            model_id: self.loaded.as_ref().map(|m| m.installation_id.clone()),
            backend: self.backend,
            consecutive_failures: self.failures,
            session_id: self.active.as_ref().map(|a| a.id),
        }
    }
    pub fn set_model(&mut self, model: ModelSpec) -> Result<()> {
        if !matches!(
            self.phase,
            Phase::Dormant | Phase::Ready | Phase::Unavailable | Phase::Paused
        ) || self.process.as_ref().is_some_and(Process::is_retiring)
        {
            return Err(Error::Busy);
        }
        if (model.installation_id != model.model_id
            && model.installation_id != format!("{}--{}", model.model_id, model.bundle_sha256))
            || model.installation_id.len() > 120
            || !PathBuf::from(&model.path).is_absolute()
            || !PathBuf::from(&model.vad_path).is_absolute()
            || speakmore_protocol::write_control(&mut std::io::sink(), &model.control(Uuid::nil()))
                .is_err()
        {
            return Err(Error::InvalidRequest);
        }
        if model.engine_id == "sherpa-onnx" && self.config.sensevoice.is_none() {
            return Err(Error::WorkerUnavailable);
        }
        if self.loaded.as_ref() == Some(&model) {
            return Ok(());
        }
        // A different engine always gets a new, fixed executable. Keep the old
        // identity for rollback while the old process and pipes are reaped.
        let switch_engine = self
            .desired
            .as_ref()
            .is_some_and(|m| m.engine_id != model.engine_id);
        self.rollback = self.loaded.take();
        self.desired = Some(model);
        self.failures = 0;
        if switch_engine {
            self.cpu_fallback = false;
            self.retire();
            self.phase = Phase::Recovering;
            self.retry_at = Instant::now();
        } else if self.process.is_some() {
            self.begin_load();
        } else {
            self.phase = Phase::Starting;
            self.retry_at = Instant::now();
        }
        Ok(())
    }
    /// Explicit user reload also resets the crash circuit breaker.
    pub fn reload(&mut self) -> Result<()> {
        if self.active.is_some() || self.shutting_down.is_some() {
            return Err(Error::Busy);
        }
        if self.desired.is_none() {
            return Err(Error::NotReady);
        }
        self.failures = 0;
        self.cpu_fallback = false;
        self.rollback = None;
        self.retire();
        self.phase = Phase::Recovering;
        self.retry_at = Instant::now();
        Ok(())
    }
    pub fn transcribe(&mut self, input: Transcription) -> Result<()> {
        if self.active.is_some() {
            return Err(Error::Busy);
        }
        if self.phase != Phase::Ready {
            return Err(Error::NotReady);
        }
        if self.recent_sessions.contains(&input.session_id) {
            return Err(Error::InvalidRequest);
        }
        if !(4000..=MAX_SAMPLES).contains(&input.samples.len())
            || input.samples.iter().any(|s| !s.is_finite())
        {
            return Err(Error::InvalidRequest);
        }
        let count = input.samples.len() as u32;
        let control = Control::Transcribe {
            session_id: input.session_id,
            profile_id: self
                .loaded
                .as_ref()
                .ok_or(Error::NotReady)?
                .profile_id
                .clone(),
            language: input.language,
            audio_samples: count,
            dictionary_revision: input.dictionary_revision,
            dictionary_terms: input.dictionary_terms,
        };
        if speakmore_protocol::write_control(&mut std::io::sink(), &control).is_err() {
            return Err(Error::InvalidRequest);
        }
        let terms = if let Control::Transcribe {
            dictionary_terms, ..
        } = &control
        {
            dictionary_terms.len() as u32
        } else {
            unreachable!()
        };
        let now = Instant::now();
        self.active = Some(Job {
            id: input.session_id,
            samples: count,
            dictionary_revision: input.dictionary_revision,
            terms,
            started: now,
            deadline: Duration::from_millis(
                (5000 + 2 * (u64::from(count) / 16)).clamp(15000, 605000),
            ),
            accepted: false,
            cancel: None,
            progress_times: VecDeque::new(),
        });
        self.phase = Phase::Transcribing;
        if !self.send(control, Some(input.samples)) {
            self.fault(Error::WorkerUnavailable);
        }
        Ok(())
    }
    /// Marks cancellation before attempting to enqueue control behind a large Audio frame.
    pub fn cancel(&mut self, id: Uuid) -> bool {
        self.cancel_with(id, CancelReason::User)
    }
    fn cancel_with(&mut self, id: Uuid, reason: CancelReason) -> bool {
        let Some(job) = self.active.as_mut().filter(|j| j.id == id) else {
            return false;
        };
        if job.cancel.is_some() {
            return true;
        }
        job.cancel = Some((Instant::now(), reason));
        self.phase = Phase::Cancelling;
        let _ = self.send(Control::Cancel { session_id: id }, None);
        true
    }
    pub fn shutdown(&mut self) {
        if self.shutting_down.is_some() || self.phase == Phase::Stopped {
            return;
        }
        self.shutting_down = Some(Instant::now());
        if let Some(id) = self.active.as_ref().map(|j| j.id) {
            self.cancel(id);
        }
        let _ = self.send(
            Control::Shutdown {
                request_id: Uuid::new_v4(),
            },
            None,
        );
        self.phase = Phase::ShuttingDown;
    }
    /// Drives bounded message handling and monotonic deadlines. Call on the native
    /// actor thread, not the WebView/main event loop. Returned events belong only to
    /// the current worker instance; retired pipes are destroyed before a new spawn.
    pub fn poll(&mut self) -> Vec<Event> {
        // Reap as soon as the worker has exited; the 1 s bound is only for one that hangs.
        let exited =
            self.phase == Phase::ShuttingDown && self.process.as_mut().is_some_and(Process::broken);
        if exited
            || self
                .shutting_down
                .is_some_and(|at| at.elapsed() >= Duration::from_secs(1))
        {
            if let Some(job) = self.active.take() {
                self.remember(job.id);
                self.terminal_after_reap = Some(Self::cancelled_event(&job));
            }
            self.retire();
        }
        if matches!(
            self.phase,
            Phase::Recovering | Phase::Paused | Phase::ShuttingDown
        ) {
            let done = self.process.as_mut().is_none_or(Process::retired);
            if done {
                self.process = None;
                if let Some(event) = self.terminal_after_reap.take() {
                    self.events.push(event);
                }
                if self.shutting_down.is_some() {
                    self.phase = Phase::Stopped;
                } else if self.phase != Phase::Paused && Instant::now() >= self.retry_at {
                    self.phase = Phase::Starting;
                }
            }
            if self.phase != Phase::Starting {
                return std::mem::take(&mut self.events);
            }
        }
        if self.phase == Phase::Starting
            && self.process.is_none()
            && Instant::now() >= self.retry_at
        {
            let executable = if self
                .desired
                .as_ref()
                .is_some_and(|m| m.engine_id == "sherpa-onnx")
            {
                // Validated before changing selection; catalogs cannot supply executables.
                self.config
                    .sensevoice
                    .as_ref()
                    .expect("validated SenseVoice executable")
            } else if self.cpu_fallback {
                &self.config.cpu
            } else {
                self.config.accelerated.as_ref().unwrap_or(&self.config.cpu)
            };
            self.backend = executable.backend;
            match Process::spawn(executable) {
                Ok(process) => {
                    self.process = Some(process);
                    self.hello_deadline = Some(Instant::now() + Duration::from_secs(5));
                }
                Err(_) => self.fault(Error::WorkerUnavailable),
            }
        }
        // A single tick consumes at most 32 bounded control messages.
        for _ in 0..32 {
            if matches!(self.phase, Phase::Recovering | Phase::Paused) {
                break;
            }
            let incoming = self
                .process
                .as_ref()
                .and_then(|p| p.receiver.try_recv().ok());
            match incoming {
                Some(Incoming::Control(control)) => self.handle(*control),
                Some(Incoming::Broken) => {
                    self.fault(Error::WorkerUnavailable);
                    break;
                }
                None => break,
            }
        }
        if !matches!(
            self.phase,
            Phase::Recovering | Phase::Paused | Phase::Stopped | Phase::ShuttingDown
        ) && self.process.as_mut().is_some_and(Process::broken)
        {
            self.fault(Error::WorkerUnavailable);
        }
        if self.phase == Phase::Starting
            && self.hello_deadline.is_some_and(|at| Instant::now() >= at)
        {
            self.fault(Error::StartTimeout);
        }
        if self.phase == Phase::Loading
            && self
                .load
                .is_some_and(|(_, at)| at.elapsed() >= Duration::from_secs(30))
        {
            self.fault(Error::LoadTimeout);
        }
        if let Some(job) = &self.active {
            let id = job.id;
            if job.cancel.is_none() {
                if job.started.elapsed() >= job.deadline {
                    self.cancel_with(id, CancelReason::Timeout(Error::InferenceTimeout));
                } else if !job.accepted && job.started.elapsed() >= Duration::from_secs(5) {
                    self.cancel_with(id, CancelReason::Timeout(Error::TransferTimeout));
                }
            } else if job
                .cancel
                .is_some_and(|(at, _)| at.elapsed() >= Duration::from_secs(1))
            {
                let reason = job.cancel.unwrap().1;
                match reason {
                    CancelReason::User => self.recover_cancel(),
                    CancelReason::Timeout(error) => self.fault(error),
                }
            }
        }
        std::mem::take(&mut self.events)
    }
    fn send(&self, control: Control, audio: Option<Vec<f32>>) -> bool {
        self.process
            .as_ref()
            .is_some_and(|p| p.send(control, audio))
    }
    fn begin_load(&mut self) {
        let id = Uuid::new_v4();
        let model = self.desired.as_ref().unwrap();
        let command = model.control(id);
        self.loaded = None;
        self.load = Some((id, Instant::now()));
        self.phase = Phase::Loading;
        if !self.send(command, None) {
            self.fault(Error::WorkerUnavailable);
        }
    }
    fn retire(&mut self) {
        self.loaded = None;
        self.load = None;
        self.hello_deadline = None;
        if let Some(process) = &mut self.process {
            process.retire();
        }
    }
    fn fault(&mut self, error: Error) {
        // The worker exits on its own 1 s watchdog when a user cancel cannot
        // interrupt native work. That is the cancel completing, not a crash.
        if error == Error::WorkerUnavailable
            && self
                .active
                .as_ref()
                .is_some_and(|j| matches!(j.cancel, Some((_, CancelReason::User))))
        {
            self.recover_cancel();
            return;
        }
        if let Some(job) = self.active.take() {
            self.remember(job.id);
            self.terminal_after_reap = Some(if job.cancel.is_some() {
                Self::cancelled_event(&job)
            } else {
                Event::Failed {
                    session_id: job.id,
                    error,
                }
            });
        }
        let mut restored_other_engine = false;
        let loading = matches!(self.phase, Phase::Starting | Phase::Loading);
        if loading {
            let model_id = self.desired.as_ref().unwrap().installation_id.clone();
            // A GPU failure first retries the requested model on CPU. Keep the
            // prior model available for rollback if that CPU load also fails.
            let previous = if self.backend != Backend::Cpu {
                None
            } else {
                self.rollback.take()
            };
            self.events.push(Event::ModelFailed {
                model_id,
                error,
                restoring_previous: previous.is_some(),
            });
            if let Some(previous) = previous {
                restored_other_engine = self
                    .desired
                    .as_ref()
                    .is_some_and(|m| m.engine_id != previous.engine_id);
                self.desired = Some(previous);
            }
        }
        self.failures = self.failures.saturating_add(1);
        // A failed GPU start/load falls back at once. A single runtime fault
        // keeps acceleration; a second consecutive one tries CPU before Paused.
        if restored_other_engine {
            self.cpu_fallback = false;
        } else if self.backend != Backend::Cpu && (loading || self.failures >= 2) {
            self.cpu_fallback = true;
        }
        self.retire();
        self.phase = if self.failures >= 3 {
            Phase::Paused
        } else {
            Phase::Recovering
        };
        self.retry_at = Instant::now() + Duration::from_millis(200 * u64::from(self.failures));
    }
    fn recover_cancel(&mut self) {
        if let Some(job) = self.active.take() {
            self.remember(job.id);
            self.terminal_after_reap = Some(Self::cancelled_event(&job));
        }
        self.retire();
        self.phase = Phase::Recovering;
        self.retry_at = Instant::now();
    }
    fn remember(&mut self, id: Uuid) {
        self.recent_sessions.push_back(id);
        if self.recent_sessions.len() > 128 {
            self.recent_sessions.pop_front();
        }
    }
    fn cancelled_event(job: &Job) -> Event {
        match job.cancel.map(|(_, reason)| reason) {
            Some(CancelReason::Timeout(error)) => Event::Failed {
                session_id: job.id,
                error,
            },
            _ => Event::Cancelled { session_id: job.id },
        }
    }
    fn terminal(&mut self, event: Event) {
        let job = self.active.take().unwrap();
        self.remember(job.id);
        self.events.push(if job.cancel.is_some() {
            Self::cancelled_event(&job)
        } else {
            event
        });
        // Success at ModelReady alone never resets crash history: ready/crash loops
        // still hit the circuit breaker. A completed job proves actual reuse.
        self.failures = 0;
        self.phase = if self.shutting_down.is_some() {
            Phase::ShuttingDown
        } else {
            Phase::Ready
        };
    }
    fn handle(&mut self, control: Control) {
        // Ignore duplicate terminal/progress frames only for a known finished ID.
        let message_id = match &control {
            Control::InputAccepted { session_id, .. }
            | Control::Progress { session_id, .. }
            | Control::Completed { session_id, .. }
            | Control::Cancelled { session_id }
            | Control::Failed {
                session_id: Some(session_id),
                ..
            } => Some(*session_id),
            _ => None,
        };
        if message_id.is_some_and(|id| self.recent_sessions.contains(&id)) {
            return;
        }
        if let Some(id) = message_id {
            if self.active.as_ref().is_none_or(|j| j.id != id) {
                self.fault(Error::Protocol);
                return;
            }
            let job = self.active.as_ref().unwrap();
            // Deadline is checked before accepting a queued completion, not only
            // at the end of a tick. Late output cannot turn a timeout into success.
            if job.cancel.is_none() && job.started.elapsed() >= job.deadline {
                self.cancel_with(id, CancelReason::Timeout(Error::InferenceTimeout));
            } else if job.cancel.is_none()
                && !job.accepted
                && job.started.elapsed() >= Duration::from_secs(5)
            {
                self.cancel_with(id, CancelReason::Timeout(Error::TransferTimeout));
            }
        }
        match control {
            Control::Hello {
                protocol_version,
                build_id,
                backend,
                engine_id,
            } if self.phase == Phase::Starting
                && protocol_version == VERSION
                && build_id == BUILD_ID
                && self
                    .desired
                    .as_ref()
                    .is_some_and(|m| m.engine_id == engine_id)
                && backend == self.backend.name() =>
            {
                if self.hello_deadline.is_some_and(|at| Instant::now() >= at) {
                    self.fault(Error::StartTimeout);
                    return;
                }
                self.hello_deadline = None;
                self.begin_load();
            }
            Control::ModelReady {
                request_id,
                model_id,
                backend,
                engine_id,
                profile_id,
                bundle_sha256,
            } if self.phase == Phase::Loading
                && self.load.is_some_and(|(id, _)| id == request_id)
                && self.desired.as_ref().is_some_and(|m| {
                    m.model_id == model_id
                        && m.engine_id == engine_id
                        && m.profile_id == profile_id
                        && m.bundle_sha256 == bundle_sha256
                })
                && backend == self.backend.name() =>
            {
                if self.load.unwrap().1.elapsed() >= Duration::from_secs(30) {
                    self.fault(Error::LoadTimeout);
                    return;
                }
                self.loaded = self.desired.clone();
                self.rollback = None;
                self.load = None;
                self.phase = Phase::Ready;
                self.events.push(Event::ModelReady {
                    model_id: self.loaded.as_ref().unwrap().installation_id.clone(),
                    backend: self.backend,
                });
            }
            Control::Failed {
                session_id: None,
                request_id: Some(id),
                code,
            } if self.phase == Phase::Loading
                && self.load.is_some_and(|(request, _)| id == request) =>
            {
                if self.backend != Backend::Cpu
                    && ["ModelLoadFailed", "VadLoadFailed", "GpuUnavailable"]
                        .contains(&code.as_str())
                {
                    self.fault(Error::ModelUnavailable);
                    return;
                }
                let model_id = self.desired.as_ref().unwrap().installation_id.clone();
                let previous = self.rollback.take();
                self.events.push(Event::ModelFailed {
                    model_id,
                    error: Error::ModelUnavailable,
                    restoring_previous: previous.is_some(),
                });
                self.load = None;
                if let Some(previous) = previous {
                    let switch_engine = self
                        .desired
                        .as_ref()
                        .is_some_and(|m| m.engine_id != previous.engine_id);
                    self.desired = Some(previous);
                    if switch_engine {
                        self.cpu_fallback = false;
                        self.retire();
                        self.phase = Phase::Recovering;
                        self.retry_at = Instant::now();
                    } else {
                        self.begin_load();
                    }
                } else {
                    self.phase = Phase::Unavailable;
                }
            }
            Control::InputAccepted {
                session_id,
                sample_count,
            } => {
                let job = self.active.as_mut().unwrap();
                if job.accepted || job.samples != sample_count {
                    self.fault(Error::Protocol);
                    return;
                }
                job.accepted = true;
                if job.cancel.is_none() {
                    self.events.push(Event::InputAccepted { session_id });
                }
            }
            Control::Progress {
                session_id,
                stage,
                progress,
            } => {
                let job = self.active.as_mut().unwrap();
                if !job.accepted || !["detecting_speech", "decoding"].contains(&stage.as_str()) {
                    self.fault(Error::Protocol);
                    return;
                }
                while job
                    .progress_times
                    .front()
                    .is_some_and(|at| at.elapsed() >= Duration::from_secs(1))
                {
                    job.progress_times.pop_front();
                }
                if job.cancel.is_none() && job.progress_times.len() < 5 {
                    job.progress_times.push_back(Instant::now());
                    self.events.push(Event::Progress {
                        session_id,
                        stage,
                        progress,
                    });
                }
            }
            Control::Completed {
                session_id,
                text,
                language,
                asr_ms,
                dictionary_revision,
                prompt_terms_used,
                count_omitted,
                quality_flags,
            } => {
                let job = self.active.as_ref().unwrap();
                if !job.accepted
                    || job.dictionary_revision != dictionary_revision
                    || prompt_terms_used.checked_add(count_omitted) != Some(job.terms)
                    || language.is_empty()
                    || language.len() > 12
                    || !language
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b == b'-')
                    || asr_ms > job.deadline.as_millis() as u64
                    || quality_flags.len() > 1
                {
                    self.fault(Error::Protocol);
                    return;
                }
                self.terminal(Event::Completed(Completed {
                    session_id,
                    text,
                    language,
                    asr_ms,
                    dictionary_revision,
                    prompt_terms_used,
                    count_omitted,
                    quality_flags,
                }));
            }
            Control::Failed {
                session_id: Some(session_id),
                request_id: None,
                code,
            } => {
                if code == "GpuUnavailable" {
                    self.fault(Error::AsrFailed);
                    self.cpu_fallback |= self.backend != Backend::Cpu;
                    return;
                }
                let error = match code.as_str() {
                    "NoSpeech" => Error::NoSpeech,
                    "ResultTooLong" => Error::ResultTooLong,
                    speakmore_protocol::INFERENCE_TIMEOUT | "AsrTimeout" => Error::InferenceTimeout,
                    _ => Error::AsrFailed,
                };
                if !self.active.as_ref().unwrap().accepted {
                    self.fault(Error::Protocol);
                    return;
                }
                self.terminal(Event::Failed { session_id, error });
            }
            Control::Cancelled { session_id } => {
                if self.active.as_ref().unwrap().cancel.is_none() {
                    self.fault(Error::Protocol);
                    return;
                }
                self.terminal(Event::Cancelled { session_id });
            }
            _ => self.fault(Error::Protocol),
        }
    }
}
