use speakmore_core::{
    error::AppError,
    storage::{ModelInstall, Store},
};
use speakmore_models::{InstalledBundle, ModelError, Registry, Result};
use std::sync::{Arc, Mutex};

#[cfg(test)]
mod fault_tests;

pub struct SqliteModels {
    pub store: Arc<Mutex<std::result::Result<Store, AppError>>>,
    pub changed: Box<dyn Fn() + Send + Sync>,
}
impl Registry for SqliteModels {
    fn installed(&self, bundle: &InstalledBundle) -> Result<()> {
        {
            let mut store = self.store.lock().map_err(|_| ModelError::Registration)?;
            let store = store.as_mut().map_err(|_| ModelError::Registration)?;
            store
                .register_model(&ModelInstall {
                    model_id: bundle.model.installation_id(),
                    revision: bundle.model.revision.clone(),
                    bundle_sha256: bundle.model.bundle_digest(),
                    size_bytes: bundle.model.size_bytes(),
                    local_path: bundle.directory.to_str().ok_or(ModelError::Storage)?.into(),
                    verified_at: bundle.verified_at,
                })
                .map_err(|_| ModelError::Registration)?;
        }
        (self.changed)();
        Ok(())
    }
    fn removed(&self, model_id: &str) -> Result<()> {
        {
            let mut store = self.store.lock().map_err(|_| ModelError::Registration)?;
            let store = store.as_mut().map_err(|_| ModelError::Registration)?;
            store
                .forget_model(model_id, super::now())
                .map_err(|_| ModelError::Registration)?;
        }
        (self.changed)();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use speakmore_models::{
        catalog::{TrustKey, TrustedCatalog},
        InstallState, ModelManager,
    };
    use std::{
        path::Path,
        sync::atomic::{AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    #[tokio::test]
    async fn app_catalog_upgrade_preserves_sqlite_selected_old_bundle_without_network() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        use ed25519_dalek::{Signer, SigningKey};
        use sha2::{Digest, Sha256};
        use speakmore_models::catalog::{Envelope, Payload};
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("models.sqlite3");
        let store = Arc::new(Mutex::new(Ok(Store::open(&db).unwrap())));
        let registry = Arc::new(SqliteModels {
            store: store.clone(),
            changed: Box::new(|| {}),
        });
        let mut source: Payload =
            serde_json::from_slice(include_bytes!("../../resources/model-catalog.payload.json"))
                .unwrap();
        source.models.truncate(1);
        for a in &mut source.models[0].artifacts {
            a.size_bytes = 4;
            a.sha256 = format!("{:x}", Sha256::digest(b"test"));
        }
        let sign = |payload: &Payload| {
            let raw = serde_json::to_vec(payload).unwrap();
            let key = SigningKey::from_bytes(&[81; 32]);
            let envelope = Envelope {
                key_id: "upgrade-fixture".into(),
                payload: STANDARD.encode(&raw),
                signature: STANDARD.encode(key.sign(&raw).to_bytes()),
            };
            TrustedCatalog::verify(
                &serde_json::to_vec(&envelope).unwrap(),
                &TrustKey {
                    key_id: envelope.key_id,
                    public_key: STANDARD.encode(key.verifying_key().to_bytes()),
                },
            )
            .unwrap()
        };
        let old = source.models[0].clone();
        let path = temp
            .path()
            .join("models/bundles")
            .join(&old.model_id)
            .join(old.bundle_digest());
        std::fs::create_dir_all(&path).unwrap();
        for a in &old.artifacts {
            std::fs::write(path.join(&a.filename), b"test").unwrap();
        }
        let manager = ModelManager::open(
            temp.path().join("models"),
            sign(&source),
            registry.clone(),
            Arc::new(|_| {}),
        )
        .unwrap();
        manager.initialize().await.unwrap();
        let settings = store
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .activate_model(Some(&old.model_id), Some(&old.bundle_digest()), None, 1)
            .unwrap();
        drop(manager);
        *store.lock().unwrap() = Ok(Store::open(&db).unwrap());
        let mut next = old.clone();
        next.version = Some("2026.10".into());
        source.models.insert(0, next.clone());
        let manager = ModelManager::open(
            temp.path().join("models"),
            sign(&source),
            registry,
            Arc::new(|_| {}),
        )
        .unwrap();
        manager.initialize().await.unwrap();
        let current = store.lock().unwrap().as_ref().unwrap().settings().unwrap();
        assert_eq!(current.active_model_id, settings.active_model_id);
        assert_eq!(current.revision, settings.revision);
        assert_eq!(
            manager.lease(&old.model_id).await.unwrap().bundle.directory,
            path.canonicalize().unwrap()
        );
        assert_eq!(manager.snapshot().models[0].state, InstallState::Absent);
        // Even after discovering and explicitly deleting a newer version, the
        // older selected row remains available and its settings revision stays.
        let next_path = temp
            .path()
            .join("models/bundles")
            .join(&next.model_id)
            .join(next.bundle_digest());
        let from = temp.path().join("source");
        std::fs::create_dir(&from).unwrap();
        let files = next
            .artifacts
            .iter()
            .map(|a| {
                let p = from.join(&a.filename);
                std::fs::write(&p, b"test").unwrap();
                p
            })
            .collect();
        manager
            .start_import(
                &next.installation_id(),
                manager
                    .prepare_import(&next.installation_id(), files)
                    .unwrap(),
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while manager.snapshot().busy {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(next_path.is_dir());
        assert_eq!(manager.snapshot().models[0].state, InstallState::Installed);
        manager.delete(&next.installation_id()).await.unwrap();
        assert!(!next_path.exists());
        assert!(path.is_dir());
        let after = store.lock().unwrap().as_ref().unwrap().settings().unwrap();
        assert_eq!(after.active_model_id, settings.active_model_id);
        assert_eq!(after.revision, settings.revision);
    }

    /// Uses the application's real SQLite adapter and production HTTPS client. It downloads
    /// public weights into a disposable directory and transcribes only the public JFK fixture.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "downloads 191 MB public model bundle and requires built worker + public fixture"]
    async fn signed_download_sqlite_reopen_and_real_worker() {
        verify_public_download(
            "whisper-small-q5_1",
            "speakmore-asr-worker",
            ".cache/asr-fixtures/jfk.wav",
            "ask not what your country",
            false,
            false,
        )
        .await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "downloads 242 MB public SenseVoice bundle; no microphone"]
    async fn signed_sensevoice_download_sqlite_reopen_and_real_worker() {
        verify_public_download(
            "sensevoice-small-int8",
            "speakmore-sensevoice-worker",
            ".cache/sensevoice/model/test_wavs/en.wav",
            "tribal",
            false,
            false,
        )
        .await;
    }
    async fn verify_public_download(
        model_id: &'static str,
        binary: &str,
        audio_file: &str,
        expected: &'static str,
        local_import: bool,
        versioned: bool,
    ) {
        use speakmore_worker_supervisor::{
            Backend, Configuration, Event, Executable, ModelSpec, Phase, Supervisor, Transcription,
        };
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let executable = workspace.join("target/debug").join(binary);
        let audio_path = workspace.join(audio_file);
        assert!(
            executable.is_file(),
            "build the worker before running this test"
        );
        assert!(audio_path.is_file(), "prepare the public JFK fixture first");
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("models.sqlite3");
        let store = Arc::new(Mutex::new(Ok(Store::open(&db).unwrap())));
        let registrations = Arc::new(AtomicUsize::new(0));
        let observed = registrations.clone();
        let registry = Arc::new(SqliteModels {
            store: store.clone(),
            changed: Box::new(move || {
                observed.fetch_add(1, Ordering::SeqCst);
            }),
        });
        let trust: TrustKey =
            serde_json::from_slice(include_bytes!("../../resources/model-trust.json")).unwrap();
        let mut catalog = TrustedCatalog::verify(
            include_bytes!("../../resources/model-catalog.signed.json"),
            &trust,
        )
        .unwrap();
        let installation_id = if versioned {
            use base64::{engine::general_purpose::STANDARD, Engine};
            use ed25519_dalek::{Signer, SigningKey};
            use speakmore_models::catalog::{Envelope, Payload};
            let mut payload: Payload = serde_json::from_slice(include_bytes!(
                "../../resources/model-catalog.payload.json"
            ))
            .unwrap();
            let mut model = catalog.model(model_id).unwrap().clone();
            model.version = Some("Public-fixture release".into());
            let id = model.installation_id();
            payload.models.push(model);
            let raw = serde_json::to_vec(&payload).unwrap();
            let key = SigningKey::from_bytes(&[81; 32]);
            let envelope = Envelope {
                key_id: "version-fixture".into(),
                payload: STANDARD.encode(&raw),
                signature: STANDARD.encode(key.sign(&raw).to_bytes()),
            };
            catalog = TrustedCatalog::verify(
                &serde_json::to_vec(&envelope).unwrap(),
                &TrustKey {
                    key_id: envelope.key_id,
                    public_key: STANDARD.encode(key.verifying_key().to_bytes()),
                },
            )
            .unwrap();
            id
        } else {
            model_id.to_owned()
        };
        let started = Instant::now();
        let manager = ModelManager::open(
            temp.path().join("models"),
            catalog.clone(),
            registry.clone(),
            Arc::new(|_| {}),
        )
        .unwrap();
        manager.initialize().await.unwrap();
        if local_import {
            let source = workspace.join(if model_id == "sensevoice-small-int8" {
                ".cache/sensevoice/model"
            } else {
                ".cache/asr-fixtures"
            });
            let files = catalog
                .model(&installation_id)
                .unwrap()
                .artifacts
                .iter()
                .map(|a| source.join(&a.filename))
                .collect();
            manager
                .start_import(
                    &installation_id,
                    manager.prepare_import(&installation_id, files).unwrap(),
                )
                .unwrap();
        } else {
            manager.start_download(&installation_id).unwrap();
        }
        let mut last = Instant::now();
        loop {
            let state = manager.snapshot();
            let small = state
                .models
                .iter()
                .find(|m| m.model_id == installation_id)
                .unwrap();
            if !state.busy {
                assert_eq!(small.state, InstallState::Installed, "{:?}", small.error);
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(600),
                "download deadline"
            );
            if last.elapsed() > Duration::from_secs(10) {
                eprintln!(
                    "public bundle: {} / {} bytes ({:?})",
                    small.received_bytes, small.size_bytes, small.state
                );
                last = Instant::now();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(store
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .settings()
            .unwrap()
            .active_model_id
            .is_none());
        let elapsed = started.elapsed();
        drop(manager);
        // Close/reopen SQLite and rescan actual bytes before passing paths to native code.
        *store.lock().unwrap() = Ok(Store::open(&db).unwrap());
        let manager = ModelManager::open(
            temp.path().join("models"),
            catalog,
            registry,
            Arc::new(|_| {}),
        )
        .unwrap();
        manager.initialize().await.unwrap();
        let lease = manager.lease(&installation_id).await.unwrap();
        let find = |role: &str| {
            lease
                .bundle
                .model
                .artifacts
                .iter()
                .find(|a| a.role == role)
                .unwrap()
        };
        let model = find("model");
        let vad = find("vad");
        let spec = ModelSpec {
            installation_id: lease.bundle.model.installation_id(),
            engine_id: lease.bundle.model.engine_id.clone(),
            profile_id: lease.bundle.model.engine_profile.clone(),
            bundle_sha256: lease.bundle.model.bundle_digest(),
            tokens: lease
                .bundle
                .model
                .artifacts
                .iter()
                .find(|a| a.role == "tokens")
                .map(|a| speakmore_protocol::ModelFile {
                    path: lease
                        .bundle
                        .directory
                        .join(&a.filename)
                        .to_str()
                        .unwrap()
                        .into(),
                    sha256: a.sha256.clone(),
                    size_bytes: a.size_bytes,
                }),
            auxiliary: lease
                .bundle
                .model
                .artifacts
                .iter()
                .filter(|a| !["model", "vad", "tokens"].contains(&a.role.as_str()))
                .map(|a| speakmore_protocol::ModelAsset {
                    role: a.role.clone(),
                    file: speakmore_protocol::ModelFile {
                        path: lease
                            .bundle
                            .directory
                            .join(&a.filename)
                            .to_str()
                            .unwrap()
                            .into(),
                        sha256: a.sha256.clone(),
                        size_bytes: a.size_bytes,
                    },
                })
                .collect(),
            model_id: lease.bundle.model.model_id.clone(),
            path: lease
                .bundle
                .directory
                .join(&model.filename)
                .to_str()
                .unwrap()
                .into(),
            sha256: model.sha256.clone(),
            size_bytes: model.size_bytes,
            vad_path: lease
                .bundle
                .directory
                .join(&vad.filename)
                .to_str()
                .unwrap()
                .into(),
            vad_sha256: vad.sha256.clone(),
            vad_size_bytes: vad.size_bytes,
        };
        let samples: Vec<f32> = hound::WavReader::open(audio_path)
            .unwrap()
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32768.0)
            .collect();
        let expected_installation = installation_id.clone();
        let asr_ms = tokio::task::spawn_blocking(move || {
            let mut worker = Supervisor::new(Configuration {
                sensevoice: (model_id == "sensevoice-small-int8").then(|| Executable {
                    path: executable.clone(),
                    backend: Backend::Cpu,
                }),
                cpu: Executable {
                    path: executable,
                    backend: Backend::Cpu,
                },
                accelerated: None,
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
                    "{events:?}"
                );
                assert!(start.elapsed() < Duration::from_secs(40));
                std::thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(
                worker.snapshot().model_id.as_deref(),
                Some(expected_installation.as_str())
            );
            worker
                .transcribe(Transcription {
                    session_id: uuid::Uuid::new_v4(),
                    language: "en".into(),
                    dictionary_revision: 1,
                    dictionary_terms: vec![],
                    samples,
                })
                .unwrap();
            let completed = loop {
                if let Some(result) = worker.poll().into_iter().find_map(|e| match e {
                    Event::Completed(result) => Some(result),
                    Event::Failed { error, .. } => panic!("{error:?}"),
                    _ => None,
                }) {
                    break result;
                }
                assert!(start.elapsed() < Duration::from_secs(90));
                std::thread::sleep(Duration::from_millis(20));
            };
            assert!(completed.text.to_ascii_lowercase().contains(expected));
            worker.shutdown();
            while worker.snapshot().phase != Phase::Stopped {
                worker.poll();
                assert!(start.elapsed() < Duration::from_secs(95));
                std::thread::sleep(Duration::from_millis(20));
            }
            completed.asr_ms
        })
        .await
        .unwrap();
        assert!(manager.delete(&installation_id).await.is_err());
        drop(lease);
        manager.delete(&installation_id).await.unwrap();
        assert!(registrations.load(Ordering::SeqCst) >= 4);
        eprintln!("Verified {} bundle + SQLite reopen + real CPU ASR ({model_id}): install {} ms, ASR {} ms", if local_import { "local import" } else { "signed HTTPS" }, elapsed.as_millis(), asr_ms);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "offline public model/audio fixture; uses a test-only signed version label and real CPU worker"]
    async fn versioned_offline_import_sqlite_reopen_and_real_worker() {
        verify_public_download(
            "whisper-small-q5_1",
            "speakmore-asr-worker",
            ".cache/asr-fixtures/jfk.wav",
            "ask not what your country",
            true,
            true,
        )
        .await;
        verify_public_download(
            "sensevoice-small-int8",
            "speakmore-sensevoice-worker",
            ".cache/sensevoice/model/test_wavs/en.wav",
            "tribal",
            true,
            true,
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires existing public model/audio fixtures and built native workers; imports offline into temporary SQLite and model directories"]
    async fn offline_import_both_engines_sqlite_reopen_and_real_worker() {
        verify_public_download(
            "whisper-small-q5_1",
            "speakmore-asr-worker",
            ".cache/asr-fixtures/jfk.wav",
            "ask not what your country",
            true,
            false,
        )
        .await;
        verify_public_download(
            "sensevoice-small-int8",
            "speakmore-sensevoice-worker",
            ".cache/sensevoice/model/test_wavs/en.wav",
            "tribal",
            true,
            false,
        )
        .await;
    }
}
