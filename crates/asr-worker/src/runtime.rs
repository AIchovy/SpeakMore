use crate::{
    cancellation::{Cancellation, SHUTDOWN, TIMEOUT, USER},
    engine::Engine,
};
use speakmore_protocol::{read_frame_checked, write_control, Control, Frame, VERSION};
use std::{
    collections::VecDeque,
    io::{self, BufReader},
    sync::{
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

type Shared = Arc<Mutex<Status>>;

extern "C" {
    fn _exit(status: std::ffi::c_int) -> !;
}
/// Native inference threads may still be running. Skip atexit handlers and
/// C++ static destructors that would race them and turn an exit into a crash.
pub fn terminate(code: i32) -> ! {
    // SAFETY: plain libc process exit; no Rust state is touched afterwards.
    unsafe { _exit(code) }
}
#[derive(Clone)]
struct Output(SyncSender<Control>);
impl Output {
    fn send(&self, message: Control) {
        // The control reader and watchdog must not wait for a parent that has
        // stopped draining stdout. Bounded output also prevents result buildup.
        if self.0.try_send(message).is_err() {
            terminate(74);
        }
    }
    fn failed(&self, session_id: Option<Uuid>, request_id: Option<Uuid>, code: &'static str) {
        self.send(Control::Failed {
            session_id,
            request_id,
            code: code.into(),
        });
    }
}

enum Status {
    Empty,
    Loading {
        started: Instant,
        cancellation: Cancellation,
    },
    Ready,
    AwaitingAudio {
        request: Control,
        started: Instant,
    },
    Running {
        session_id: Uuid,
        started: Instant,
        deadline: Duration,
        cancellation: Cancellation,
        cancel_at: Option<Instant>,
    },
    ShuttingDown {
        started: Instant,
    },
}

enum Job {
    Load(Control, Cancellation),
    Decode {
        request: Control,
        samples: Vec<f32>,
        started: Instant,
        cancellation: Cancellation,
    },
}

pub fn run(gpu: bool, backend: &'static str) {
    let shared = Arc::new(Mutex::new(Status::Empty));
    let (outbound, messages) = mpsc::sync_channel(32);
    let output = Output(outbound);
    thread::spawn(move || {
        let mut writer = io::stdout().lock();
        while let Ok(message) = messages.recv() {
            if write_control(&mut writer, &message).is_err() {
                terminate(74);
            }
        }
    });
    let (jobs, receiver) = mpsc::sync_channel(1);
    let state = shared.clone();
    let result_writer = output.clone();
    thread::spawn(move || {
        let mut engine = None;
        while let Ok(job) = receiver.recv() {
            match job {
                Job::Load(spec, cancellation) => {
                    // A switch unloads first: never keep two ASR models alive.
                    engine = None;
                    let result = Engine::load(&spec, gpu, &cancellation);
                    let mut status = state.lock().unwrap();
                    if matches!(*status, Status::ShuttingDown { .. }) {
                        return;
                    }
                    let Control::LoadModel {
                        request_id,
                        model_id,
                        engine_id,
                        profile_id,
                        bundle_sha256,
                        ..
                    } = spec
                    else {
                        unreachable!()
                    };
                    match result {
                        Ok(loaded) => {
                            engine = Some(loaded);
                            *status = Status::Ready;
                            result_writer.send(Control::ModelReady {
                                request_id,
                                model_id,
                                backend: backend.into(),
                                engine_id,
                                profile_id,
                                bundle_sha256,
                            });
                        }
                        Err(code) => {
                            *status = Status::Empty;
                            result_writer.failed(None, Some(request_id), code);
                        }
                    }
                }
                Job::Decode {
                    request,
                    samples,
                    started,
                    cancellation,
                } => {
                    // Rate limiting belongs to this session. A fast preceding
                    // result must not suppress the next recording's stages.
                    let mut progress_times = VecDeque::<Instant>::new();
                    let Control::Transcribe { session_id, .. } = &request else {
                        unreachable!()
                    };
                    let id = *session_id;
                    let result = engine
                        .as_mut()
                        .ok_or("ModelUnavailable")
                        .and_then(|engine| {
                            engine.transcribe(&request, &samples, started, &cancellation, |stage| {
                                // Stages have no estimated percentage. Avoid blocking
                                // native callbacks on pipe writes; callbacks only abort.
                                while progress_times
                                    .front()
                                    .is_some_and(|at| at.elapsed() >= Duration::from_secs(1))
                                {
                                    progress_times.pop_front();
                                }
                                if progress_times.len() < 5 {
                                    result_writer.send(Control::Progress {
                                        session_id: id,
                                        stage: stage.into(),
                                        progress: None,
                                    });
                                    progress_times.push_back(Instant::now());
                                }
                            })
                        });
                    // Release audio before publishing the terminal outcome.
                    drop(samples);
                    let mut status = state.lock().unwrap();
                    if matches!(*status, Status::ShuttingDown { .. }) {
                        return;
                    }
                    if let Status::Running {
                        started, deadline, ..
                    } = &*status
                    {
                        if started.elapsed() >= *deadline {
                            cancellation.cancel(TIMEOUT);
                        }
                    }
                    let result = cancellation.check().and(result);
                    *status = Status::Ready;
                    match result {
                        Ok(completed) => result_writer.send(completed),
                        Err("Cancelled") => {
                            result_writer.send(Control::Cancelled { session_id: id })
                        }
                        Err(code) => result_writer.failed(Some(id), None, code),
                    }
                }
            }
        }
    });
    let watched = shared.clone();
    // Also works when stdin is blocked in a truncated Audio frame or the native
    // engine ignores cancellation. The parent's supervisor remains authoritative.
    thread::spawn(move || loop {
        thread::sleep(Duration::from_millis(20));
        let mut status = watched.lock().unwrap();
        match &mut *status {
            Status::Loading { started, .. } if started.elapsed() >= Duration::from_secs(30) => {
                terminate(75)
            }
            Status::AwaitingAudio { started, .. }
                if started.elapsed() >= Duration::from_secs(5) =>
            {
                terminate(75)
            }
            Status::Running {
                started,
                deadline,
                cancellation,
                cancel_at,
                ..
            } => {
                if started.elapsed() >= *deadline {
                    cancellation.cancel(TIMEOUT);
                    cancel_at.get_or_insert_with(Instant::now);
                }
                if cancel_at.is_some_and(|at| at.elapsed() >= Duration::from_secs(1)) {
                    terminate(75);
                }
            }
            Status::ShuttingDown { started } if started.elapsed() >= Duration::from_secs(1) => {
                terminate(0)
            }
            _ => {}
        }
    });
    output.send(Control::Hello {
        protocol_version: VERSION,
        engine_id: crate::engine::ENGINE_ID.into(),
        build_id: speakmore_protocol::BUILD_ID.into(),
        backend: backend.into(),
    });
    let mut input = BufReader::new(io::stdin());
    loop {
        let frame = read_frame_checked(&mut input, |id, count| {
            let state = shared.lock().unwrap();
            match &*state {
                Status::AwaitingAudio {
                    request:
                        Control::Transcribe {
                            session_id,
                            audio_samples,
                            ..
                        },
                    ..
                } if *session_id == id && *audio_samples as usize == count => Ok(()),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unexpected audio",
                )),
            }
        });
        match frame {
            Ok(Frame::Control(Control::Shutdown { .. })) => {
                shutdown(&shared);
                terminate(0);
            }
            Ok(frame) => {
                if !handle(frame, &shared, &jobs, &output) {
                    terminate(64);
                }
            }
            Err(err) => {
                shutdown(&shared);
                terminate(if err.kind() == io::ErrorKind::UnexpectedEof {
                    0
                } else {
                    64
                });
            }
        }
    }
}

fn enqueue(jobs: &SyncSender<Job>, job: Job) {
    // There is one active task. A full queue indicates an internal invariant
    // failure; never create a backlog of potentially large audio buffers.
    if jobs.try_send(job).is_err() {
        terminate(70);
    }
}

fn handle(frame: Frame, shared: &Shared, jobs: &SyncSender<Job>, output: &Output) -> bool {
    let mut status = shared.lock().unwrap();
    match frame {
        Frame::Control(spec @ Control::LoadModel { request_id, .. }) => {
            if !matches!(*status, Status::Empty | Status::Ready) {
                output.failed(None, Some(request_id), "Busy");
            } else {
                let cancellation = Cancellation::default();
                *status = Status::Loading {
                    started: Instant::now(),
                    cancellation: cancellation.clone(),
                };
                enqueue(jobs, Job::Load(spec, cancellation));
            }
        }
        Frame::Control(request @ Control::Transcribe { session_id, .. }) => {
            if matches!(*status, Status::Ready) {
                *status = Status::AwaitingAudio {
                    request,
                    started: Instant::now(),
                };
            } else {
                output.failed(
                    Some(session_id),
                    None,
                    if matches!(*status, Status::Empty) {
                        "ModelUnavailable"
                    } else {
                        "Busy"
                    },
                );
            }
        }
        Frame::Audio {
            session_id,
            samples,
        } => {
            let previous = std::mem::replace(&mut *status, Status::Empty);
            let Status::AwaitingAudio { request, started } = previous else {
                return false;
            };
            let Control::Transcribe {
                session_id: expected,
                audio_samples,
                ..
            } = &request
            else {
                return false;
            };
            if session_id != *expected || samples.len() != *audio_samples as usize {
                return false;
            }
            let duration_ms = samples.len() as u64 / 16;
            let cancellation = Cancellation::default();
            *status = Status::Running {
                session_id,
                started,
                deadline: Duration::from_millis((5000 + 2 * duration_ms).clamp(15000, 605000)),
                cancellation: cancellation.clone(),
                cancel_at: None,
            };
            output.send(Control::InputAccepted {
                session_id,
                sample_count: samples.len() as u32,
            });
            enqueue(
                jobs,
                Job::Decode {
                    request,
                    samples,
                    started,
                    cancellation,
                },
            );
        }
        Frame::Control(Control::Cancel { session_id }) => match &mut *status {
            Status::AwaitingAudio {
                request:
                    Control::Transcribe {
                        session_id: active, ..
                    },
                ..
            } if *active == session_id => {
                *status = Status::Ready;
                output.send(Control::Cancelled { session_id });
            }
            Status::Running {
                session_id: active,
                cancellation,
                cancel_at,
                ..
            } if *active == session_id => {
                cancellation.cancel(USER);
                cancel_at.get_or_insert_with(Instant::now);
            }
            _ => {} // Duplicate/late cancel never affects another session.
        },
        _ => return false, // Wrong direction or invalid ordering closes the pipe.
    }
    true
}

fn shutdown(shared: &Shared) {
    let mut status = shared.lock().unwrap();
    match &*status {
        Status::Running { cancellation, .. } | Status::Loading { cancellation, .. } => {
            cancellation.cancel(SHUTDOWN)
        }
        _ => {}
    }
    *status = Status::ShuttingDown {
        started: Instant::now(),
    };
    // The caller exits immediately, ending remaining threads; the watchdog is
    // an additional bound if shutdown later gains a graceful join path.
}
