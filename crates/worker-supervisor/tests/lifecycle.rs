#![cfg(feature = "test-fixtures")]
use speakmore_worker_supervisor::*;
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

struct Fixture {
    dir: tempfile::TempDir,
    supervisor: Supervisor,
}
impl Fixture {
    fn new(mode: &str, gpu: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let filename = if cfg!(windows) {
            "speakmore-asr-worker.exe"
        } else {
            "speakmore-asr-worker"
        };
        let path = dir.path().join(filename);
        std::fs::copy(env!("CARGO_BIN_EXE_speakmore-supervisor-fixture"), &path).unwrap();
        std::fs::write(dir.path().join("fixture-mode"), mode).unwrap();
        let sensevoice_path = dir.path().join("speakmore-sensevoice-worker");
        std::fs::copy(
            env!("CARGO_BIN_EXE_speakmore-supervisor-fixture"),
            &sensevoice_path,
        )
        .unwrap();
        let mut supervisor = Supervisor::new(Configuration {
            sensevoice: Some(Executable {
                path: sensevoice_path,
                backend: Backend::Cpu,
            }),
            cpu: Executable {
                path: path.clone(),
                backend: Backend::Cpu,
            },
            accelerated: gpu.then_some(Executable {
                path,
                backend: Backend::Metal,
            }),
        })
        .unwrap();
        supervisor
            .set_model(model("whisper-small-q5_1", dir.path().into()))
            .unwrap();
        Self { dir, supervisor }
    }
    fn count(&self) -> u32 {
        std::fs::read_to_string(self.dir.path().join("spawn-count"))
            .unwrap()
            .parse()
            .unwrap()
    }
    fn set_mode(&self, value: &str) {
        std::fs::write(self.dir.path().join("fixture-mode"), value).unwrap();
    }
    fn until(
        &mut self,
        timeout: Duration,
        mut done: impl FnMut(&Snapshot, &[Event]) -> bool,
    ) -> Vec<Event> {
        let end = Instant::now() + timeout;
        let mut all = Vec::new();
        loop {
            all.extend(self.supervisor.poll());
            if done(&self.supervisor.snapshot(), &all) {
                return all;
            }
            assert!(
                Instant::now() < end,
                "deadline, phase={:?}, events={all:?}",
                self.supervisor.snapshot()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn ready(&mut self) {
        self.until(Duration::from_secs(5), |s, _| s.phase == Phase::Ready);
    }
    fn audio(&mut self, samples: usize) -> Uuid {
        let input = input(samples);
        let id = input.session_id;
        self.supervisor.transcribe(input).unwrap();
        id
    }
}
fn model(id: &str, path: PathBuf) -> ModelSpec {
    ModelSpec {
        installation_id: id.into(),
        engine_id: "whisper.cpp".into(),
        profile_id: "whisper-v1".into(),
        bundle_sha256: "1".repeat(64),
        tokens: None,
        auxiliary: Vec::new(),
        model_id: id.into(),
        path: path.join("model.bin").to_str().unwrap().into(),
        sha256: "a".repeat(64),
        size_bytes: 1,
        vad_path: path.join("vad.bin").to_str().unwrap().into(),
        vad_sha256: "b".repeat(64),
        vad_size_bytes: 1,
    }
}
fn input(samples: usize) -> Transcription {
    Transcription {
        session_id: Uuid::new_v4(),
        language: "en".into(),
        dictionary_revision: 7,
        dictionary_terms: vec!["SpeakMore".into()],
        samples: vec![0.0; samples],
    }
}

#[test]
fn complete_five_minute_audio_has_one_result_and_rejects_busy_and_stale_replay() {
    let mut f = Fixture::new("replay_old", false);
    f.ready();
    let id = f.audio(4_800_000);
    assert_eq!(f.supervisor.transcribe(input(4000)), Err(Error::Busy));
    let events = f.until(Duration::from_secs(8), |s, _| s.phase == Phase::Ready);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e,Event::Completed(c) if c.session_id==id))
            .count(),
        1
    );
    assert!(!f.supervisor.cancel(id));
    let second = f.audio(4000);
    let events = f.until(Duration::from_secs(5), |s, _| s.phase == Phase::Ready);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e,Event::Completed(c) if c.session_id==second))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Completed(_)))
            .count(),
        1
    );
    assert_eq!(f.count(), 1);
}

#[test]
fn cancellation_interrupts_a_blocked_large_audio_writer_and_recovers_without_replay() {
    let mut f = Fixture::new("transfer_stall", false);
    f.ready();
    let id = f.audio(4_800_000);
    for _ in 0..5 {
        f.supervisor.poll();
        thread::sleep(Duration::from_millis(10));
    }
    f.set_mode("normal");
    let start = Instant::now();
    assert!(f.supervisor.cancel(id));
    assert!(start.elapsed() < Duration::from_millis(50));
    let events = f.until(Duration::from_secs(3), |s, _| s.phase == Phase::Ready);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e,Event::Cancelled{session_id} if *session_id==id))
            .count(),
        1
    );
    assert!(!events.iter().any(|e| matches!(e, Event::Completed(_))));
    assert_eq!(f.count(), 2);
    assert_eq!(f.supervisor.snapshot().consecutive_failures, 0);
}

#[test]
fn cancelled_session_discards_late_success_and_worker_is_reusable() {
    let mut f = Fixture::new("late_completed", false);
    f.ready();
    let id = f.audio(4000);
    f.until(Duration::from_secs(3), |_, e| {
        e.iter().any(|e| matches!(e, Event::InputAccepted { .. }))
    });
    assert!(f.supervisor.cancel(id));
    let events = f.until(Duration::from_secs(3), |s, _| s.phase == Phase::Ready);
    assert!(matches!(events.as_slice(), [Event::Cancelled{session_id}] if *session_id==id));
    assert_eq!(f.count(), 1);
}

#[test]
fn worker_exit_after_user_cancel_is_not_a_fault() {
    let mut f = Fixture::new("cancel_exit", true);
    f.ready();
    // Three in a row previously tripped the breaker and dropped to CPU.
    for _ in 0..3 {
        let id = f.audio(4000);
        f.until(Duration::from_secs(3), |_, e| {
            e.iter().any(|e| matches!(e, Event::InputAccepted { .. }))
        });
        assert!(f.supervisor.cancel(id));
        let events = f.until(Duration::from_secs(3), |s, e| {
            s.phase == Phase::Ready && e.iter().any(|e| matches!(e, Event::Cancelled { .. }))
        });
        assert!(matches!(events.as_slice(), [Event::Cancelled{session_id}, ..] if *session_id==id));
        assert!(!events.iter().any(|e| matches!(e, Event::Failed { .. })));
        let snapshot = f.supervisor.snapshot();
        assert_eq!(snapshot.consecutive_failures, 0);
        assert_eq!(snapshot.backend, Backend::Metal);
    }
    assert_eq!(f.count(), 4);
}

#[test]
fn isolated_runtime_fault_keeps_acceleration_and_repeated_faults_try_cpu() {
    let mut f = Fixture::new("decode_crash", true);
    f.ready();
    let id = f.audio(4000);
    let events = f.until(Duration::from_secs(5), |s, e| {
        s.phase == Phase::Ready && e.iter().any(|e| matches!(e, Event::Failed { .. }))
    });
    assert!(events.iter().any(
        |e| matches!(e,Event::Failed{session_id,error:Error::WorkerUnavailable} if *session_id==id)
    ));
    assert_eq!(f.supervisor.snapshot().backend, Backend::Metal);
    assert_eq!(f.supervisor.snapshot().consecutive_failures, 1);
    f.audio(4000);
    f.until(Duration::from_secs(5), |s, e| {
        s.phase == Phase::Ready && e.iter().any(|e| matches!(e, Event::Failed { .. }))
    });
    assert_eq!(f.supervisor.snapshot().backend, Backend::Cpu);
    assert_eq!(f.count(), 3);
}

#[test]
fn shutdown_of_an_exited_worker_does_not_wait_for_the_force_reap() {
    let mut f = Fixture::new("normal", false);
    f.ready();
    let start = Instant::now();
    f.supervisor.shutdown();
    f.until(Duration::from_secs(3), |s, _| s.phase == Phase::Stopped);
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn crash_loop_pauses_after_three_spawns_and_only_explicit_reload_resumes() {
    for mode in ["crash", "ready_crash", "bad_build", "huge_audio_output"] {
        let mut f = Fixture::new(mode, false);
        f.until(Duration::from_secs(5), |s, _| s.phase == Phase::Paused);
        // Keep polling after the breaker; it must not create more processes.
        for _ in 0..10 {
            f.supervisor.poll();
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(f.count(), 3, "{mode}");
        f.set_mode("normal");
        f.supervisor.reload().unwrap();
        f.ready();
        assert_eq!(f.count(), 4);
    }
}

#[test]
fn transfer_timeout_does_not_accept_a_result_and_reloads_for_next_session() {
    let mut f = Fixture::new("transfer_stall", false);
    f.ready();
    let id = f.audio(4_800_000);
    f.set_mode("normal");
    let events = f.until(Duration::from_secs(8), |s, e| {
        s.phase == Phase::Ready && e.iter().any(|e| matches!(e, Event::Failed { .. }))
    });
    assert!(events.iter().any(
        |e| matches!(e,Event::Failed{session_id,error:Error::TransferTimeout} if *session_id==id)
    ));
    assert!(!events.iter().any(|e| matches!(e, Event::Completed(_))));
    assert_eq!(f.count(), 2);
}

#[test]
fn corrupt_job_metadata_cannot_be_delivered() {
    for mode in ["wrong_count", "wrong_revision", "before_ack"] {
        let mut f = Fixture::new(mode, false);
        f.ready();
        let id = f.audio(4000);
        let events = f.until(Duration::from_secs(3), |_, e| {
            e.iter().any(|e| matches!(e, Event::Failed { .. }))
        });
        assert!(
            events.iter().any(
                |e| matches!(e,Event::Failed{session_id,error:Error::Protocol} if *session_id==id)
            ),
            "{mode}: {events:?}"
        );
        assert!(!events.iter().any(|e| matches!(e, Event::Completed(_))));
    }
}

#[test]
fn failed_model_switch_restores_previous_model_without_two_processes() {
    let mut f = Fixture::new("switch_fail", false);
    f.ready();
    f.supervisor
        .set_model(model("whisper-large-v3-turbo-q5_0", f.dir.path().into()))
        .unwrap();
    let events = f.until(Duration::from_secs(4), |s, _| s.phase == Phase::Ready);
    assert!(events.iter().any(|e| matches!(
        e,
        Event::ModelFailed {
            restoring_previous: true,
            ..
        }
    )));
    assert_eq!(
        f.supervisor.snapshot().model_id.as_deref(),
        Some("whisper-small-q5_1")
    );
    assert_eq!(f.count(), 1);
}

#[test]
fn unavailable_acceleration_falls_back_to_cpu() {
    for mode in ["gpu_fail", "gpu_unavailable"] {
        let mut f = Fixture::new(mode, true);
        f.ready();
        assert_eq!(f.supervisor.snapshot().backend, Backend::Cpu);
        assert_eq!(f.count(), 2);
    }
}

#[test]
fn shutdown_reaps_a_worker_blocked_on_input_without_waiting_for_the_writer() {
    let mut f = Fixture::new("transfer_stall", false);
    f.ready();
    let id = f.audio(4_800_000);
    let start = Instant::now();
    f.supervisor.shutdown();
    assert!(start.elapsed() < Duration::from_millis(50));
    let events = f.until(Duration::from_secs(3), |s, _| s.phase == Phase::Stopped);
    assert!(events
        .iter()
        .any(|e| matches!(e,Event::Cancelled{session_id} if *session_id==id)));
    assert_eq!(f.count(), 1);
}

#[test]
fn inference_deadline_includes_transfer_and_never_retries_the_failed_audio() {
    let mut f = Fixture::new("decode_stall", false);
    f.ready();
    let id = f.audio(4000);
    f.set_mode("normal");
    let events = f.until(Duration::from_secs(19), |s, e| {
        s.phase == Phase::Ready && e.iter().any(|e| matches!(e, Event::Failed { .. }))
    });
    assert!(events.iter().any(
        |e| matches!(e,Event::Failed{session_id,error:Error::InferenceTimeout} if *session_id==id)
    ));
    assert!(!events.iter().any(|e| matches!(e, Event::Completed(_))));
    assert_eq!(f.count(), 2);
}

#[test]
fn startup_and_model_load_deadlines_are_enforced_by_the_parent() {
    let mut f = Fixture::new("hello_stall", false);
    f.until(Duration::from_secs(19), |s, _| s.phase == Phase::Paused);
    assert_eq!(f.count(), 3);
    let mut f = Fixture::new("model_stall", false);
    let events = f.until(Duration::from_secs(33), |_, e| {
        e.iter().any(|e| {
            matches!(
                e,
                Event::ModelFailed {
                    error: Error::LoadTimeout,
                    ..
                }
            )
        })
    });
    assert!(!events.iter().any(|e| matches!(e, Event::ModelReady { .. })));
}

#[test]
fn progress_pressure_is_bounded_and_keeps_the_supervisor_responsive() {
    let mut f = Fixture::new("progress_flood", false);
    f.ready();
    f.audio(4000);
    let events = f.until(Duration::from_secs(4), |_, e| {
        e.iter()
            .any(|e| matches!(e, Event::Completed(_) | Event::Failed { .. }))
    });
    assert!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Progress { .. }))
            .count()
            <= 5
    );
    // Overflow closes the untrusted pipe; a fast reader may instead consume the
    // flood successfully. Either outcome is bounded and no result is duplicated.
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Completed(_) | Event::Failed { .. }))
            .count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn dropping_the_supervisor_reaps_its_actual_child_without_another_poll() {
    let mut f = Fixture::new("decode_stall", false);
    f.ready();
    f.audio(4000);
    f.until(Duration::from_secs(3), |_, e| {
        e.iter().any(|e| matches!(e, Event::InputAccepted { .. }))
    });
    let pid = std::fs::read_to_string(f.dir.path().join("last-pid")).unwrap();
    drop(f.supervisor);
    let end = Instant::now() + Duration::from_secs(2);
    loop {
        let exists = std::process::Command::new("/bin/kill")
            .arg("-0")
            .arg(&pid)
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        if !exists {
            break;
        }
        assert!(
            Instant::now() < end,
            "dropped supervisor left a child or zombie"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn sensevoice_model(path: PathBuf) -> ModelSpec {
    let mut m = model("sensevoice-small-int8", path.clone());
    m.engine_id = "sherpa-onnx".into();
    m.profile_id = "sensevoice-v1".into();
    m.tokens = Some(speakmore_protocol::ModelFile {
        path: path.join("tokens.txt").to_str().unwrap().into(),
        sha256: "a".repeat(64),
        size_bytes: 1,
    });
    m
}
#[test]
fn versions_keep_native_model_identity_but_report_exact_installation_and_rollback() {
    let mut f = Fixture::new("version_fail", false);
    f.ready();
    let mut next = model("whisper-small-q5_1", f.dir.path().into());
    next.bundle_sha256 = "2".repeat(64);
    next.installation_id = format!("{}--{}", next.model_id, next.bundle_sha256);
    let mut invalid = next.clone();
    invalid.installation_id = "unrelated".into();
    assert_eq!(f.supervisor.set_model(invalid), Err(Error::InvalidRequest));
    f.supervisor.set_model(next.clone()).unwrap();
    let events = f.until(Duration::from_secs(5), |s, _| s.phase == Phase::Ready);
    assert_eq!(
        f.supervisor.snapshot().model_id.as_ref(),
        Some(&next.installation_id)
    );
    assert!(events.iter().any(
        |e| matches!(e, Event::ModelReady {model_id, ..} if model_id == &next.installation_id)
    ));
    // The fixture validates the wire frame against the exact logical model ID;
    // the installation suffix is never sent as a new native adapter identity.
    f.supervisor
        .set_model(model("whisper-small-q5_1", f.dir.path().into()))
        .unwrap();
    f.ready();
    std::fs::write(f.dir.path().join("rejected-bundle"), &next.bundle_sha256).unwrap();
    f.supervisor.set_model(next.clone()).unwrap();
    let events = f.until(Duration::from_secs(5), |s, es| {
        s.phase == Phase::Ready && es.iter().any(|e| matches!(e, Event::ModelFailed { .. }))
    });
    assert_eq!(
        f.supervisor.snapshot().model_id.as_deref(),
        Some("whisper-small-q5_1")
    );
    assert!(events.iter().any(|e| matches!(e, Event::ModelFailed {model_id, restoring_previous:true,..} if model_id == &next.installation_id)));
}
#[test]
fn switch_engine_retires_old_process_and_restores_the_right_executable() {
    let mut f = Fixture::new("normal", true);
    f.ready();
    assert_eq!(f.supervisor.snapshot().backend, Backend::Metal);
    f.supervisor
        .set_model(sensevoice_model(f.dir.path().into()))
        .unwrap();
    f.ready();
    assert_eq!(f.count(), 2);
    assert_eq!(f.supervisor.snapshot().backend, Backend::Cpu);
    assert_eq!(
        f.supervisor.snapshot().model_id.as_deref(),
        Some("sensevoice-small-int8")
    );
    f.supervisor
        .set_model(model("whisper-small-q5_1", f.dir.path().into()))
        .unwrap();
    f.ready();
    assert_eq!(f.count(), 3);
    assert_eq!(f.supervisor.snapshot().backend, Backend::Metal);
    f.set_mode("sensevoice_fail");
    f.supervisor
        .set_model(sensevoice_model(f.dir.path().into()))
        .unwrap();
    let events = f.until(Duration::from_secs(5), |s, events| {
        s.phase == Phase::Ready
            && events.iter().any(|e| {
                matches!(
                    e,
                    Event::ModelFailed {
                        restoring_previous: true,
                        ..
                    }
                )
            })
    });
    assert!(events.iter().any(
        |e| matches!(e, Event::ModelReady {model_id, ..} if model_id == "whisper-small-q5_1")
    ));
    assert_eq!(f.count(), 5);
    assert_eq!(f.supervisor.snapshot().backend, Backend::Metal);
}
#[test]
fn wrong_bundle_ready_never_activates_a_model() {
    let mut f = Fixture::new("wrong_bundle", false);
    let events = f.until(Duration::from_secs(5), |s, _| s.phase == Phase::Paused);
    assert!(events
        .iter()
        .all(|e| !matches!(e, Event::ModelReady { .. })));
    assert_eq!(f.count(), 3);
}

#[test]
fn wrong_engine_handshake_rolls_back_without_loading_into_the_wrong_process() {
    let mut f = Fixture::new("normal", true);
    f.ready();
    f.set_mode("wrong_engine");
    f.supervisor
        .set_model(sensevoice_model(f.dir.path().into()))
        .unwrap();
    let ev = f.until(Duration::from_secs(5), |s, ev| {
        s.phase == Phase::Ready
            && ev.iter().any(|e| {
                matches!(
                    e,
                    Event::ModelFailed {
                        restoring_previous: true,
                        ..
                    }
                )
            })
    });
    assert_eq!(
        f.supervisor.snapshot().model_id.as_deref(),
        Some("whisper-small-q5_1")
    );
    assert_eq!(f.supervisor.snapshot().backend, Backend::Metal);
    assert!(ev.iter().all(
        |e| !matches!(e, Event::ModelReady {model_id, ..} if model_id == "sensevoice-small-int8")
    ));
    assert_eq!(f.count(), 3);
}
