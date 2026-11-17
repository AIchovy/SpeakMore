//! Explicit developer diagnostic. Uses existing packaged workers, no microphone,
//! application database, cloud service, text polishing or automatic input.
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use speakmore_models::catalog::Payload;
use speakmore_protocol::ModelFile;
use speakmore_worker_supervisor::{
    Backend, Configuration, Event, Executable, ModelSpec, Phase, Supervisor, Transcription,
};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
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
fn hash(raw: &[u8]) -> String {
    format!("{:x}", Sha256::digest(raw))
}
fn safe_cache(path: &Path) {
    assert!(path.is_absolute());
    assert!(path.starts_with(root().join(".cache/asr-quality")));
    assert!(path.ancestors().all(|p| !p.is_symlink()));
    assert!(!path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir)));
}
fn line(file: &mut File, row: Value) {
    serde_json::to_writer(&mut *file, &row).unwrap();
    file.write_all(b"\n").unwrap();
    file.sync_data().unwrap();
}
fn wait_ready(worker: &mut Supervisor) {
    let started = Instant::now();
    loop {
        for event in worker.poll() {
            assert!(!matches!(event, Event::ModelFailed { .. }), "{event:?}");
        }
        if worker.snapshot().phase == Phase::Ready {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(40));
        assert_ne!(worker.snapshot().phase, Phase::Unavailable);
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires a prepared public diagnostic corpus and cached packaged workers/models"]
fn transcribe_pinned_public_development_corpus() {
    let manifest = PathBuf::from(std::env::var_os("SPEAKMORE_QUALITY_MANIFEST").unwrap());
    let output = PathBuf::from(std::env::var_os("SPEAKMORE_QUALITY_OUTPUT").unwrap());
    safe_cache(&manifest);
    safe_cache(&output);
    let raw = fs::read(&manifest).unwrap();
    let corpus: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(corpus["purpose"], "public_development_diagnostic");
    let clips = corpus["clips"].as_array().unwrap();
    assert!(!clips.is_empty() && clips.len() <= 1000);
    let mut ids = HashSet::new();
    for clip in clips {
        assert!(ids.insert(clip["id"].as_str().unwrap()));
        let path = manifest
            .parent()
            .unwrap()
            .join(clip["wav"].as_str().unwrap());
        safe_cache(&path);
        assert_eq!(hash(&fs::read(path).unwrap()), clip["wav_sha256"]);
    }
    let whisper = root().join("resources/workers/speakmore-asr-worker");
    let sensevoice = root().join("resources/workers/speakmore-sensevoice-worker");
    let catalog_bytes = include_bytes!("../../../resources/model-catalog.payload.json");
    let catalog: Payload = serde_json::from_slice(catalog_bytes).unwrap();
    let requested = std::env::var("SPEAKMORE_QUALITY_MODELS")
        .unwrap_or_else(|_| "sensevoice-small-int8,whisper-small-q5_1".into());
    let model_ids: Vec<_> = requested.split(',').collect();
    let backend_name = std::env::var("SPEAKMORE_QUALITY_BACKEND").unwrap_or_else(|_| "cpu".into());
    let backend = match backend_name.as_str() {
        "cpu" => Backend::Cpu,
        "metal" => Backend::Metal,
        _ => panic!("SPEAKMORE_QUALITY_BACKEND must be cpu or metal"),
    };
    assert_eq!(
        model_ids.iter().collect::<HashSet<_>>().len(),
        model_ids.len()
    );
    for id in &model_ids {
        let model = catalog.models.iter().find(|m| m.model_id == *id).unwrap();
        assert!(
            backend == Backend::Cpu || model.family == "whisper",
            "Metal diagnostic requires Whisper; SenseVoice remains CPU"
        );
    }
    let mut results = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .unwrap();
    line(
        &mut results,
        json!({
            "type":"header", "schema":1, "purpose":corpus["purpose"],
            "manifest_sha256":hash(&raw), "catalog_sha256":hash(catalog_bytes),
            "whisper_worker_sha256":hash(&fs::read(&whisper).unwrap()),
            "sensevoice_worker_sha256":hash(&fs::read(&sensevoice).unwrap()),
            "models":model_ids, "clips":clips.len(), "backend":backend_name, "language":"auto",
            "dictionary_terms":[], "polishing":false, "automatic_input":false,
            "runner_sha256":hash(&fs::read(std::env::current_exe().unwrap()).unwrap())
        }),
    );
    for model_id in model_ids {
        let model = catalog
            .models
            .iter()
            .find(|m| m.model_id == model_id)
            .unwrap();
        let directory = root().join(if model.family == "sensevoice" {
            ".cache/sensevoice/model"
        } else {
            ".cache/asr-fixtures"
        });
        let file = |role: &str| {
            model.artifacts.iter().find(|a| a.role == role).map(|a| {
                let path = directory.join(&a.filename);
                let bytes = fs::read(&path).unwrap();
                assert_eq!(bytes.len() as u64, a.size_bytes);
                assert_eq!(hash(&bytes), a.sha256);
                ModelFile {
                    path: path.to_str().unwrap().into(),
                    sha256: a.sha256.clone(),
                    size_bytes: a.size_bytes,
                }
            })
        };
        let main = file("model").unwrap();
        let vad = file("vad").unwrap();
        let spec = ModelSpec {
            installation_id: model.installation_id(),
            model_id: model.model_id.clone(),
            engine_id: model.engine_id.clone(),
            profile_id: model.engine_profile.clone(),
            bundle_sha256: model.bundle_digest(),
            tokens: file("tokens"),
            auxiliary: model
                .artifacts
                .iter()
                .filter(|a| !["model", "tokens", "vad"].contains(&a.role.as_str()))
                .map(|a| speakmore_protocol::ModelAsset {
                    role: a.role.clone(),
                    file: file(&a.role).unwrap(),
                })
                .collect(),
            path: main.path,
            sha256: main.sha256,
            size_bytes: main.size_bytes,
            vad_path: vad.path,
            vad_sha256: vad.sha256,
            vad_size_bytes: vad.size_bytes,
        };
        let mut worker = Supervisor::new(Configuration {
            cpu: Executable {
                path: whisper.clone(),
                backend: Backend::Cpu,
            },
            accelerated: (backend == Backend::Metal).then_some(Executable {
                path: whisper.clone(),
                backend: Backend::Metal,
            }),
            sensevoice: Some(Executable {
                path: sensevoice.clone(),
                backend: Backend::Cpu,
            }),
        })
        .unwrap();
        let loading = Instant::now();
        worker.set_model(spec).unwrap();
        wait_ready(&mut worker);
        // A production fallback is valid behavior, but cannot be reported as
        // a successful measurement of the requested acceleration backend.
        assert_eq!(worker.snapshot().backend, backend, "backend fell back");
        line(
            &mut results,
            json!({"type":"model", "model_id":model_id,
            "profile":model.engine_profile, "bundle_sha256":model.bundle_digest(),
            "verified_backend":backend_name,
            "load_wall_ms":loading.elapsed().as_millis()}),
        );
        for (index, clip) in clips.iter().enumerate() {
            wait_ready(&mut worker);
            assert_eq!(worker.snapshot().backend, backend, "backend changed");
            let path = manifest
                .parent()
                .unwrap()
                .join(clip["wav"].as_str().unwrap());
            // Recheck immediately before reading, so edits after preflight fail.
            let bytes = fs::read(path).unwrap();
            assert_eq!(hash(&bytes), clip["wav_sha256"]);
            let mut wav = hound::WavReader::new(bytes.as_slice()).unwrap();
            let format = wav.spec();
            assert_eq!(
                (format.sample_rate, format.channels, format.bits_per_sample),
                (16000, 1, 16)
            );
            assert_eq!(format.sample_format, hound::SampleFormat::Int);
            let samples: Vec<_> = wav
                .samples::<i16>()
                .map(|s| f32::from(s.unwrap()) / 32768.0)
                .collect();
            assert_eq!(samples.len() as u64, clip["samples"].as_u64().unwrap());
            let id = Uuid::new_v4();
            let begin = Instant::now();
            worker
                .transcribe(Transcription {
                    session_id: id,
                    language: "auto".into(),
                    dictionary_revision: 1,
                    dictionary_terms: vec![],
                    samples,
                })
                .unwrap();
            let mut accepted = 0;
            let mut stages = HashSet::new();
            let mut outcome = None;
            while outcome.is_none() {
                for event in worker.poll() {
                    match event {
                        Event::InputAccepted { session_id } => {
                            assert_eq!(session_id, id);
                            accepted += 1;
                        }
                        Event::Progress {
                            session_id, stage, ..
                        } => {
                            assert_eq!(session_id, id);
                            stages.insert(stage);
                        }
                        Event::Completed(result) => {
                            assert_eq!(result.session_id, id);
                            assert_eq!(result.dictionary_revision, 1);
                            assert_eq!((result.prompt_terms_used, result.count_omitted), (0, 0));
                            assert!(outcome.is_none());
                            outcome = Some(json!({"status":"completed", "text":result.text,
                                "detected_language":result.language, "asr_ms":result.asr_ms,
                                "quality_flags":result.quality_flags}));
                        }
                        Event::Failed { session_id, error } => {
                            assert_eq!(session_id, id);
                            assert!(outcome.is_none());
                            outcome = Some(
                                json!({"status":"failed", "error":error.to_string(), "text":""}),
                            );
                        }
                        Event::Cancelled { session_id } => {
                            assert_eq!(session_id, id);
                            assert!(outcome.is_none());
                            outcome = Some(json!({"status":"cancelled", "text":""}));
                        }
                        _ => {}
                    }
                }
                assert!(begin.elapsed() < Duration::from_secs(620));
                if outcome.is_none() {
                    thread::sleep(Duration::from_millis(5));
                }
            }
            let mut row = outcome.unwrap();
            row["type"] = json!("result");
            row["id"] = clip["id"].clone();
            row["model_id"] = json!(model_id);
            row["wall_ms"] = json!(begin.elapsed().as_millis());
            row["accepted_events"] = json!(accepted);
            let mut stages: Vec<_> = stages.into_iter().collect();
            stages.sort();
            row["stages"] = json!(stages);
            line(&mut results, row);
            if (index + 1) % 10 == 0 {
                println!("{model_id}: {}/{}", index + 1, clips.len());
            }
        }
        worker.shutdown();
        let begin = Instant::now();
        while worker.snapshot().phase != Phase::Stopped {
            worker.poll();
            assert!(begin.elapsed() < Duration::from_secs(5));
            thread::sleep(Duration::from_millis(5));
        }
    }
    line(&mut results, json!({"type":"complete"}));
}
