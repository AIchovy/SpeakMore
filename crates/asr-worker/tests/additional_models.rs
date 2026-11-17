#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use speakmore_protocol::{ModelAsset, ModelFile};
use speakmore_worker_supervisor::{
    Backend, Configuration, Event, Executable, ModelSpec, Phase, Supervisor, Transcription,
};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[test]
#[ignore = "one public sample per newly pinned model; requires test cache and packaged release worker"]
fn new_models_load_and_transcribe_public_samples() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let catalog: serde_json::Value = serde_json::from_str(include_str!(
        "../../../resources/model-catalog.payload.json"
    ))
    .unwrap();
    for id in ["parakeet-tdt-v3-int8", "qwen3-asr-0.6b-int8"] {
        let m = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["model_id"] == id)
            .unwrap();
        let directory = root.join(".cache/additional-asr").join(id);
        let artifacts = m["artifacts"].as_array().unwrap();
        let file = |a: &serde_json::Value| ModelFile {
            path: directory
                .join(a["filename"].as_str().unwrap())
                .to_str()
                .unwrap()
                .into(),
            sha256: a["sha256"].as_str().unwrap().into(),
            size_bytes: a["size_bytes"].as_u64().unwrap(),
        };
        let find = |role| file(artifacts.iter().find(|a| a["role"] == role).unwrap());
        let encoder = find("model");
        let vad = find("vad");
        let spec = ModelSpec {
            installation_id: id.into(),
            model_id: id.into(),
            engine_id: "sherpa-onnx".into(),
            profile_id: m["engine_profile"].as_str().unwrap().into(),
            bundle_sha256: "a".repeat(64),
            tokens: artifacts.iter().find(|a| a["role"] == "tokens").map(file),
            auxiliary: artifacts
                .iter()
                .filter(|a| !["model", "vad", "tokens"].contains(&a["role"].as_str().unwrap()))
                .map(|a| ModelAsset {
                    role: a["role"].as_str().unwrap().into(),
                    file: file(a),
                })
                .collect(),
            path: encoder.path,
            sha256: encoder.sha256,
            size_bytes: encoder.size_bytes,
            vad_path: vad.path,
            vad_sha256: vad.sha256,
            vad_size_bytes: vad.size_bytes,
        };
        let executable = Executable {
            path: root.join("resources/workers/speakmore-sensevoice-worker"),
            backend: Backend::Cpu,
        };
        let mut worker = Supervisor::new(Configuration {
            cpu: executable.clone(),
            accelerated: None,
            sensevoice: Some(executable),
        })
        .unwrap();
        worker.set_model(spec).unwrap();
        let start = Instant::now();
        while worker.snapshot().phase != Phase::Ready {
            let events = worker.poll();
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, Event::ModelFailed { .. })),
                "{id}: {events:?}"
            );
            assert!(
                start.elapsed() < Duration::from_secs(40),
                "{id}: loading timeout"
            );
            thread::sleep(Duration::from_millis(10));
        }
        let mut reader = hound::WavReader::open(directory.join("sample.wav")).unwrap();
        let format = reader.spec();
        let source: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.unwrap() as f32 / 32768.0)
            .collect();
        let mut dsp = speakmore_audio::dsp::Dsp::new(
            format.sample_rate,
            format.channels,
            speakmore_audio::dsp::Mix::Average,
        )
        .unwrap();
        dsp.push_interleaved(&source).unwrap();
        let audio = dsp.finish().unwrap().samples;
        let session_id = Uuid::new_v4();
        worker
            .transcribe(Transcription {
                session_id,
                samples: audio,
                language: "auto".into(),
                dictionary_revision: 1,
                dictionary_terms: vec!["SpeakMore".into()],
            })
            .unwrap();
        let start = Instant::now();
        loop {
            let events = worker.poll();
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, Event::Failed { .. } | Event::Cancelled { .. })),
                "{id}: {events:?}"
            );
            if let Some(result) = events.iter().find_map(|e| {
                if let Event::Completed(r) = e {
                    Some(r)
                } else {
                    None
                }
            }) {
                assert_eq!(result.session_id, session_id);
                assert!(!result.text.trim().is_empty());
                assert!(!result.text.contains("<|"));
                assert_eq!((result.prompt_terms_used, result.count_omitted), (0, 1));
                eprintln!(
                    "{id}: {} ms, {} UTF-8 bytes; public sample succeeded",
                    result.asr_ms,
                    result.text.len()
                );
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(60),
                "{id}: transcription timeout"
            );
            thread::sleep(Duration::from_millis(10));
        }
        worker.shutdown();
        let start = Instant::now();
        while worker.snapshot().phase != Phase::Stopped {
            worker.poll();
            assert!(start.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(10));
        }
    }
}
