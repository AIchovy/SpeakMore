//! Opt-in offline validation of an unpublished graph, through the real protocol,
//! supervisor, VAD and packaged adapters. Never accesses user data or a mic.
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use speakmore_models::catalog::{Model, Payload};
use speakmore_protocol::{
    read_frame, write_audio, write_control, Control, Frame, ModelFile, QualityFlag, MAX_SAMPLES,
    SENSEVOICE_PROFILE, SENSEVOICE_TILED_BYTES, SENSEVOICE_TILED_PROFILE, SENSEVOICE_TILED_SHA256,
    VERSION,
};
use speakmore_worker_supervisor::{
    Backend, Completed, Configuration, Error, Event, Executable, ModelSpec, Phase, Snapshot,
    Supervisor, Transcription,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}
fn model(id: &str) -> Model {
    let catalog: Payload = serde_json::from_slice(include_bytes!(
        "../../../resources/model-catalog.payload.json"
    ))
    .unwrap();
    catalog
        .models
        .into_iter()
        .find(|m| m.model_id == id)
        .unwrap()
}
fn spec(model: &Model, directory: &Path) -> ModelSpec {
    let file = |role: &str| {
        model
            .artifacts
            .iter()
            .find(|a| a.role == role)
            .map(|a| ModelFile {
                path: directory.join(&a.filename).to_str().unwrap().into(),
                sha256: a.sha256.clone(),
                size_bytes: a.size_bytes,
            })
    };
    let m = file("model").unwrap();
    let vad = file("vad").unwrap();
    ModelSpec {
        installation_id: model.installation_id(),
        model_id: model.model_id.clone(),
        engine_id: model.engine_id.clone(),
        profile_id: model.engine_profile.clone(),
        bundle_sha256: model.bundle_digest(),
        tokens: file("tokens"),
        auxiliary: Vec::new(),
        path: m.path,
        sha256: m.sha256,
        size_bytes: m.size_bytes,
        vad_path: vad.path,
        vad_sha256: vad.sha256,
        vad_size_bytes: vad.size_bytes,
    }
}
fn baseline() -> ModelSpec {
    spec(
        &model("sensevoice-small-int8"),
        &root().join(".cache/sensevoice/model"),
    )
}
fn candidate() -> (ModelSpec, Value) {
    let path = PathBuf::from(
        std::env::var_os("SPEAKMORE_TEST_TILED_REPORT")
            .expect("set SPEAKMORE_TEST_TILED_REPORT to the 256-row graph generation report"),
    );
    assert!(path.is_absolute());
    assert!(path.ancestors().all(|p| !p.is_symlink()));
    assert_eq!(
        path.parent().unwrap().parent().unwrap(),
        root().join(".cache/sensevoice/graph-study")
    );
    let raw = fs::read(&path).unwrap();
    let report: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(report["completed"], true);
    assert_eq!(report["block"], 256);
    assert_eq!(report["layers"], 70);
    assert_eq!(report["source_sha256"], baseline().sha256);
    assert_eq!(report["candidate_sha256"], SENSEVOICE_TILED_SHA256);
    assert_eq!(report["candidate_bytes"], SENSEVOICE_TILED_BYTES);
    let mut m = model("sensevoice-small-int8");
    m.version = Some("tiled256-v1".into());
    m.engine_profile = SENSEVOICE_TILED_PROFILE.into();
    let artifact = m.artifacts.iter_mut().find(|a| a.role == "model").unwrap();
    artifact.filename = "model.tiled.onnx".into();
    artifact.sha256 = SENSEVOICE_TILED_SHA256.into();
    artifact.size_bytes = SENSEVOICE_TILED_BYTES;
    // Synthetic metadata is only a test identity, never a published URL or a
    // trusted catalog. Real files are hash-checked by the production worker.
    artifact.url = format!(
        "https://huggingface.co/speakmore/test/resolve/{}/model.tiled.onnx",
        artifact.revision
    );
    let mut input = spec(&m, &root().join(".cache/sensevoice/model"));
    input.path = path
        .parent()
        .unwrap()
        .join("model.tiled.onnx")
        .to_str()
        .unwrap()
        .into();
    (
        input,
        json!({"report":path.strip_prefix(root()).unwrap(), "report_sha256":format!("{:x}", Sha256::digest(&raw)), "graph_sha256":SENSEVOICE_TILED_SHA256, "graph_bytes":SENSEVOICE_TILED_BYTES}),
    )
}
fn speech(name: &str) -> Vec<f32> {
    let name = format!("test_wavs/{name}.wav");
    let lock: Value = serde_json::from_slice(include_bytes!(
        "../../../resources/sensevoice-fixtures.lock.json"
    ))
    .unwrap();
    let record = lock["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["filename"] == name)
        .unwrap();
    let path = root().join(".cache/sensevoice/model").join(name);
    assert!(fs::symlink_metadata(&path).unwrap().is_file());
    let raw = fs::read(path).unwrap();
    assert_eq!(raw.len() as u64, record["size_bytes"].as_u64().unwrap());
    assert_eq!(
        format!("{:x}", Sha256::digest(&raw)),
        record["sha256"].as_str().unwrap()
    );
    let mut reader = hound::WavReader::new(raw.as_slice()).unwrap();
    assert_eq!(
        (
            reader.spec().sample_rate,
            reader.spec().channels,
            reader.spec().bits_per_sample
        ),
        (16000, 1, 16)
    );
    reader
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect()
}
fn long(names: &[&str]) -> Vec<f32> {
    let mut phrase = Vec::new();
    for name in names {
        phrase.extend(speech(name));
        phrase.extend([0.0; 16_000]);
    }
    phrase.iter().copied().cycle().take(MAX_SAMPLES).collect()
}
fn worker() -> Supervisor {
    Supervisor::new(Configuration {
        cpu: Executable {
            path: env!("CARGO_BIN_EXE_speakmore-asr-worker").into(),
            backend: Backend::Cpu,
        },
        accelerated: None,
        sensevoice: Some(Executable {
            path: env!("CARGO_BIN_EXE_speakmore-sensevoice-worker").into(),
            backend: Backend::Cpu,
        }),
    })
    .unwrap()
}
fn until(
    w: &mut Supervisor,
    seconds: u64,
    ready: impl Fn(&Snapshot, &[Event]) -> bool,
) -> Vec<Event> {
    let begin = Instant::now();
    let mut events = Vec::new();
    loop {
        events.extend(w.poll());
        if ready(&w.snapshot(), &events) {
            return events;
        }
        assert!(
            begin.elapsed() < Duration::from_secs(seconds),
            "deadline {:?}, {events:?}",
            w.snapshot()
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn ready(w: &mut Supervisor, spec: &ModelSpec) {
    let events = until(w, 35, |s, _| s.phase == Phase::Ready);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::ModelFailed { .. })),
        "{events:?}"
    );
    assert_eq!(
        w.snapshot().model_id.as_deref(),
        Some(spec.installation_id.as_str())
    );
    assert_eq!(w.snapshot().backend, Backend::Cpu);
}
fn submit(w: &mut Supervisor, samples: Vec<f32>, language: &str) -> Uuid {
    let id = Uuid::new_v4();
    w.transcribe(Transcription {
        session_id: id,
        samples,
        language: language.into(),
        dictionary_revision: 17,
        dictionary_terms: vec!["SpeakMore".into()],
    })
    .unwrap();
    id
}
fn complete(w: &mut Supervisor, id: Uuid) -> Completed {
    let events = until(w, 100, |_, ev| {
        ev.iter().any(|e| {
            matches!(
                e,
                Event::Completed(_) | Event::Failed { .. } | Event::Cancelled { .. }
            )
        })
    });
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::InputAccepted { session_id } if *session_id == id))
            .count(),
        1,
        "{events:?}"
    );
    assert!(events.iter().any(|e| matches!(e, Event::Progress { session_id, stage, .. } if *session_id == id && stage == "detecting_speech")));
    assert!(events.iter().any(|e| matches!(e, Event::Progress { session_id, stage, .. } if *session_id == id && stage == "decoding")));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::Failed { .. } | Event::Cancelled { .. })),
        "{events:?}"
    );
    let mut results = events.into_iter().filter_map(|e| {
        if let Event::Completed(r) = e {
            Some(r)
        } else {
            None
        }
    });
    let result = results.next().unwrap();
    assert!(results.next().is_none());
    assert_eq!(result.session_id, id);
    assert_eq!(result.dictionary_revision, 17);
    assert_eq!((result.prompt_terms_used, result.count_omitted), (0, 1));
    assert_eq!(result.quality_flags, [QualityFlag::NeedsReview]);
    result
}
fn run(w: &mut Supervisor, samples: Vec<f32>, language: &str) -> Completed {
    let id = submit(w, samples, language);
    complete(w, id)
}
fn stop(w: &mut Supervisor) {
    w.shutdown();
    until(w, 4, |s, _| s.phase == Phase::Stopped);
}
fn result_record(name: &str, r: &Completed) -> Value {
    json!({"case":name, "asr_ms":r.asr_ms, "text_bytes":r.text.len(), "text_sha256":format!("{:x}", Sha256::digest(r.text.as_bytes())), "language":r.language})
}
fn suite(w: &mut Supervisor) -> Vec<Completed> {
    let mut output = Vec::new();
    for (name, language) in [
        ("zh", "zh"),
        ("en", "en"),
        ("yue", "auto"),
        ("ja", "ja"),
        ("ko", "ko"),
    ] {
        let r = run(w, speech(name), language);
        eprintln!("{}", result_record(name, &r));
        output.push(r);
    }
    for (name, samples, language) in [
        ("en300", long(&["en"]), "en"),
        ("mixed300", long(&["zh", "en", "yue", "ja", "ko"]), "auto"),
    ] {
        assert_eq!(samples.len(), MAX_SAMPLES);
        let r = run(w, samples, language);
        if name == "en300" {
            assert_eq!(r.text.to_lowercase().matches("tribal").count(), 37);
        }
        eprintln!("{}", result_record(name, &r));
        output.push(r);
    }
    output
}

#[test]
#[ignore = "requires exported 256-row candidate and pinned real native workers/fixtures; no microphone"]
fn real_tiled_worker_vad_five_minutes_cancel_and_cross_family_recovery() {
    let (candidate, provenance) = candidate();
    let base = baseline();
    let parent = root().join(".cache/sensevoice/worker-study");
    fs::create_dir_all(&parent).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("run-")
        .tempdir_in(parent)
        .unwrap()
        .keep();
    let report_path = directory.join("report.json");
    let mut report = json!({"schema":1, "completed":false, "candidate":provenance, "profile":candidate.profile_id, "installation_id":candidate.installation_id, "baseline_profile":base.profile_id, "max_samples":MAX_SAMPLES, "scope":"real supervisor / IPC / VAD / CPU workers; public fixtures only; no GUI or microphone; no memory measurement"});
    fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    eprintln!("Worker evidence: {}", report_path.display());
    let mut w = worker();
    w.set_model(base.clone()).unwrap();
    ready(&mut w, &base);
    let expected = suite(&mut w);
    stop(&mut w);
    let mut w = worker();
    w.set_model(candidate.clone()).unwrap();
    ready(&mut w, &candidate);
    let actual = suite(&mut w);
    let names = ["zh", "en", "yue", "ja", "ko", "en300", "mixed300"];
    let mut comparisons = Vec::new();
    for ((name, expected), actual) in names.into_iter().zip(expected).zip(actual) {
        assert_eq!(actual.text, expected.text, "full text differs for {name}");
        assert_eq!(
            actual.language, expected.language,
            "language differs for {name}"
        );
        comparisons.push(json!({"baseline":result_record(name, &expected), "candidate":result_record(name, &actual), "text_and_language_identical":true}));
    }
    report["comparisons"] = json!(comparisons);
    // VAD receives the complete silence frame but must not invoke ASR.
    let id = submit(&mut w, vec![0.0; MAX_SAMPLES], "auto");
    let events = until(&mut w, 30, |_, ev| {
        ev.iter()
            .any(|e| matches!(e, Event::Failed { .. } | Event::Completed(_)))
    });
    assert_eq!(events.iter().filter(|e| matches!(e, Event::Failed {session_id, error:Error::NoSpeech} if *session_id == id)).count(), 1);
    assert!(!events.iter().any(|e| matches!(e, Event::Completed(_))
        || matches!(e, Event::Progress { stage, .. } if stage == "decoding")));
    report["silence300_no_speech"] = json!(true);
    let id = submit(&mut w, long(&["en"]), "en");
    until(&mut w, 15, |_, ev| {
        ev.iter()
            .any(|e| matches!(e, Event::Progress { stage, .. } if stage == "decoding"))
    });
    assert_eq!(w.set_model(base.clone()), Err(Error::Busy));
    let begin = Instant::now();
    assert!(w.cancel(id));
    let events = until(&mut w, 4, |_, ev| {
        ev.iter()
            .any(|e| matches!(e, Event::Cancelled {session_id} if *session_id == id))
    });
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::Cancelled { .. }))
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|e| matches!(e, Event::Completed(_) | Event::Failed { .. })));
    report["cancel_to_reaped_ms"] = json!(begin.elapsed().as_millis());
    ready(&mut w, &candidate);
    let recovered = run(&mut w, speech("en"), "en");
    assert!(recovered.text.to_lowercase().contains("tribal"));
    report["after_cancel"] = result_record("en", &recovered);
    // Syntactically valid metadata, damaged tokens: real file verification must
    // fail, unload the candidate and reload the previous verified installation.
    let mut bad = base.clone();
    bad.tokens.as_mut().unwrap().sha256 = "2".repeat(64);
    w.set_model(bad).unwrap();
    let events = until(&mut w, 35, |s, ev| {
        s.phase == Phase::Ready && ev.iter().any(|e| matches!(e, Event::ModelFailed { .. }))
    });
    assert!(events.iter().any(|e| matches!(e, Event::ModelFailed {model_id, restoring_previous:true, ..} if model_id == &base.installation_id)));
    assert_eq!(
        w.snapshot().model_id.as_deref(),
        Some(candidate.installation_id.as_str())
    );
    report["after_failed_switch"] = result_record("zh", &run(&mut w, speech("zh"), "zh"));
    w.set_model(base.clone()).unwrap();
    ready(&mut w, &base);
    report["after_legacy_switch"] = result_record("en", &run(&mut w, speech("en"), "en"));
    let whisper = spec(
        &model("whisper-small-q5_1"),
        &root().join(".cache/asr-fixtures"),
    );
    w.set_model(whisper.clone()).unwrap();
    ready(&mut w, &whisper);
    // Whisper has different prompt/quality semantics, inspect this result
    // separately instead of applying SenseVoice-only assertions.
    let id = submit(&mut w, speech("en"), "en");
    let events = until(&mut w, 60, |_, ev| {
        ev.iter()
            .any(|e| matches!(e, Event::Completed(_) | Event::Failed { .. }))
    });
    let results: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let Event::Completed(r) = e {
                Some(r)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(results.len(), 1, "{events:?}");
    assert_eq!(results[0].session_id, id);
    assert!(results[0].text.to_lowercase().contains("tribal"));
    report["after_whisper_switch"] = result_record("en", results[0]);
    w.set_model(candidate.clone()).unwrap();
    ready(&mut w, &candidate);
    report["after_return_to_candidate"] = result_record("en", &run(&mut w, speech("en"), "en"));
    stop(&mut w);
    report["completed"] = json!(true);
    fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    eprintln!("Completed worker evidence: {}", report_path.display());
}

// Raw protocol client is necessary to exercise a valid-but-wrong transcribe
// profile: the real supervisor deliberately always sends the loaded profile.
struct RawWorker {
    child: Child,
    input: ChildStdin,
    messages: mpsc::Receiver<Frame>,
}
impl RawWorker {
    fn new() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_speakmore-sensevoice-worker"))
            .arg("--cpu")
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        let (sender, messages) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(frame) = read_frame(&mut output) {
                if sender.send(frame).is_err() {
                    break;
                }
            }
        });
        let worker = Self {
            child,
            input,
            messages,
        };
        assert!(
            matches!(worker.next(), Control::Hello {protocol_version:VERSION, engine_id, ..} if engine_id == "sherpa-onnx")
        );
        worker
    }
    fn next(&self) -> Control {
        match self.messages.recv_timeout(Duration::from_secs(35)).unwrap() {
            Frame::Control(c) => c,
            _ => panic!("unexpected audio"),
        }
    }
    fn load(&mut self, m: &ModelSpec) {
        let request_id = Uuid::new_v4();
        write_control(
            &mut self.input,
            &Control::LoadModel {
                request_id,
                model_id: m.model_id.clone(),
                engine_id: m.engine_id.clone(),
                profile_id: m.profile_id.clone(),
                bundle_sha256: m.bundle_sha256.clone(),
                tokens: m.tokens.clone().map(Box::new),
                auxiliary: Vec::new(),
                path: m.path.clone(),
                sha256: m.sha256.clone(),
                size_bytes: m.size_bytes,
                vad_path: m.vad_path.clone(),
                vad_sha256: m.vad_sha256.clone(),
                vad_size_bytes: m.vad_size_bytes,
            },
        )
        .unwrap();
        assert!(
            matches!(self.next(), Control::ModelReady {request_id:id, profile_id, bundle_sha256, ..} if id == request_id && profile_id == m.profile_id && bundle_sha256 == m.bundle_sha256)
        );
    }
    fn check_request(&mut self, profile: &str, accepted: bool) {
        let id = Uuid::new_v4();
        let audio = speech("en");
        write_control(
            &mut self.input,
            &Control::Transcribe {
                session_id: id,
                profile_id: profile.into(),
                language: "en".into(),
                audio_samples: audio.len() as u32,
                dictionary_revision: 1,
                dictionary_terms: vec![],
            },
        )
        .unwrap();
        write_audio(&mut self.input, id, &audio).unwrap();
        assert!(
            matches!(self.next(), Control::InputAccepted {session_id, sample_count} if session_id == id && sample_count == audio.len() as u32)
        );
        if !accepted {
            assert!(
                matches!(self.next(), Control::Failed {session_id:Some(s), code, ..} if s == id && code == "InvalidTranscriptionProfile")
            );
            return;
        }
        loop {
            match self.next() {
                Control::Progress { session_id, .. } => assert_eq!(session_id, id),
                Control::Completed {
                    session_id, text, ..
                } => {
                    assert_eq!(session_id, id);
                    assert!(text.to_lowercase().contains("tribal"));
                    break;
                }
                other => panic!("unexpected public fixture response {other:?}"),
            }
        }
    }
}
impl Drop for RawWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "requires exported candidate; validates binding in real SenseVoice engine"]
fn real_tiled_worker_rejects_cross_profile_requests_without_losing_readiness() {
    let mut w = RawWorker::new();
    for (model, wrong) in [
        (candidate().0, SENSEVOICE_PROFILE),
        (baseline(), SENSEVOICE_TILED_PROFILE),
    ] {
        w.load(&model);
        w.check_request(wrong, false);
        w.check_request(&model.profile_id, true);
    }
}
