use speakmore_protocol::{
    read_frame, write_audio, write_control, Control, Frame, ENGINE_PROFILE, VERSION,
};
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[test]
#[ignore = "requires source-locked models; real parent supervisor + worker CPU/Metal"]
fn real_supervisor_roundtrip_cancellation_and_five_minute_silence() {
    use speakmore_worker_supervisor::{
        Backend, Configuration, Event, Executable, ModelSpec, Phase, Supervisor, Transcription,
    };
    fn until(
        supervisor: &mut Supervisor,
        seconds: u64,
        done: impl Fn(&speakmore_worker_supervisor::Snapshot, &[Event]) -> bool,
    ) -> Vec<Event> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let mut events = Vec::new();
        loop {
            events.extend(supervisor.poll());
            if done(&supervisor.snapshot(), &events) {
                return events;
            }
            assert!(
                Instant::now() < deadline,
                "supervisor deadline: {:?}, {events:?}",
                supervisor.snapshot()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
    let lock: serde_json::Value =
        serde_json::from_str(include_str!("../../../resources/model-sources.lock.json")).unwrap();
    let models = lock["models"].as_array().unwrap();
    let m = models
        .iter()
        .find(|m| m["model_id"] == "whisper-small-q5_1")
        .unwrap();
    let v = models
        .iter()
        .find(|m| m["model_id"] == "silero-vad")
        .unwrap();
    let spec = ModelSpec {
        installation_id: m["model_id"].as_str().unwrap().into(),
        engine_id: "whisper.cpp".into(),
        profile_id: "whisper-v1".into(),
        bundle_sha256: "1".repeat(64),
        tokens: None,
        auxiliary: Vec::new(),
        model_id: m["model_id"].as_str().unwrap().into(),
        path: fixtures()
            .join(m["filename"].as_str().unwrap())
            .to_str()
            .unwrap()
            .into(),
        sha256: m["sha256"].as_str().unwrap().into(),
        size_bytes: m["size_bytes"].as_u64().unwrap(),
        vad_path: fixtures()
            .join(v["filename"].as_str().unwrap())
            .to_str()
            .unwrap()
            .into(),
        vad_sha256: v["sha256"].as_str().unwrap().into(),
        vad_size_bytes: v["size_bytes"].as_u64().unwrap(),
    };
    let backends: &[Backend] = if cfg!(feature = "metal") {
        &[Backend::Cpu, Backend::Metal]
    } else {
        &[Backend::Cpu]
    };
    for &backend in backends {
        let path = PathBuf::from(env!("CARGO_BIN_EXE_speakmore-asr-worker"));
        let mut supervisor = Supervisor::new(Configuration {
            sensevoice: None,
            cpu: Executable {
                path: path.clone(),
                backend: Backend::Cpu,
            },
            accelerated: (backend == Backend::Metal).then_some(Executable {
                path,
                backend: Backend::Metal,
            }),
        })
        .unwrap();
        supervisor.set_model(spec.clone()).unwrap();
        let load_events = until(&mut supervisor, 35, |s, _| s.phase == Phase::Ready);
        assert_eq!(
            supervisor.snapshot().backend,
            backend,
            "accelerated test must actually load that backend; {load_events:?}"
        );
        let id = Uuid::new_v4();
        supervisor
            .transcribe(Transcription {
                session_id: id,
                samples: speech(),
                language: "en".into(),
                dictionary_revision: 3,
                dictionary_terms: vec!["Americans".into()],
            })
            .unwrap();
        let events = until(&mut supervisor, 30, |s, e| {
            s.phase == Phase::Ready && e.iter().any(|e| matches!(e, Event::Completed(_)))
        });
        let result = events
            .iter()
            .find_map(|e| {
                if let Event::Completed(c) = e {
                    Some(c)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(result.session_id, id);
        let normalized: String = result
            .text
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect();
        assert!(normalized.contains("asknotwhatyourcountrycandoforyou"));
        assert_eq!(
            (
                result.dictionary_revision,
                result.prompt_terms_used,
                result.count_omitted
            ),
            (3, 1, 0)
        );
        eprintln!(
            "Supervisor {:?}: public 11-second speech ASR {} ms, {} text bytes",
            backend,
            result.asr_ms,
            result.text.len()
        );

        let id = Uuid::new_v4();
        supervisor
            .transcribe(Transcription {
                session_id: id,
                samples: vec![0.0; 4_800_000],
                language: "auto".into(),
                dictionary_revision: 3,
                dictionary_terms: vec![],
            })
            .unwrap();
        let events = until(&mut supervisor, 30, |s, e| {
            s.phase == Phase::Ready && e.iter().any(|e| matches!(e, Event::Failed { .. }))
        });
        assert!(events.iter().any(|e| matches!(e,Event::Failed{session_id,error:speakmore_worker_supervisor::Error::NoSpeech} if *session_id==id)));
        assert!(!events.iter().any(|e| matches!(e, Event::Completed(_))));

        let id = Uuid::new_v4();
        let repeated = speech().into_iter().cycle().take(4_800_000).collect();
        supervisor
            .transcribe(Transcription {
                session_id: id,
                samples: repeated,
                language: "en".into(),
                dictionary_revision: 3,
                dictionary_terms: vec![],
            })
            .unwrap();
        until(&mut supervisor, 25, |_, e| {
            e.iter()
                .any(|e| matches!(e,Event::Progress{stage,..} if stage == "decoding"))
        });
        let started = Instant::now();
        assert!(supervisor.cancel(id));
        let events = until(&mut supervisor, 35, |s, e| {
            s.phase == Phase::Ready && e.iter().any(|e| matches!(e, Event::Cancelled { .. }))
        });
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e,Event::Cancelled{session_id} if *session_id==id))
                .count(),
            1
        );
        assert!(!events.iter().any(|e| matches!(e, Event::Completed(_))));
        eprintln!(
            "Supervisor {:?}: decode cancellation and readiness {} ms",
            backend,
            started.elapsed().as_millis()
        );
        supervisor.shutdown();
        until(&mut supervisor, 3, |s, _| s.phase == Phase::Stopped);
    }
}

struct Worker {
    child: Child,
    input: Option<ChildStdin>,
    messages: mpsc::Receiver<Frame>,
    stderr: Arc<Mutex<Vec<u8>>>,
}
impl Worker {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_speakmore-asr-worker"))
            .arg("--cpu")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let mut stdout = child.stdout.take().unwrap();
        let (sender, messages) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(frame) = read_frame(&mut stdout) {
                if sender.send(frame).is_err() {
                    return;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let captured = stderr.clone();
        let mut errors = child.stderr.take().unwrap();
        thread::spawn(move || {
            let mut block = [0; 4096];
            while let Ok(size) = errors.read(&mut block) {
                if size == 0 {
                    break;
                }
                let mut retained = captured.lock().unwrap();
                if retained.len() < 65536 {
                    retained.extend_from_slice(&block[..size]);
                }
            }
        });
        let worker = Self {
            child,
            input,
            messages,
            stderr,
        };
        assert!(
            matches!(worker.next(Duration::from_secs(5)), Control::Hello {protocol_version: VERSION, ref backend, ..} if backend == "cpu")
        );
        worker
    }
    fn send(&mut self, control: Control) {
        write_control(self.input.as_mut().unwrap(), &control).unwrap();
    }
    fn next(&self, timeout: Duration) -> Control {
        match self
            .messages
            .recv_timeout(timeout)
            .expect("worker response deadline exceeded or pipe closed")
        {
            Frame::Control(control) => control,
            _ => panic!("worker must not send audio"),
        }
    }
    fn exit_within(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "worker did not exit by deadline");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn load(&mut self, model_id: &str) {
        let lock: serde_json::Value =
            serde_json::from_str(include_str!("../../../resources/model-sources.lock.json"))
                .unwrap();
        let models = lock["models"].as_array().unwrap();
        let model = models.iter().find(|m| m["model_id"] == model_id).unwrap();
        let vad = models
            .iter()
            .find(|m| m["model_id"] == "silero-vad")
            .unwrap();
        let request_id = Uuid::new_v4();
        self.send(Control::LoadModel {
            engine_id: "whisper.cpp".into(),
            profile_id: "whisper-v1".into(),
            bundle_sha256: "1".repeat(64),
            tokens: None,
            auxiliary: Vec::new(),
            request_id,
            model_id: model_id.into(),
            path: fixtures()
                .join(model["filename"].as_str().unwrap())
                .to_str()
                .unwrap()
                .into(),
            sha256: model["sha256"].as_str().unwrap().into(),
            size_bytes: model["size_bytes"].as_u64().unwrap(),
            vad_path: fixtures()
                .join(vad["filename"].as_str().unwrap())
                .to_str()
                .unwrap()
                .into(),
            vad_sha256: vad["sha256"].as_str().unwrap().into(),
            vad_size_bytes: vad["size_bytes"].as_u64().unwrap(),
        });
        assert!(
            matches!(self.next(Duration::from_secs(35)), Control::ModelReady {request_id: id, ..} if id == request_id)
        );
    }
    fn audio(&mut self, samples: &[f32], language: &str) -> Uuid {
        let id = Uuid::new_v4();
        self.send(request(id, samples.len(), language));
        write_audio(self.input.as_mut().unwrap(), id, samples).unwrap();
        assert!(
            matches!(self.next(Duration::from_secs(6)), Control::InputAccepted {session_id, sample_count} if session_id == id && sample_count as usize == samples.len())
        );
        id
    }
    fn terminal(&self, id: Uuid, timeout: Duration) -> Control {
        let deadline = Instant::now() + timeout;
        loop {
            let control = self.next(deadline.saturating_duration_since(Instant::now()));
            if let Control::Progress { session_id, .. } = control {
                assert_eq!(session_id, id);
            } else {
                return control;
            }
        }
    }
    fn until_decoding(&self, id: Uuid) {
        loop {
            match self.next(Duration::from_secs(20)) {
                Control::Progress {
                    session_id, stage, ..
                } if session_id == id => {
                    if stage == "decoding" {
                        return;
                    }
                }
                _ => panic!("expected decoding stage before terminal result"),
            }
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn fixtures() -> PathBuf {
    std::env::var_os("SPEAKMORE_ASR_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.cache/asr-fixtures")
        })
        .canonicalize()
        .expect("run python3 scripts/prepare-asr-fixtures.py first")
}
fn speech() -> Vec<f32> {
    let reader =
        hound::WavReader::open(fixtures().join("jfk.wav")).expect("missing official audio fixture");
    assert_eq!(reader.spec().sample_rate, 16000);
    assert_eq!(reader.spec().channels, 1);
    reader
        .into_samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect()
}
fn request(session_id: Uuid, samples: usize, language: &str) -> Control {
    Control::Transcribe {
        session_id,
        profile_id: ENGINE_PROFILE.into(),
        language: language.into(),
        audio_samples: samples as u32,
        dictionary_revision: 3,
        dictionary_terms: vec!["Americans".into()],
    }
}
fn assert_speech(control: Control, id: Uuid) {
    let Control::Completed {
        session_id,
        text,
        language,
        asr_ms,
        dictionary_revision,
        prompt_terms_used,
        count_omitted,
        ..
    } = control
    else {
        panic!("expected actual ASR completion, got a failure");
    };
    assert_eq!(session_id, id);
    let normalized: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    assert!(
        normalized.contains("asknotwhatyourcountrycandoforyou"),
        "known public speech was not recognized"
    );
    assert_eq!(language, "en");
    assert_eq!(
        (dictionary_revision, prompt_terms_used, count_omitted),
        (3, 1, 0)
    );
    assert!(asr_ms > 0);
    println!("Real ASR: {asr_ms} ms; {} result bytes", text.len());
}

#[test]
fn handshake_not_ready_and_parent_eof_exit() {
    let mut worker = Worker::spawn();
    let id = Uuid::new_v4();
    worker.send(request(id, 16000, "en"));
    assert!(
        matches!(worker.next(Duration::from_secs(2)), Control::Failed {code, session_id: Some(session_id), ..} if code == "ModelUnavailable" && session_id == id)
    );
    drop(worker.input.take());
    assert!(worker.exit_within(Duration::from_secs(2)).success());
}

#[test]
fn unsolicited_audio_is_rejected_before_reading_payload() {
    let mut worker = Worker::spawn();
    let input = worker.input.as_mut().unwrap();
    input.write_all(&19_200_021u32.to_le_bytes()).unwrap();
    input.write_all(&[4]).unwrap();
    input.write_all(Uuid::new_v4().as_bytes()).unwrap();
    input.write_all(&4_800_000u32.to_le_bytes()).unwrap();
    input.flush().unwrap();
    // No PCM is sent and stdin stays open: rejecting cannot depend on payload EOF.
    assert_eq!(worker.exit_within(Duration::from_secs(2)).code(), Some(64));
}

#[test]
fn model_integrity_failure_keeps_worker_not_ready() {
    let mut worker = Worker::spawn();
    let mut model = tempfile::NamedTempFile::new().unwrap();
    model.write_all(b"corrupted model").unwrap();
    let request_id = Uuid::new_v4();
    worker.send(Control::LoadModel {
        engine_id: "whisper.cpp".into(),
        profile_id: "whisper-v1".into(),
        bundle_sha256: "1".repeat(64),
        tokens: None,
        auxiliary: Vec::new(),
        request_id,
        model_id: "whisper-small-q5_1".into(),
        path: model.path().to_str().unwrap().into(),
        sha256: "0".repeat(64),
        size_bytes: 15,
        vad_path: model.path().to_str().unwrap().into(),
        vad_sha256: "0".repeat(64),
        vad_size_bytes: 15,
    });
    assert!(
        matches!(worker.next(Duration::from_secs(2)), Control::Failed {request_id: Some(id), code, ..} if id == request_id && code == "ModelHashMismatch")
    );
    worker.send(request(Uuid::new_v4(), 16000, "en"));
    assert!(
        matches!(worker.next(Duration::from_secs(2)), Control::Failed {code, ..} if code == "ModelUnavailable")
    );
}

#[test]
#[ignore = "requires source-locked real models and public WAV fixture"]
fn real_asr_silence_cancellation_and_reuse() {
    let mut worker = Worker::spawn();
    worker.load("whisper-small-q5_1");
    let samples = speech();
    let id = worker.audio(&samples, "auto");
    assert_speech(worker.terminal(id, Duration::from_secs(40)), id);
    let id = worker.audio(&vec![0.0; 4_800_000], "en");
    assert!(
        matches!(worker.terminal(id, Duration::from_secs(40)), Control::Failed {session_id: Some(session_id), code, ..} if session_id == id && code == "NoSpeech")
    );
    let id = worker.audio(&samples, "en");
    worker.until_decoding(id);
    let started = Instant::now();
    worker.send(Control::Cancel {
        session_id: Uuid::new_v4(),
    });
    worker.send(Control::Cancel { session_id: id });
    assert!(
        matches!(worker.terminal(id, Duration::from_secs(2)), Control::Cancelled {session_id} if session_id == id)
    );
    println!("Decode cancellation: {} ms", started.elapsed().as_millis());
    // A late cancel from a prior job must not contaminate the next fresh state.
    worker.send(Control::Cancel { session_id: id });
    let next = worker.audio(&samples, "en");
    assert_speech(worker.terminal(next, Duration::from_secs(40)), next);
    assert!(
        worker.stderr.lock().unwrap().is_empty(),
        "native logging must be suppressed"
    );
    drop(worker.input.take());
    assert!(worker.exit_within(Duration::from_secs(2)).success());
}

#[test]
#[ignore = "requires source-locked real models and public WAV fixture"]
fn real_parent_eof_during_decode() {
    let mut worker = Worker::spawn();
    worker.load("whisper-small-q5_1");
    let id = worker.audio(&speech(), "en");
    worker.until_decoding(id);
    drop(worker.input.take());
    assert!(worker.exit_within(Duration::from_secs(2)).success());
}

#[test]
#[ignore = "requires source-locked real models; full 300-second inference"]
fn real_five_minute_speech() {
    let mut worker = Worker::spawn();
    worker.load("whisper-small-q5_1");
    let original = speech();
    let samples: Vec<f32> = original.iter().copied().cycle().take(4_800_000).collect();
    let id = worker.audio(&samples, "en");
    let result = worker.terminal(id, Duration::from_secs(606));
    if let Control::Completed { ref text, .. } = result {
        let lower = text.to_lowercase();
        assert!(
            lower.matches("country").count() >= 20,
            "long audio lost most repeated utterances"
        );
    }
    assert_speech(result, id);
}

#[test]
#[ignore = "requires source-locked turbo model and public WAV fixture"]
fn real_turbo_model() {
    let mut worker = Worker::spawn();
    worker.load("whisper-large-v3-turbo-q5_0");
    let id = worker.audio(&speech(), "en");
    assert_speech(worker.terminal(id, Duration::from_secs(40)), id);
}

#[test]
#[ignore = "requires source-locked models; real five-second transfer watchdog"]
fn real_truncated_audio_transfer_exits_without_accepting() {
    let mut worker = Worker::spawn();
    worker.load("whisper-small-q5_1");
    let session_id = Uuid::new_v4();
    worker.send(request(session_id, 4_800_000, "en"));
    let input = worker.input.as_mut().unwrap();
    input.write_all(&19_200_021u32.to_le_bytes()).unwrap();
    input.write_all(&[4]).unwrap();
    input.write_all(session_id.as_bytes()).unwrap();
    input.write_all(&4_800_000u32.to_le_bytes()).unwrap();
    input.write_all(&0f32.to_le_bytes()).unwrap();
    input.flush().unwrap();
    let started = Instant::now();
    assert_eq!(worker.exit_within(Duration::from_secs(6)).code(), Some(75));
    assert!(started.elapsed() >= Duration::from_secs(4));
    assert!(worker.messages.try_recv().is_err());
}

#[test]
#[ignore = "requires source-locked models; actual multilingual tokenizer budget"]
fn real_dictionary_budget_reports_omissions() {
    let mut worker = Worker::spawn();
    worker.load("whisper-small-q5_1");
    let samples = speech();
    let session_id = Uuid::new_v4();
    let mut declaration = request(session_id, samples.len(), "en");
    if let Control::Transcribe {
        dictionary_terms, ..
    } = &mut declaration
    {
        *dictionary_terms = (0..200)
            .map(|index| format!("工程词典候选专有名词{index}"))
            .collect();
    }
    worker.send(declaration);
    write_audio(worker.input.as_mut().unwrap(), session_id, &samples).unwrap();
    assert!(matches!(
        worker.next(Duration::from_secs(6)),
        Control::InputAccepted { .. }
    ));
    match worker.terminal(session_id, Duration::from_secs(40)) {
        Control::Completed {
            prompt_terms_used,
            count_omitted,
            ..
        } => {
            assert!(prompt_terms_used > 0 && prompt_terms_used < 200);
            assert_eq!(prompt_terms_used + count_omitted, 200);
        }
        _ => panic!("dictionary should not prevent ASR"),
    }
}

#[test]
#[ignore = "requires real models; stereo device-rate preprocessing into actual ASR"]
fn real_asr_after_device_rate_downmix_and_resampling() {
    use speakmore_audio::dsp::{Dsp, Mix};
    let source = speech();
    // Construct an independent 48 kHz stereo fixture from the public 16 kHz
    // source. No microphone is opened and no private audio is read or written.
    let device_pcm: Vec<f32> = source
        .iter()
        .flat_map(|sample| std::iter::repeat_n([*sample, *sample * 0.6], 3).flatten())
        .collect();
    let mut dsp = Dsp::new(48000, 2, Mix::Average).unwrap();
    for chunk in device_pcm.chunks(982) {
        dsp.push_interleaved(chunk).unwrap();
    }
    let processed = dsp.finish().unwrap();
    assert_eq!(processed.samples.len(), source.len());
    assert_eq!(processed.duration_ms, 11000);
    let mut worker = Worker::spawn();
    worker.load("whisper-small-q5_1");
    let id = worker.audio(&processed.samples, "en");
    assert_speech(worker.terminal(id, Duration::from_secs(40)), id);
}
