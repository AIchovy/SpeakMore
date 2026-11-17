#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use speakmore_protocol::{Control, ModelFile, QualityFlag};
use speakmore_worker_supervisor::{
    Backend, Configuration, Event, Executable, ModelSpec, Phase, Snapshot, Supervisor,
    Transcription,
};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.cache/sensevoice/model")
        .canonicalize()
        .unwrap()
}
fn spec() -> ModelSpec {
    let lock: serde_json::Value = serde_json::from_str(include_str!(
        "../../../resources/sensevoice-fixtures.lock.json"
    ))
    .unwrap();
    let file = |name: &str| {
        let f = lock["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["filename"] == name)
            .unwrap();
        ModelFile {
            path: root().join(name).to_str().unwrap().into(),
            sha256: f["sha256"].as_str().unwrap().into(),
            size_bytes: f["size_bytes"].as_u64().unwrap(),
        }
    };
    let m = file("model.int8.onnx");
    let v = file("silero_vad_v5.onnx");
    ModelSpec {
        installation_id: "sensevoice-small-int8".into(),
        model_id: "sensevoice-small-int8".into(),
        engine_id: "sherpa-onnx".into(),
        profile_id: "sensevoice-v1".into(),
        bundle_sha256: "1".repeat(64),
        path: m.path,
        sha256: m.sha256,
        size_bytes: m.size_bytes,
        tokens: Some(file("tokens.txt")),
        auxiliary: Vec::new(),
        vad_path: v.path,
        vad_sha256: v.sha256,
        vad_size_bytes: v.size_bytes,
    }
}
fn worker() -> Supervisor {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_speakmore-sensevoice-worker"));
    Supervisor::new(Configuration {
        // This entry must never be chosen for a SenseVoice model.
        cpu: Executable {
            path: root().join("nonexistent-whisper-worker"),
            backend: Backend::Cpu,
        },
        accelerated: None,
        sensevoice: Some(Executable {
            path,
            backend: Backend::Cpu,
        }),
    })
    .unwrap()
}
fn until(
    worker: &mut Supervisor,
    seconds: u64,
    mut ready: impl FnMut(&Snapshot, &[Event]) -> bool,
) -> Vec<Event> {
    let started = Instant::now();
    let mut events = Vec::new();
    loop {
        events.extend(worker.poll());
        if ready(&worker.snapshot(), &events) {
            return events;
        }
        assert!(
            started.elapsed() < Duration::from_secs(seconds),
            "timeout {:?}, {events:?}",
            worker.snapshot()
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn speech(name: &str) -> Vec<f32> {
    hound::WavReader::open(root().join("test_wavs").join(name))
        .unwrap()
        .samples::<i16>()
        .map(|v| f32::from(v.unwrap()) / 32768.0)
        .collect()
}
fn submit(worker: &mut Supervisor, samples: Vec<f32>, lang: &str) -> Uuid {
    let id = Uuid::new_v4();
    worker
        .transcribe(Transcription {
            session_id: id,
            language: lang.into(),
            dictionary_revision: 17,
            dictionary_terms: vec!["SpeakMore".into()],
            samples,
        })
        .unwrap();
    id
}
fn complete(worker: &mut Supervisor, id: Uuid, expected: &str) {
    let events = until(worker, 60, |_, events| {
        events.iter().any(|e| {
            matches!(
                e,
                Event::Completed(_) | Event::Failed { .. } | Event::Cancelled { .. }
            )
        })
    });
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::Failed { .. } | Event::Cancelled { .. })),
        "{events:?}"
    );
    let results: Vec<_> = events
        .into_iter()
        .filter_map(|e| {
            if let Event::Completed(r) = e {
                Some(r)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(results.len(), 1);
    let r = &results[0];
    assert_eq!(r.session_id, id);
    assert!(
        r.text.to_lowercase().contains(expected),
        "Unexpected public fixture text: {}",
        r.text
    );
    assert_eq!(r.dictionary_revision, 17);
    assert_eq!((r.prompt_terms_used, r.count_omitted), (0, 1));
    assert_eq!(r.quality_flags, [QualityFlag::NeedsReview]);
    eprintln!(
        "SenseVoice process: {} ms, language={}, {} bytes",
        r.asr_ms,
        r.language,
        r.text.len()
    );
}
#[test]
#[ignore = "real pinned native models and public audio only; no microphone"]
fn real_sensevoice_process_speech_silence_cancel_recover_and_integrity() {
    let mut w = worker();
    w.set_model(spec()).unwrap();
    until(&mut w, 35, |s, _| s.phase == Phase::Ready);
    assert_eq!(w.snapshot().backend, Backend::Cpu);
    for (name, lang, expected) in [
        ("zh.wav", "zh", "9点"),
        ("en.wav", "en", "tribal"),
        ("yue.wav", "auto", ""),
        ("ja.wav", "ja", ""),
        ("ko.wav", "ko", ""),
    ] {
        let id = submit(&mut w, speech(name), lang);
        complete(&mut w, id, expected);
    }
    let id = submit(&mut w, vec![0.0; 4_800_000], "auto");
    let events = until(&mut w, 30, |_, ev| {
        ev.iter()
            .any(|e| matches!(e, Event::Failed { session_id, .. } if *session_id == id))
    });
    assert!(events.iter().any(|e| matches!(
        e,
        Event::Failed {
            error: speakmore_worker_supervisor::Error::NoSpeech,
            ..
        }
    )));
    assert!(!events.iter().any(|e| matches!(e, Event::Completed(_))));
    let long = speech("en.wav")
        .into_iter()
        .cycle()
        .take(4_800_000)
        .collect();
    let id = submit(&mut w, long, "en");
    until(&mut w, 30, |_, ev| {
        ev.iter()
            .any(|e| matches!(e, Event::Progress {stage, ..} if stage == "decoding"))
    });
    let started = Instant::now();
    assert!(w.cancel(id));
    let events = until(&mut w, 4, |_, ev| {
        ev.iter()
            .any(|e| matches!(e, Event::Cancelled {session_id} if *session_id == id))
    });
    assert!(!events.iter().any(|e| matches!(e, Event::Completed(_))));
    eprintln!(
        "SenseVoice blocking native decode cancelled/reaped in {} ms",
        started.elapsed().as_millis()
    );
    until(&mut w, 35, |s, _| s.phase == Phase::Ready);
    let id = submit(&mut w, speech("en.wav"), "en");
    complete(&mut w, id, "tribal");
    let mut damaged = spec();
    damaged.tokens.as_mut().unwrap().sha256 = "2".repeat(64);
    w.set_model(damaged).unwrap();
    let events = until(&mut w, 35, |s, ev| {
        s.phase == Phase::Ready && ev.iter().any(|e| matches!(e, Event::ModelFailed { .. }))
    });
    assert!(events.iter().any(|e| matches!(
        e,
        Event::ModelFailed {
            restoring_previous: true,
            ..
        }
    )));
    let id = submit(&mut w, speech("zh.wav"), "zh");
    complete(&mut w, id, "9点");
    w.shutdown();
    until(&mut w, 4, |s, _| s.phase == Phase::Stopped);
}

#[test]
fn incompatible_model_contract_is_rejected_before_sending() {
    let file = ModelFile {
        path: "/model.onnx".into(),
        sha256: "a".repeat(64),
        size_bytes: 1,
    };
    let mut spec = Control::LoadModel {
        request_id: Uuid::new_v4(),
        model_id: "sensevoice-small-int8".into(),
        engine_id: "sherpa-onnx".into(),
        profile_id: "sensevoice-v1".into(),
        bundle_sha256: "1".repeat(64),
        tokens: None,
        auxiliary: Vec::new(),
        path: file.path.clone(),
        sha256: file.sha256.clone(),
        size_bytes: 1,
        vad_path: file.path.clone(),
        vad_sha256: file.sha256.clone(),
        vad_size_bytes: 1,
    };
    assert!(speakmore_protocol::write_control(&mut Vec::new(), &spec).is_err());
    if let Control::LoadModel { tokens, .. } = &mut spec {
        *tokens = Some(Box::new(file));
    }
    assert!(speakmore_protocol::write_control(&mut Vec::new(), &spec).is_ok());
    if let Control::LoadModel { engine_id, .. } = &mut spec {
        *engine_id = "whisper.cpp".into();
    }
    assert!(speakmore_protocol::write_control(&mut Vec::new(), &spec).is_err());
}

#[test]
#[ignore = "real 300-second repeated public English speech, one worker result"]
fn real_sensevoice_worker_full_five_minute_speech() {
    let mut w = worker();
    w.set_model(spec()).unwrap();
    until(&mut w, 35, |s, _| s.phase == Phase::Ready);
    let mut phrase = speech("en.wav");
    phrase.extend(vec![0.0; 16_000]);
    let expected = 4_800_000 / phrase.len();
    let samples = phrase.iter().copied().cycle().take(4_800_000).collect();
    let id = submit(&mut w, samples, "en");
    let ev = until(&mut w, 90, |_, ev| {
        ev.iter()
            .any(|e| matches!(e, Event::Completed(_) | Event::Failed { .. }))
    });
    let results: Vec<_> = ev
        .iter()
        .filter_map(|e| {
            if let Event::Completed(r) = e {
                Some(r)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(results.len(), 1, "{ev:?}");
    let result = results[0];
    assert_eq!(result.session_id, id);
    let count = result.text.to_lowercase().matches("tribal").count();
    assert!((expected..=expected + 1).contains(&count));
    assert_eq!(result.quality_flags, [QualityFlag::NeedsReview]);
    eprintln!(
        "SenseVoice worker 300 s: {} ms, {} bytes, {count} phrases",
        result.asr_ms,
        result.text.len()
    );
    w.shutdown();
    until(&mut w, 4, |s, _| s.phase == Phase::Stopped);
}
