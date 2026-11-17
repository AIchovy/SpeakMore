//! Audio/worker fault injection for the real actor. These helpers are test-only;
//! a separate ignored test exercises the production worker with public speech.
#[path = "tests/polishing.rs"]
mod polishing;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[path = "tests/tiled.rs"]
mod tiled;
use super::super::{capture, Handle};
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use speakmore_audio::{
    capture::{CaptureEvent, Diagnostics, Selection},
    device::{Configuration as AudioConfig, Format},
    dsp::ProcessedAudio,
    error::AudioError,
};
use speakmore_models::{
    catalog::{Envelope, Payload, TrustKey, TrustedCatalog},
    InstalledBundle, Registry,
};
use std::path::{Path, PathBuf};

struct RegistryStore(SharedStore);
impl Registry for RegistryStore {
    fn installed(&self, bundle: &InstalledBundle) -> speakmore_models::Result<()> {
        self.0
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .register_model(&speakmore_core::storage::ModelInstall {
                model_id: bundle.model.installation_id(),
                revision: bundle.model.revision.clone(),
                bundle_sha256: bundle.model.bundle_digest(),
                size_bytes: bundle.model.size_bytes(),
                local_path: bundle.directory.to_str().unwrap().into(),
                verified_at: bundle.verified_at,
            })
            .map_err(|_| speakmore_models::ModelError::Registration)
    }
    fn removed(&self, id: &str) -> speakmore_models::Result<()> {
        self.0
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .forget_model(id, super::super::super::now())
            .map_err(|_| speakmore_models::ModelError::Registration)
    }
}
struct AudioScript {
    permission: Mutex<crate::permissions::Status>,
    device_available: AtomicBool,
    device_slow: AtomicBool,
    busy: AtomicBool,
    starts: std::sync::atomic::AtomicUsize,
    ready: AtomicBool,
    samples: Vec<f32>,
}
struct ScriptSource(Arc<AudioScript>);
struct ScriptCapture {
    shared: Arc<AudioScript>,
    id: Uuid,
    intent: std::sync::atomic::AtomicU8,
    ready_sent: AtomicBool,
    finished: AtomicBool,
}
impl capture::Source for ScriptSource {
    fn permissions(&self) -> crate::permissions::Status {
        *self.0.permission.lock().unwrap()
    }
    fn device_probe(
        &self,
        _: Option<String>,
    ) -> Box<dyn FnOnce() -> speakmore_audio::error::Result<()> + Send> {
        let source = self.0.clone();
        Box::new(move || {
            if source.device_slow.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_secs(1));
            }
            if source.device_available.load(Ordering::Acquire) {
                Ok(())
            } else {
                Err(AudioError::DeviceUnavailable)
            }
        })
    }
    fn busy(&self) -> bool {
        self.0.busy.load(Ordering::Acquire)
    }
    fn start(
        &self,
        id: Uuid,
        _: Selection,
    ) -> speakmore_audio::error::Result<Box<dyn capture::Capture>> {
        assert!(!self.0.busy.swap(true, Ordering::AcqRel));
        self.0.starts.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(ScriptCapture {
            shared: self.0.clone(),
            id,
            intent: std::sync::atomic::AtomicU8::new(0),
            ready_sent: AtomicBool::new(false),
            finished: AtomicBool::new(false),
        }))
    }
}
impl capture::Capture for ScriptCapture {
    fn stop(&self) {
        let _ = self
            .intent
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
    }
    fn cancel(&self) {
        self.intent.store(2, Ordering::Release);
    }
    fn level(&self) -> f32 {
        0.25
    }
    fn try_recv(&self) -> std::result::Result<CaptureEvent, std::sync::mpsc::TryRecvError> {
        let intent = self.intent.load(Ordering::Acquire);
        if intent != 0 && !self.finished.swap(true, Ordering::AcqRel) {
            self.shared.busy.store(false, Ordering::Release);
            let result = if intent == 2 {
                Err(AudioError::Cancelled)
            } else {
                Ok(ProcessedAudio {
                    samples: self.shared.samples.clone(),
                    input_frames: self.shared.samples.len() as u64,
                    input_rate: 16_000,
                    duration_ms: self.shared.samples.len() as u64 / 16,
                    reached_limit: self.shared.samples.len() == 4_800_000,
                })
            };
            return Ok(CaptureEvent::Finished {
                session_id: self.id,
                result,
                diagnostics: Diagnostics {
                    accepted_frames: self.shared.samples.len() as u64,
                    clipped_samples: 0,
                    scheduling_fallback: false,
                },
            });
        }
        if self.shared.ready.load(Ordering::Acquire)
            && !self.ready_sent.swap(true, Ordering::AcqRel)
        {
            return Ok(CaptureEvent::Ready {
                session_id: self.id,
                device_id: "test-only-source".into(),
                configuration: AudioConfig {
                    sample_rate: 16_000,
                    channels: 1,
                    sample_format: Format::F32,
                },
                started_at: Instant::now(),
            });
        }
        Err(std::sync::mpsc::TryRecvError::Empty)
    }
}
const SMALL: &str = "whisper-small-q5_1";
const TURBO: &str = "whisper-large-v3-turbo-q5_0";
#[tokio::test]
async fn version_selection_persists_exact_installation_and_can_return_to_old_release() {
    let h = Harness::new("versions", vec![0.25; 4000], false).await;
    let next = h
        .models
        .snapshot()
        .models
        .into_iter()
        .find(|m| m.version.is_some())
        .unwrap()
        .model_id;
    h.select(SMALL).await;
    let fault = rusqlite::Connection::open(h._temp.path().join("store.sqlite3")).unwrap();
    fault.execute_batch("CREATE TRIGGER reject_version_selection BEFORE UPDATE OF active_model_id ON settings WHEN new.active_model_id LIKE '%--%' BEGIN SELECT RAISE(ABORT, 'version selection fixture'); END").unwrap();
    let before = h.handle.snapshot().unwrap().settings.revision;
    assert!(h
        .handle
        .select_model(Some(next.clone()), before)
        .await
        .is_err());
    let restored = h.wait(|s| s.can_start && s.model_operation.is_none()).await;
    assert_eq!(restored.settings.active_model_id.as_deref(), Some(SMALL));
    assert_eq!(restored.worker.model_id.as_deref(), Some(SMALL));
    assert_eq!(restored.settings.revision, before);
    fault
        .execute_batch("DROP TRIGGER reject_version_selection")
        .unwrap();
    drop(fault);
    h.select(&next).await;
    let current = h.handle.snapshot().unwrap();
    assert_eq!(
        current.settings.active_model_id.as_deref(),
        Some(next.as_str())
    );
    assert_eq!(current.worker.model_id.as_deref(), Some(next.as_str()));
    assert!(current.model_name.unwrap().contains("2026.10"));
    assert_eq!(
        h.handle.delete_model(next.clone()).await.unwrap_err().code,
        "ModelInUse"
    );
    let id = h.record().await;
    assert_eq!(
        h.handle
            .select_model(Some(SMALL.into()), current.settings.revision)
            .await
            .unwrap_err()
            .code,
        "Busy"
    );
    let finished = h.finish(id).await;
    assert_eq!(finished.recent_results[0].model_id, next);
    assert_eq!(finished.usage_today, 1);
    h.select(SMALL).await;
    h.handle.delete_model(next.clone()).await.unwrap();
    assert!(h.handle.snapshot().unwrap().can_start);
    assert!(h.models.lease(SMALL).await.is_ok());
    assert!(h.models.lease(&next).await.is_err());
    h.shutdown().await;
    let reopened =
        speakmore_core::storage::Store::open(&h._temp.path().join("store.sqlite3")).unwrap();
    assert_eq!(
        reopened.settings().unwrap().active_model_id.as_deref(),
        Some(SMALL)
    );
}
#[tokio::test]
async fn failed_version_load_keeps_old_selection_and_does_not_publish_candidate_ready() {
    let h = Harness::new("version_fail", vec![0.25; 4000], false).await;
    let next = h
        .models
        .snapshot()
        .models
        .into_iter()
        .find(|m| m.version.is_some())
        .unwrap()
        .model_id;
    h.select(SMALL).await;
    let revision = h.handle.snapshot().unwrap().settings.revision;
    assert!(h
        .handle
        .select_model(Some(next.clone()), revision)
        .await
        .is_err());
    let state = h.wait(|s| s.model_operation.is_none() && s.can_start).await;
    assert_eq!(state.settings.active_model_id.as_deref(), Some(SMALL));
    assert_eq!(state.settings.revision, revision);
    assert_eq!(state.worker.model_id.as_deref(), Some(SMALL));
    assert_eq!(state.usage_today, 0);
    assert!(
        h.models.lease(&next).await.is_ok(),
        "failed load does not delete verified bytes"
    );
    h.shutdown().await;
}
struct Harness {
    handle: Handle,
    store: SharedStore,
    models: ModelManager,
    source: Arc<AudioScript>,
    delivery_updates: Arc<Mutex<Vec<crate::delivery::DeliveryUpdate>>>,
    _temp: tempfile::TempDir,
}
impl Harness {
    async fn new(mode: &str, samples: Vec<f32>, real: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let catalog = if real {
            let trust =
                serde_json::from_slice(include_bytes!("../../../resources/model-trust.json"))
                    .unwrap();
            TrustedCatalog::verify(
                include_bytes!("../../../resources/model-catalog.signed.json"),
                &trust,
            )
            .unwrap()
        } else {
            let mut payload: Payload = serde_json::from_slice(include_bytes!(
                "../../../resources/model-catalog.payload.json"
            ))
            .unwrap();
            for model in &mut payload.models {
                for file in &mut model.artifacts {
                    file.size_bytes = 4;
                    file.sha256 = format!("{:x}", Sha256::digest(b"test"));
                }
            }
            if matches!(mode, "versions" | "version_fail") {
                let mut next = payload
                    .models
                    .iter()
                    .find(|m| m.model_id == SMALL)
                    .unwrap()
                    .clone();
                next.version = Some("2026.10".into());
                if mode == "version_fail" {
                    std::fs::write(temp.path().join("rejected-bundle"), next.bundle_digest())
                        .unwrap();
                }
                payload.models.push(next);
            }
            let raw = serde_json::to_vec(&payload).unwrap();
            let key = SigningKey::from_bytes(&[81; 32]);
            let envelope = Envelope {
                key_id: "actor-test-only".into(),
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
        let store_path = temp.path().join("store.sqlite3");
        let initial_store = speakmore_core::storage::Store::open(&store_path);
        let store = Arc::new(Mutex::new(Ok(initial_store.unwrap())));
        let models = ModelManager::open(
            temp.path().join("models"),
            catalog.clone(),
            Arc::new(RegistryStore(store.clone())),
            Arc::new(|_| {}),
        )
        .unwrap();
        // Populate a disposable fixture directory before initialization, which fully hashes it.
        for model in catalog.models().iter().filter(|m| {
            !real || m.model_id == SMALL || (mode == "both" && m.family == "sensevoice")
        }) {
            let root = temp
                .path()
                .join("models/bundles")
                .join(&model.model_id)
                .join(model.bundle_digest());
            std::fs::create_dir_all(&root).unwrap();
            for file in &model.artifacts {
                if real {
                    std::fs::copy(
                        workspace
                            .join(if model.family == "sensevoice" {
                                ".cache/sensevoice/model"
                            } else {
                                ".cache/asr-fixtures"
                            })
                            .join(&file.filename),
                        root.join(&file.filename),
                    )
                    .unwrap();
                } else {
                    std::fs::write(root.join(&file.filename), b"test").unwrap();
                }
            }
        }
        models.initialize().await.unwrap();
        let executable = if real {
            // Test-only override lets the same public fixture verify a packaged worker.
            std::env::var_os("SPEAKMORE_TEST_WORKER")
                .map(PathBuf::from)
                .unwrap_or_else(|| workspace.join("target/debug/speakmore-asr-worker"))
        } else {
            let original = workspace.join("target/debug/speakmore-supervisor-fixture");
            assert!(
                original.is_file(),
                "run npm run test:session to build the test-only worker"
            );
            let destination = temp.path().join("speakmore-supervisor-fixture");
            std::fs::copy(original, &destination).unwrap();
            std::fs::write(temp.path().join("fixture-mode"), mode).unwrap();
            destination
        };
        let source = Arc::new(AudioScript {
            permission: Mutex::new(crate::permissions::Status {
                microphone: crate::permissions::Microphone::Authorized,
                accessibility: false,
                requesting_microphone: false,
            }),
            device_available: AtomicBool::new(true),
            device_slow: AtomicBool::new(false),
            busy: AtomicBool::new(false),
            starts: std::sync::atomic::AtomicUsize::new(0),
            ready: AtomicBool::new(true),
            samples,
        });
        let delivery_updates = Arc::new(Mutex::new(Vec::new()));
        let updates = delivery_updates.clone();
        let handle = Handle::launch_with_source(
            store.clone(),
            Ok(models.clone()),
            Configuration {
                sensevoice: real.then(|| speakmore_worker_supervisor::Executable {
                    path: workspace.join("target/debug/speakmore-sensevoice-worker"),
                    backend: speakmore_worker_supervisor::Backend::Cpu,
                }),
                cpu: speakmore_worker_supervisor::Executable {
                    path: executable,
                    backend: speakmore_worker_supervisor::Backend::Cpu,
                },
                accelerated: None,
            },
            Arc::new(|_| {}),
            Arc::new(move |update| updates.lock().unwrap().push(update)),
            Box::new(ScriptSource(source.clone())),
        )
        .unwrap();
        let harness = Self {
            handle,
            store,
            models,
            source,
            delivery_updates,
            _temp: temp,
        };
        harness
            .wait(|s| {
                s.model_operation.is_none()
                    && s.readiness_code != super::ReadinessCode::UpdatingConfiguration
            })
            .await;
        harness
    }
    async fn wait(&self, predicate: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            if let Ok(snapshot) = self.handle.snapshot() {
                if predicate(&snapshot) {
                    return snapshot;
                }
            }
            assert!(
                Instant::now() < deadline,
                "actor timeout; snapshot: {}",
                self.handle
                    .snapshot()
                    .map(|s| format!(
                        "{} / {:?} / {:?}",
                        s.readiness_reason, s.worker.phase, s.last_error
                    ))
                    .unwrap_or_default()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    async fn select(&self, id: &str) {
        self.handle
            .select_model(
                Some(id.into()),
                self.handle.snapshot().unwrap().settings.revision,
            )
            .await
            .unwrap();
        self.wait(|s| s.can_start).await;
    }
    async fn record(&self) -> Uuid {
        let id = self
            .handle
            .start(Uuid::new_v4(), StartMode::Toggle)
            .await
            .unwrap();
        self.wait(|s| s.session.phase == Phase::Recording).await;
        id
    }
    async fn finish(&self, id: Uuid) -> Snapshot {
        self.handle.stop(id, false).await.unwrap();
        self.wait(|s| s.session.session_id.is_none()).await
    }
    async fn shutdown(&self) {
        self.handle.shutdown();
        self.wait(|s| s.worker.phase == WorkerPhase::Stopped).await;
    }
}
#[tokio::test]
async fn usage_write_failure_preserves_results_and_does_not_impose_a_recording_quota() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let fault = rusqlite::Connection::open(h._temp.path().join("store.sqlite3")).unwrap();
    fault.execute_batch("CREATE TRIGGER reject_usage BEFORE INSERT ON usage_events BEGIN SELECT RAISE(ABORT,'synthetic usage failure'); END;").unwrap();
    for _ in 0..2 {
        let id = h.record().await;
        h.finish(id).await;
        let snapshot = h.wait(|s| s.can_start).await;
        assert_eq!(
            snapshot.storage_warning.as_ref().unwrap().code,
            "UsageNotSaved"
        );
        assert_eq!(snapshot.usage_today, 0);
        assert_eq!(
            h.handle.result_text(id, "raw".into()).await.unwrap(),
            "Synthetic test result"
        );
    }
    assert_eq!(h.handle.snapshot().unwrap().recent_results.len(), 2);
    fault.execute_batch("DROP TRIGGER reject_usage;").unwrap();
    let id = h.record().await;
    h.finish(id).await;
    let recovered = h.wait(|s| s.can_start && s.usage_today == 1).await;
    assert!(recovered.storage_warning.is_none());
    h.shutdown().await;
}

#[tokio::test]
async fn unlimited_dictation_keeps_five_minutes_despite_legacy_license_data() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    // Existing use and an obsolete malformed license must not gate the new edition.
    let db_path = h._temp.path().join("store.sqlite3");
    let connection = rusqlite::Connection::open(&db_path).unwrap();
    connection
        .execute(
            "INSERT INTO license(id,signed_token,imported_at) VALUES(1,'obsolete-fixture',1)",
            [],
        )
        .unwrap();
    drop(connection);
    for expected in 1..=25 {
        let id = h.record().await;
        h.finish(id).await;
        let snapshot = h.wait(|s| s.can_start && s.usage_today == expected).await;
        assert_eq!(snapshot.session.max_duration_ms, 300_000);
        assert_eq!(snapshot.readiness_code, super::ReadinessCode::Ready);
    }
    h.shutdown().await;
    drop(h);
}

#[tokio::test]
async fn provider_save_survives_closed_caller_and_preserves_journal_through_failures() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let (service, vault) = crate::providers::Service::fixture(h.handle.clone());
    let id = Uuid::new_v4();
    let input = |expected, revision: Option<i64>| -> crate::providers::SaveInput {
        serde_json::from_value(serde_json::json!({
            "expectedRevision":expected,"providerId":id,"providerRevision":revision,
            "displayName":"Synthetic provider","configuration":{"protocol":{"kind":"openai"},"endpoint":"https://fixture.invalid/v1/chat/completions","model":"fixture-model"},
            "credential":{"action":"replace","key":"synthetic-test-key"},"confirmDestination":true
        })).unwrap()
    };
    let release = vault.hold_writes();
    let request = input(h.handle.snapshot().unwrap().settings.revision, None);
    let worker = service.clone();
    let caller = tokio::spawn(async move { worker.save(request).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while vault.writes() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    h.wait(|s| !s.can_start).await;
    assert!(h
        .handle
        .start(Uuid::new_v4(), StartMode::Toggle)
        .await
        .is_err());
    let native = h.handle.native_input();
    native.key(true);
    tokio::time::sleep(Duration::from_millis(60)).await;
    caller.abort();
    assert!(matches!(caller.await, Err(error) if error.is_cancelled()));
    assert!(
        serde_json::to_value(service.view().await.unwrap()).unwrap()["busy"]
            .as_bool()
            .unwrap()
    );
    drop(release);
    tokio::time::timeout(Duration::from_secs(3), async {
        while serde_json::to_value(service.view().await.unwrap()).unwrap()["busy"] == true {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    h.wait(|s| s.can_start).await;
    assert_eq!(
        h.source.starts.load(Ordering::SeqCst),
        0,
        "a held key from configuration must not start a late recording"
    );
    native.key(false);
    let original = h
        .handle
        .database(move |store| Ok(store.providers()?[0].clone()))
        .await
        .unwrap();
    assert_eq!(original.revision, 1);
    assert_eq!(vault.entries(), 1);
    assert_eq!(h.handle.snapshot().unwrap().usage_today, 0);
    assert!(h.handle.snapshot().unwrap().recent_results.is_empty());
    let revision = h.handle.snapshot().unwrap().settings.revision;
    // Local-only is checked before Keychain access or any remote request.
    assert_eq!(
        service.test(id, 1).await.unwrap_err().code,
        "ProviderLocalOnly"
    );
    assert_eq!(vault.reads(), 0);
    assert_eq!(h.handle.snapshot().unwrap().settings.revision, revision);
    vault.fail_write_after_commit(true);
    assert_eq!(
        service
            .save(input(revision, Some(1)))
            .await
            .unwrap_err()
            .code,
        "CredentialUnavailable"
    );
    vault.fail_write_after_commit(false);
    let after = h
        .handle
        .database(|store| Ok(store.providers()?[0].clone()))
        .await
        .unwrap();
    assert_eq!(after.draft.credential_ref, original.draft.credential_ref);
    assert_eq!(after.revision, 1);
    assert_eq!(h.handle.snapshot().unwrap().settings.revision, revision);
    assert_eq!(vault.entries(), 1);
    vault.fail_delete(true);
    service.save(input(revision, Some(1))).await.unwrap();
    let view = serde_json::to_value(service.view().await.unwrap()).unwrap();
    assert_eq!(view["cleanupPending"], 1);
    assert_eq!(view["cleanupError"]["code"], "CredentialDeleteUnconfirmed");
    assert_eq!(vault.entries(), 2);
    assert!(service.retry_cleanup().await.is_err());
    vault.fail_delete(false);
    service.retry_cleanup().await.unwrap();
    assert_eq!(vault.entries(), 1);
    let recording = h.record().await;
    let before = vault.writes();
    assert_eq!(
        service
            .save(input(
                h.handle.snapshot().unwrap().settings.revision,
                Some(2)
            ))
            .await
            .unwrap_err()
            .code,
        "Busy"
    );
    assert_eq!(vault.writes(), before);
    h.handle.stop(recording, true).await.unwrap();
    h.wait(|s| s.can_start).await;
    service
        .delete(h.handle.snapshot().unwrap().settings.revision, id)
        .await
        .unwrap();
    let view = serde_json::to_value(service.view().await.unwrap()).unwrap();
    assert!(view["rows"].as_array().unwrap().is_empty());
    assert_eq!(view["cleanupPending"], 0);
    assert_eq!(vault.entries(), 0);
    h.shutdown().await;
}

#[tokio::test]
async fn cancelled_provider_connection_discards_late_success_without_usage_or_receipt() {
    use std::io::{Read, Write};
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/api/chat", listener.local_addr().unwrap());
    let (received, waiting) = tokio::sync::oneshot::channel();
    let (release, wait_release) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut received = Some(received);
        for attempt in 0..2 {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buffer = [0; 4096];
            assert!(socket.read(&mut buffer).unwrap() > 0);
            if attempt == 1 {
                let _ = received.take().unwrap().send(());
                let _ = wait_release.recv_timeout(Duration::from_secs(3));
            }
            let body = br#"{"message":{"role":"assistant","content":"Please add punctuation to this sentence."},"done":true,"done_reason":"stop"}"#;
            // A cancelled client may already have closed the socket; either outcome
            // must leave the late success unable to create a receipt.
            let _ = write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
            let _ = socket.write_all(body);
        }
    });
    let (service, vault) = crate::providers::Service::fixture(h.handle.clone());
    let id = Uuid::new_v4();
    service.save(serde_json::from_value(serde_json::json!({"expectedRevision":h.handle.snapshot().unwrap().settings.revision,"providerId":id,"providerRevision":null,"displayName":"Cancellation fixture","configuration":{"protocol":{"kind":"ollama"},"endpoint":endpoint,"model":"fixture"},"credential":{"action":"remove"},"confirmDestination":false})).unwrap()).await.unwrap();
    service.test(id, 1).await.unwrap();
    assert_eq!(
        serde_json::to_value(service.view().await.unwrap()).unwrap()["rows"][0]["testValid"],
        true
    );
    let revision = h.handle.snapshot().unwrap().settings.revision;
    let worker = service.clone();
    let test = tokio::spawn(async move { worker.test(id, 1).await });
    tokio::time::timeout(Duration::from_secs(3), waiting)
        .await
        .unwrap()
        .unwrap();
    assert!(
        serde_json::to_value(service.view().await.unwrap()).unwrap()["busy"]
            .as_bool()
            .unwrap()
    );
    service.cancel_test();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), test)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .code,
        "ProviderTestCancelled"
    );
    let _ = release.send(());
    server.join().unwrap();
    let view = serde_json::to_value(service.view().await.unwrap()).unwrap();
    assert_eq!(view["rows"][0]["preparedThisRun"], false);
    assert!(view["rows"][0]["testedAt"].is_null());
    assert_eq!(view["busy"], false);
    assert_eq!(view["revision"], revision + 1);
    assert_eq!(view["rows"][0]["testValid"], false);
    let (restarted, _) = crate::providers::Service::fixture(h.handle.clone());
    assert_eq!(
        serde_json::to_value(restarted.view().await.unwrap()).unwrap()["rows"][0]["testValid"],
        false
    );
    assert_eq!(
        service
            .select(revision + 1, Some(id))
            .await
            .unwrap_err()
            .code,
        "ProviderNotTested"
    );
    assert_eq!(vault.reads(), 0);
    let snapshot = h.wait(|s| s.can_start).await;
    assert_eq!(snapshot.usage_today, 0);
    assert!(snapshot.recent_results.is_empty());
    h.shutdown().await;
}

#[tokio::test]
async fn invalid_provider_recovery_preserves_healthy_services_and_requires_current_idle_review() {
    use speakmore_core::providers::ProviderDraft;
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let (service, vault) = crate::providers::Service::fixture(h.handle.clone());
    let healthy = Uuid::new_v4();
    let broken = Uuid::new_v4();
    h.handle
        .database(move |store| {
            for (id, profile, name) in [
                (healthy, "{\"kind\":\"ollama\"}", "Healthy local"),
                (
                    broken,
                    "{\"unknown_profile\":\"private-canary\"}",
                    "Broken local",
                ),
            ] {
                store.save_provider(
                    store.settings()?.revision,
                    None,
                    ProviderDraft {
                        provider_id: id,
                        display_name: name.into(),
                        kind: "ollama".into(),
                        endpoint: "http://127.0.0.1:11434/api/chat".into(),
                        model_name: "fixture".into(),
                        request_profile_json: profile.into(),
                        consent_origin: None,
                        credential_ref: None,
                    },
                    crate::now(),
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let view = serde_json::to_value(service.view().await.unwrap()).unwrap();
    assert_eq!(view["rows"].as_array().unwrap().len(), 1);
    assert_eq!(view["rows"][0]["providerId"], healthy.to_string());
    assert_eq!(view["totalProviders"], 2);
    assert_eq!(view["recoveryRows"].as_array().unwrap().len(), 1);
    assert!(!view.to_string().contains("private-canary"));
    let id = Uuid::parse_str(view["recoveryRows"][0]["recoveryId"].as_str().unwrap()).unwrap();
    let revision = view["revision"].as_i64().unwrap();
    let repeated = serde_json::to_value(service.view().await.unwrap()).unwrap();
    assert_eq!(repeated["recoveryRows"][0]["recoveryId"], id.to_string());
    assert_eq!(vault.reads(), 0);
    assert_eq!(vault.writes(), 0);
    assert_eq!(
        service
            .discard_invalid(revision, Uuid::new_v4())
            .await
            .unwrap_err()
            .code,
        "ProviderRecoveryExpired"
    );
    let recording = h.record().await;
    assert_eq!(
        service
            .discard_invalid(revision, id)
            .await
            .unwrap_err()
            .code,
        "Busy"
    );
    h.handle.stop(recording, true).await.unwrap();
    h.wait(|s| s.can_start).await;
    let view = serde_json::to_value(service.view().await.unwrap()).unwrap();
    let id = Uuid::parse_str(view["recoveryRows"][0]["recoveryId"].as_str().unwrap()).unwrap();
    service.expire_recovery_for_test();
    assert_eq!(
        service
            .discard_invalid(view["revision"].as_i64().unwrap(), id)
            .await
            .unwrap_err()
            .code,
        "ProviderRecoveryExpired"
    );
    let view = serde_json::to_value(service.view().await.unwrap()).unwrap();
    let id = Uuid::parse_str(view["recoveryRows"][0]["recoveryId"].as_str().unwrap()).unwrap();
    service
        .discard_invalid(view["revision"].as_i64().unwrap(), id)
        .await
        .unwrap();
    let view = serde_json::to_value(service.view().await.unwrap()).unwrap();
    assert!(view["recoveryRows"].as_array().unwrap().is_empty());
    assert_eq!(view["totalProviders"], 1);
    assert_eq!(view["rows"][0]["providerId"], healthy.to_string());
    assert_eq!(h.handle.snapshot().unwrap().usage_today, 0);
    assert!(h.handle.snapshot().unwrap().recent_results.is_empty());
    assert_eq!(vault.reads(), 0);
    service
        .delete(view["revision"].as_i64().unwrap(), healthy)
        .await
        .unwrap();
    assert!(
        serde_json::to_value(service.view().await.unwrap()).unwrap()["rows"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    h.shutdown().await;
}

#[tokio::test]
async fn local_provider_test_restores_after_service_restart_without_a_second_network_request() {
    use std::io::{Read, Write};
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/api/chat", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("fixture accept: {error}"),
            }
        };
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0; 4096];
            let count = socket.read(&mut chunk).unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&chunk[..count]);
            assert!(bytes.len() < 16_384);
            if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                if bytes.len() >= end + 4 + length {
                    let body: serde_json::Value =
                        serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
                    assert_eq!(
                        body["messages"][1]["content"],
                        "Please add punctuation to this sentence"
                    );
                    assert!(!head.contains("authorization"));
                    break;
                }
            }
        }
        let body = serde_json::to_vec(&serde_json::json!({"message":{"role":"assistant","content":"Please add punctuation to this sentence."},"done":true,"done_reason":"stop"})).unwrap();
        write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
        socket.write_all(&body).unwrap();
        socket.shutdown(std::net::Shutdown::Write).unwrap();
    });
    let (service, vault) = crate::providers::Service::fixture(h.handle.clone());
    let id = Uuid::new_v4();
    let input = serde_json::from_value(serde_json::json!({"expectedRevision":h.handle.snapshot().unwrap().settings.revision,"providerId":id,"providerRevision":null,"displayName":"Local fixture","configuration":{"protocol":{"kind":"ollama"},"endpoint":endpoint,"model":"fixture"},"credential":{"action":"remove"},"confirmDestination":false})).unwrap();
    service.save(input).await.unwrap();
    assert!(service
        .select(h.handle.snapshot().unwrap().settings.revision, Some(id))
        .await
        .is_err());
    service.test(id, 1).await.unwrap();
    server.join().unwrap();
    let view = serde_json::to_value(service.view().await.unwrap()).unwrap();
    assert_eq!(view["rows"][0]["preparedThisRun"], true);
    service
        .select(h.handle.snapshot().unwrap().settings.revision, Some(id))
        .await
        .unwrap();
    assert_eq!(
        h.handle.snapshot().unwrap().settings.active_provider_id,
        Some(id.to_string())
    );
    assert!(!h.handle.snapshot().unwrap().settings.polish_enabled);
    assert_eq!(vault.entries(), 0);
    let (reopened, _) = crate::providers::Service::fixture(h.handle.clone());
    let view = serde_json::to_value(reopened.view().await.unwrap()).unwrap();
    assert!(view["rows"][0]["testedAt"].is_number());
    assert_eq!(view["rows"][0]["preparedThisRun"], false);
    assert_eq!(view["rows"][0]["testValid"], true);
    reopened
        .select(h.handle.snapshot().unwrap().settings.revision, Some(id))
        .await
        .unwrap();
    let prepared = serde_json::to_value(reopened.view().await.unwrap()).unwrap();
    assert_eq!(prepared["rows"][0]["preparedThisRun"], true);
    // The only HTTP server has exited. A second request would fail this action.
    // Failed re-testing durably revokes the old pass, including after a restart.
    assert!(reopened.test(id, 1).await.is_err());
    let (failed_restart, _) = crate::providers::Service::fixture(h.handle.clone());
    let failed = serde_json::to_value(failed_restart.view().await.unwrap()).unwrap();
    assert_eq!(failed["rows"][0]["testValid"], false);
    assert!(failed["rows"][0]["testedAt"].is_null());
    assert_eq!(
        failed_restart
            .select(h.handle.snapshot().unwrap().settings.revision, Some(id))
            .await
            .unwrap_err()
            .code,
        "ProviderNotTested"
    );
    h.shutdown().await;
}

#[tokio::test]
async fn result_records_manual_origin_and_missing_accessibility_without_exposing_target_context() {
    use speakmore_core::delivery::ManualReason;
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let id = h.record().await;
    let first = h.finish(id).await;
    assert_eq!(
        first.recent_results[0].delivery_reason,
        Some(ManualReason::MainWindow)
    );
    let input = h.handle.native_input();
    input.key(true);
    h.wait(|s| s.session.phase == Phase::Recording).await;
    input.key(false);
    let second = h.wait(|s| s.can_start).await;
    assert_eq!(
        second.recent_results[0].delivery_reason,
        Some(ManualReason::AccessibilityRequired)
    );
    assert_eq!(second.usage_today, 2);
    assert!(second
        .recent_results
        .iter()
        .all(|r| r.app_identity.is_none()));
    assert!(second
        .recent_results
        .iter()
        .all(|r| r.delivery_status == "ready_for_copy"));
    h.shutdown().await;
}

#[tokio::test]
async fn independent_target_check_keeps_actor_responsive_and_preserves_generated_text_once() {
    use speakmore_core::delivery::ManualReason;
    #[derive(Default)]
    struct ProbeGate {
        blocked: Mutex<bool>,
        wake: std::sync::Condvar,
        checks: std::sync::atomic::AtomicUsize,
        reason: Mutex<Option<ManualReason>>,
    }
    struct Probe(Arc<ProbeGate>);
    impl crate::target::Driver for Probe {
        fn capture(
            &mut self,
            _: u64,
            _: Instant,
        ) -> std::result::Result<Box<dyn crate::target::Watch>, ManualReason> {
            Ok(Box::new(Probe(self.0.clone())))
        }
    }
    impl crate::target::Watch for Probe {
        fn check(&mut self, _: Instant) -> std::result::Result<(), ManualReason> {
            Ok(())
        }
        fn recheck(&mut self, _: Instant) -> std::result::Result<(), ManualReason> {
            self.0.checks.fetch_add(1, Ordering::SeqCst);
            let mut blocked = self.0.blocked.lock().unwrap();
            while *blocked {
                blocked = self.0.wake.wait(blocked).unwrap();
            }
            self.0.reason.lock().unwrap().map_or(Ok(()), Err)
        }
    }
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    let gate = Arc::new(ProbeGate::default());
    *gate.blocked.lock().unwrap() = true;
    let probe_gate = gate.clone();
    let target_service = crate::target::Service::for_test(move || Box::new(Probe(probe_gate)));
    let (reply, ready) = oneshot::channel();
    h.handle
        .send(Command::Database(Box::new(move |_| {
            Box::new(move |actor| {
                actor.targets = Some(target_service);
                let _ = reply.send(());
            })
        })))
        .unwrap();
    ready.await.unwrap();
    // Only the fake capture source's status is changed. No OS permission or
    // real AX adapter is used by this integration test.
    h.source.permission.lock().unwrap().accessibility = true;
    h.wait(|s| s.permissions.accessibility).await;
    h.select(SMALL).await;
    let input = h.handle.native_input();
    input.key(true);
    let first = h.wait(|s| s.session.phase == Phase::Recording).await;
    input.key(false);
    h.wait(|s| s.session.phase == Phase::DeliveryCheck && gate.checks.load(Ordering::SeqCst) == 1)
        .await;
    // Cancellation must remain responsive while the platform probe is blocked.
    tokio::time::timeout(
        Duration::from_millis(200),
        h.handle.stop(first.session.session_id.unwrap(), true),
    )
    .await
    .unwrap()
    .unwrap();
    let copied = h.wait(|s| s.can_start).await;
    assert_eq!(copied.recent_results.len(), 1);
    assert_eq!(copied.usage_today, 1);
    assert_eq!(copied.recent_results[0].delivery_status, "ready_for_copy");
    assert_eq!(
        copied.recent_results[0].delivery_reason,
        Some(ManualReason::TargetUnavailable)
    );
    assert!(!copied.recent_results[0].raw_text.is_empty());
    *gate.blocked.lock().unwrap() = false;
    gate.wake.notify_all();
    input.key(true);
    h.wait(|s| s.session.phase == Phase::Recording).await;
    input.key(false);
    let next = h.wait(|s| s.can_start && s.usage_today == 2).await;
    assert_eq!(gate.checks.load(Ordering::SeqCst), 2);
    assert_eq!(next.recent_results.len(), 2);
    assert_eq!(
        next.recent_results[0].delivery_reason,
        Some(ManualReason::CompatibilityUnverified)
    );
    assert!(next
        .recent_results
        .iter()
        .all(|r| r.delivery_status == "ready_for_copy" && r.app_identity.is_none()));
    h.shutdown().await;
}

#[tokio::test]
async fn automatic_actor_persists_intent_before_dispatch_and_updates_one_history_record() {
    use crate::delivery::fixture::*;
    for (mode, expected, posted, save_history, delete_during_delivery, review) in [
        (NORMAL, "submitted", 1, true, false, false),
        (TARGET_AFTER_WRITE, "copied_only", 0, true, false, false),
        (CLIPBOARD_CHANGED, "ready_for_copy", 0, true, false, false),
        (DISPATCH_UNCERTAIN, "uncertain", 1, true, false, false),
        (NORMAL, "submitted", 1, false, false, false),
        (NORMAL, "submitted", 1, true, true, false),
        (NORMAL, "submitted", 1, true, false, true),
    ] {
        let h = Harness::new(
            if review { "needs_review" } else { "normal" },
            vec![0.25; 4000],
            false,
        )
        .await;
        let platform = State::new(mode);
        let (targets, clipboard) = services(&platform);
        let (reply, ready) = oneshot::channel();
        h.handle
            .send(Command::Database(Box::new(move |store| {
                let store = store.unwrap();
                let revision = store.settings().unwrap().revision;
                store
                    .update_settings(
                        revision,
                        SettingsPatch {
                            history_enabled: Some(save_history),
                            ..Default::default()
                        },
                        super::super::super::now(),
                    )
                    .unwrap();
                Box::new(move |actor| {
                    actor.targets = Some(targets);
                    actor.clipboard = Ok(clipboard);
                    let _ = reply.send(());
                })
            })))
            .unwrap();
        ready.await.unwrap();
        h.source.permission.lock().unwrap().accessibility = true;
        h.wait(|s| s.permissions.accessibility).await;
        h.select(SMALL).await;
        let input = h.handle.native_input();
        input.key(true);
        let recording = h.wait(|s| s.session.phase == Phase::Recording).await;
        let id = recording.session.session_id.unwrap();
        let intent_seen = Arc::new(AtomicBool::new(false));
        let observed = intent_seen.clone();
        let store = h.store.clone();
        *platform.before_submit.lock().unwrap() = Some(Box::new(move || {
            let saved = store
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .find_result(id)
                .unwrap();
            observed.store(
                saved.map_or(!save_history, |r| {
                    save_history && r.delivery_status == "uncertain"
                }),
                Ordering::SeqCst,
            );
            if delete_during_delivery {
                store
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .clear_history()
                    .unwrap();
            }
        }));
        input.key(false);
        let done = h.wait(|s| s.can_start && s.usage_today == 1).await;
        assert_eq!(done.recent_results.len(), 1);
        assert_eq!(done.recent_results[0].needs_review, review);
        assert_eq!(
            done.recent_results[0].delivery_status, expected,
            "scenario {mode}"
        );
        assert_eq!(platform.posts.load(Ordering::SeqCst), posted);
        if posted > 0 {
            assert!(intent_seen.load(Ordering::SeqCst));
        }
        let saved = h
            .handle
            .database(move |s| Ok((s.find_result(id)?, s.history("", None)?)))
            .await
            .unwrap();
        let retained = save_history && !delete_during_delivery;
        if retained {
            assert_eq!(saved.0.unwrap().delivery_status, expected);
        } else {
            assert!(saved.0.is_none());
        }
        assert_eq!(saved.1.items.len(), usize::from(retained));
        {
            let updates = h.delivery_updates.lock().unwrap();
            if save_history {
                assert_eq!(updates.len(), if delete_during_delivery { 1 } else { 2 });
                assert!(updates.iter().all(|u| u.session_id == id));
                assert_eq!(updates[0].delivery_status.as_str(), "uncertain");
                if retained {
                    assert_eq!(updates[1].delivery_status.as_str(), expected);
                    assert_eq!(
                        updates[1].delivery_reason,
                        done.recent_results[0].delivery_reason
                    );
                }
            } else {
                assert!(
                    updates.is_empty(),
                    "unsaved history must not emit durable updates"
                );
            }
        }
        assert_eq!(platform.writes.load(Ordering::SeqCst), 1);
        h.shutdown().await;
    }
}

#[tokio::test]
async fn per_app_rules_enforce_history_for_native_menu_and_unknown_sources_without_affecting_other_apps(
) {
    // kind: restricted at start, another app, transient restriction, unknown,
    // main-window recording, menu recording with a source app.
    for kind in 0..6 {
        let h = Harness::new("normal", vec![0.25; 4000], false).await;
        h.handle
            .database(move |store| {
                store.update_settings(
                    store.settings()?.revision,
                    SettingsPatch {
                        history_enabled: Some(true),
                        ..Default::default()
                    },
                    super::super::super::now(),
                )?;
                store.upsert_app_rule(
                    store.settings()?.revision,
                    AppRule {
                        platform: "macos".into(),
                        app_identity: "org.test.private".into(),
                        display_name: "Private Notes".into(),
                        manual_only: kind != 2,
                        history_excluded: kind != 2,
                    },
                    super::super::super::now(),
                )
            })
            .await
            .unwrap();
        h.wait(|s| s.settings.history_enabled).await;
        h.select(SMALL).await;
        let input = h.handle.native_input();
        let source = if kind == 3 {
            None
        } else if kind == 1 {
            Some("org.test.public".to_string())
        } else {
            Some("org.test.private".to_string())
        };
        input.application_activated(source.clone());
        let id = if kind == 4 {
            h.handle
                .start(Uuid::new_v4(), StartMode::Toggle)
                .await
                .unwrap()
        } else if kind == 5 {
            let origin = super::super::native_input::Press {
                cycle: Uuid::new_v4(),
                at: Instant::now(),
                control_revision: input.control_revision(),
                target_generation: input.target_generation(),
                app_identity: source.clone(),
                epoch: input.lifecycle().epoch,
                manual_only: true,
            };
            h.handle
                .start_with_origin(Uuid::new_v4(), StartMode::Toggle, Some(origin))
                .await
                .unwrap()
        } else {
            input.key(true);
            h.wait(|s| s.session.phase == Phase::Recording)
                .await
                .session
                .session_id
                .unwrap()
        };
        h.wait(|s| s.session.phase == Phase::Recording).await;
        if kind == 2 {
            // Both changes and deletion occur in one DB callback, before the
            // actor can see an intermediate view. The policy watch must latch.
            h.handle
                .database(|store| {
                    let mut rule = store.app_rules()?.remove(0);
                    rule.manual_only = true;
                    rule.history_excluded = true;
                    store.upsert_app_rule(store.settings()?.revision, rule.clone(), 1)?;
                    rule.manual_only = false;
                    rule.history_excluded = false;
                    store.upsert_app_rule(store.settings()?.revision, rule, 1)?;
                    store.delete_app_rule(store.settings()?.revision, "org.test.private", 1)
                })
                .await
                .unwrap();
        }
        input.application_activated(Some("org.test.public".into()));
        if kind == 4 || kind == 5 {
            h.handle.stop(id, false).await.unwrap();
        } else {
            input.key(false);
        }
        let done = h.wait(|s| s.can_start && s.usage_today == 1).await;
        assert_eq!(done.recent_results.len(), 1);
        let record = &done.recent_results[0];
        assert_eq!(record.app_identity, if kind == 4 { None } else { source });
        assert_eq!(record.delivery_status, "ready_for_copy");
        if ![1, 4].contains(&kind) {
            assert_eq!(
                record.delivery_reason,
                Some(speakmore_core::delivery::ManualReason::UserRequested)
            );
        }
        let count = h
            .handle
            .database(|store| Ok(store.history("", None)?.items.len()))
            .await
            .unwrap();
        assert_eq!(count, usize::from([1, 4].contains(&kind)), "case {kind}");
        h.shutdown().await;
    }
}

#[tokio::test]
async fn readiness_checks_model_permission_and_device_independently_of_usage() {
    use crate::permissions::Microphone;
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    assert!(!h.handle.snapshot().unwrap().permissions.accessibility);
    // Missing AX permission never blocks explicit local dictation and copying.
    for _ in 0..20 {
        let id = h.record().await;
        h.finish(id).await;
    }
    h.wait(|s| s.can_start).await;
    h.source.permission.lock().unwrap().microphone = Microphone::Denied;
    h.wait(|s| s.readiness_action == ReadinessAction::Microphone)
        .await;
    h.handle
        .select_model(None, h.handle.snapshot().unwrap().settings.revision)
        .await
        .unwrap();
    h.wait(|s| s.readiness_action == ReadinessAction::Models)
        .await;
    // Permission denial must still allow selecting and preloading a model.
    h.handle
        .select_model(
            Some(SMALL.into()),
            h.handle.snapshot().unwrap().settings.revision,
        )
        .await
        .unwrap();
    h.wait(|s| {
        s.worker.phase == WorkerPhase::Ready && s.readiness_action == ReadinessAction::Microphone
    })
    .await;
    h.source.device_available.store(false, Ordering::Release);
    h.source.permission.lock().unwrap().microphone = Microphone::Authorized;
    let device = h
        .wait(|s| s.readiness_code == super::ReadinessCode::InputDeviceUnavailable)
        .await;
    assert_eq!(device.readiness_action, ReadinessAction::Device);
    assert_eq!(
        device.readiness_detail_code.as_deref(),
        Some("DeviceUnavailable")
    );
    h.source.device_available.store(true, Ordering::Release);
    let s = h.wait(|s| s.can_start).await;
    assert_eq!(s.usage_today, 20);
    assert_eq!(s.readiness_code, super::ReadinessCode::Ready);
    assert_eq!(s.readiness_detail_code, None);
    assert!(s.can_start);
    assert_eq!(h.source.starts.load(Ordering::Acquire), 20);
    h.shutdown().await;
}

#[tokio::test]
async fn microphone_revocation_cancels_preparation_recording_and_late_transcription_without_charge()
{
    use crate::permissions::Microphone;
    for phase in [Phase::Preparing, Phase::Recording, Phase::Transcribing] {
        let h = Harness::new("late_completed", vec![0.25; 4_800_000], false).await;
        h.select(SMALL).await;
        h.source
            .ready
            .store(phase != Phase::Preparing, Ordering::Release);
        let id = h
            .handle
            .start(Uuid::new_v4(), StartMode::Toggle)
            .await
            .unwrap();
        if phase == Phase::Transcribing {
            h.wait(|s| s.session.phase == Phase::Recording).await;
            h.handle.stop(id, false).await.unwrap();
        }
        h.wait(|s| s.session.phase == phase).await;
        h.source.permission.lock().unwrap().microphone = Microphone::Denied;
        let s = h
            .wait(|s| {
                s.session.session_id.is_none() && s.readiness_action == ReadinessAction::Microphone
            })
            .await;
        assert_eq!(s.usage_today, 0);
        assert!(s.recent_results.is_empty());
        assert_eq!(s.last_error.unwrap().code, "MicrophonePermissionRevoked");
        assert!(!h.source.busy.load(Ordering::Acquire));
        assert!(h
            .handle
            .start(Uuid::new_v4(), StartMode::Toggle)
            .await
            .is_err());
        // Restoring permission prepares the model but never replays audio or starts.
        h.source.permission.lock().unwrap().microphone = Microphone::Authorized;
        h.wait(|s| s.can_start).await;
        assert_eq!(h.source.starts.load(Ordering::Acquire), 1);
        h.shutdown().await;
    }
}

#[tokio::test]
async fn start_rechecks_permission_and_rejects_pre_grant_requests_and_held_keys() {
    use crate::permissions::Microphone;
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let input = h.handle.native_input();
    let old = input.control_revision();
    // No polling wait: start itself must observe the denial.
    h.source.permission.lock().unwrap().microphone = Microphone::NotDetermined;
    assert!(h
        .handle
        .start(Uuid::new_v4(), StartMode::Toggle)
        .await
        .is_err());
    input.key(true);
    h.source.permission.lock().unwrap().microphone = Microphone::Authorized;
    h.wait(|s| s.can_start).await;
    let (reply, response) = oneshot::channel();
    h.handle
        .0
        .sender
        .try_send(Command::Start {
            request_id: Uuid::new_v4(),
            mode: StartMode::Toggle,
            epoch: input.lifecycle().epoch,
            control_revision: old,
            origin: None,
            reply,
        })
        .unwrap();
    assert!(response.await.unwrap().is_err());
    assert_eq!(h.source.starts.load(Ordering::Acquire), 0);
    input.key(false);
    input.key(true);
    let id = h
        .wait(|s| s.session.phase == Phase::Recording)
        .await
        .session
        .session_id
        .unwrap();
    h.handle.stop(id, true).await.unwrap();
    input.key(false);
    h.wait(|s| s.can_start).await;
    h.shutdown().await;
}

#[tokio::test]
async fn combination_toggle_ignores_repeat_and_release_then_second_press_finishes() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let input = h.handle.native_input();
    input.native_key(true);
    input.native_key(true); // OS key-repeat must not become a second press.
    let first = h.wait(|s| s.session.phase == Phase::Recording).await;
    assert_eq!(first.session.mode, Some(StartMode::Toggle));
    input.native_key(true);
    input.native_key(false);
    tokio::time::sleep(Duration::from_millis(75)).await;
    assert_eq!(
        h.handle.snapshot().unwrap().session.session_id,
        first.session.session_id
    );
    input.native_key(true);
    input.native_key(true);
    input.native_key(false);
    let done = h.wait(|s| s.can_start).await;
    assert_eq!(done.usage_today, 1);
    assert_eq!(done.recent_results.len(), 1);
    let editor = h.handle.begin_shortcut_edit().await.unwrap();
    input.native_key(true);
    input.end_edit(editor, false);
    input.native_key(true); // a held edit-time press is never replayed.
    input.native_key(false);
    tokio::time::sleep(Duration::from_millis(75)).await;
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 1);
    h.shutdown().await;
}

#[tokio::test]
async fn fn_toggle_starts_survives_release_and_finishes_once_even_with_two_stop_taps() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let input = h.handle.native_input();
    input.native_toggle();
    let first = h.wait(|s| s.session.phase == Phase::Recording).await;
    assert_eq!(first.session.mode, Some(StartMode::Toggle));
    input.key(false);
    tokio::time::sleep(Duration::from_millis(75)).await;
    assert_eq!(
        h.handle.snapshot().unwrap().session.session_id,
        first.session.session_id
    );
    input.native_toggle();
    input.native_toggle();
    let done = h.wait(|s| s.can_start).await;
    assert_eq!(done.usage_today, 1);
    assert_eq!(done.recent_results.len(), 1);
    let editor = h.handle.begin_shortcut_edit().await.unwrap();
    input.native_toggle();
    input.end_edit(editor, false);
    tokio::time::sleep(Duration::from_millis(75)).await;
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 1);
    h.shutdown().await;
}

#[tokio::test]
async fn shortcut_editor_reserves_idle_controls_without_unloading_model_or_replaying_keys() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let input = h.handle.native_input();
    let old_revision = input.control_revision();
    let editor = h.handle.begin_shortcut_edit().await.unwrap();
    assert!(h.handle.begin_shortcut_edit().await.is_err());
    assert!(h
        .handle
        .start(Uuid::new_v4(), StartMode::Toggle)
        .await
        .is_err());
    assert!(h.handle.reload_model().await.is_err());
    input.key(true);
    h.wait(|s| !s.can_start).await;
    assert_eq!(
        h.handle.snapshot().unwrap().worker.phase,
        WorkerPhase::Ready
    );
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 0);
    input.key(false);
    input.end_edit(editor, false);
    h.wait(|s| s.can_start).await;
    let (reply, response) = oneshot::channel();
    h.handle
        .0
        .sender
        .try_send(Command::Start {
            request_id: Uuid::new_v4(),
            mode: StartMode::Toggle,
            epoch: input.lifecycle().epoch,
            control_revision: old_revision,
            origin: None,
            reply,
        })
        .unwrap();
    assert!(response.await.unwrap().is_err());
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 0);
    input.key(true);
    let id = h
        .wait(|s| s.session.phase == Phase::Recording)
        .await
        .session
        .session_id
        .unwrap();
    assert!(h.handle.begin_shortcut_edit().await.is_err());
    assert_eq!(h.handle.snapshot().unwrap().session.session_id, Some(id));
    input.key(false);
    let s = h.wait(|s| s.can_start).await;
    assert_eq!(s.usage_today, 1);
    h.shutdown().await;
}

#[tokio::test]
async fn interruption_quick_resume_cancels_capture_and_rejects_old_start_and_held_key() {
    use super::super::Interruption;
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let input = h.handle.native_input();
    let old_epoch = input.lifecycle().epoch;
    input.key(true);
    h.wait(|s| s.session.phase == Phase::Recording).await;
    let epoch = input.interrupt(Interruption::SystemSleep, true);
    input.interrupt(Interruption::SystemSleep, false); // deliberately no actor poll between these
    let s = h
        .wait(|s| s.can_start && s.session.session_id.is_none())
        .await;
    assert!(input.wait_quiesced(epoch, Duration::ZERO));
    assert!(s.recent_results.is_empty());
    assert_eq!(s.usage_today, 0);
    let (reply, response) = oneshot::channel();
    h.handle
        .0
        .sender
        .try_send(Command::Start {
            request_id: Uuid::new_v4(),
            mode: StartMode::Toggle,
            epoch: old_epoch,
            control_revision: 0,
            origin: None,
            reply,
        })
        .unwrap();
    assert_eq!(
        response.await.unwrap().unwrap_err().code,
        "SessionInterrupted"
    );
    for _ in 0..1000 {
        input.key(true);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 1);
    input.key(false);
    input.key(true);
    h.wait(|s| s.session.phase == Phase::Recording).await;
    input.key(false);
    let s = h.wait(|s| s.can_start).await;
    assert_eq!(s.recent_results.len(), 1);
    assert_eq!(s.usage_today, 1);
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 2);
    h.shutdown().await;
}

#[tokio::test]
async fn interruption_quiescence_does_not_wait_for_a_blocked_database() {
    use super::super::Interruption;
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    h.record().await;
    let (entered, entry) = oneshot::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let client = h.handle.clone();
    let pending = tokio::spawn(async move {
        client
            .database(move |_| {
                let _ = entered.send(());
                gate.recv_timeout(Duration::from_secs(5))
                    .map_err(|_| db::unavailable())?;
                Ok(())
            })
            .await
    });
    entry.await.unwrap();
    let input = h.handle.native_input();
    let epoch = input.interrupt(Interruption::SystemSleep, true);
    // No database completion or WebView acknowledgement can occur before this.
    assert!(input.wait_quiesced(epoch, Duration::from_secs(2)));
    assert!(!h.source.busy.load(Ordering::Acquire));
    release.send(()).unwrap();
    pending.await.unwrap().unwrap();
    h.wait(|s| s.worker.phase == WorkerPhase::Stopped).await;
    input.interrupt(Interruption::SystemSleep, false);
    let s = h.wait(|s| s.can_start).await;
    assert!(s.recent_results.is_empty());
    assert_eq!(s.usage_today, 0);
    h.shutdown().await;
}

#[tokio::test]
async fn interruption_of_preparing_capture_waits_for_every_resume_reason() {
    use super::super::Interruption;
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    h.source.ready.store(false, Ordering::Release);
    h.handle
        .start(Uuid::new_v4(), StartMode::Toggle)
        .await
        .unwrap();
    let input = h.handle.native_input();
    input.interrupt(Interruption::DisplaySleep, true);
    let epoch = input.interrupt(Interruption::InactiveSession, true);
    let s = h
        .wait(|s| s.worker.phase == WorkerPhase::Stopped && s.session.session_id.is_none())
        .await;
    assert!(s.capture_suspended && !s.can_start);
    assert!(input.wait_quiesced(epoch, Duration::ZERO));
    assert!(!h.source.busy.load(Ordering::Acquire));
    assert_eq!(
        h.handle
            .start(Uuid::new_v4(), StartMode::Toggle)
            .await
            .unwrap_err()
            .code,
        "SessionInterrupted"
    );
    input.interrupt(Interruption::DisplaySleep, false);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        h.handle.snapshot().unwrap().worker.phase,
        WorkerPhase::Stopped
    );
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 1);
    input.interrupt(Interruption::InactiveSession, false);
    let s = h.wait(|s| s.can_start).await;
    assert!(s.recent_results.is_empty());
    assert_eq!(s.usage_today, 0);
    h.shutdown().await;
}

#[tokio::test]
async fn interruption_of_inference_reaps_worker_and_drops_late_completed_audio() {
    use super::super::Interruption;
    let h = Harness::new("late_completed", vec![0.25; 4_800_000], false).await;
    h.select(SMALL).await;
    let id = h.record().await;
    h.handle.stop(id, false).await.unwrap();
    h.wait(|s| s.session.phase == Phase::Transcribing).await;
    let input = h.handle.native_input();
    let epoch = input.interrupt(Interruption::SystemSleep, true);
    let s = h
        .wait(|s| s.worker.phase == WorkerPhase::Stopped && s.session.session_id.is_none())
        .await;
    assert!(input.wait_quiesced(epoch, Duration::ZERO));
    assert!(s.recent_results.is_empty());
    assert_eq!(s.usage_today, 0);
    input.interrupt(Interruption::SystemSleep, false);
    h.wait(|s| s.can_start).await;
    assert!(h.handle.snapshot().unwrap().recent_results.is_empty());
    h.shutdown().await;
}

#[tokio::test]
async fn interruption_during_model_load_releases_candidate_only_after_reaping() {
    use super::super::Interruption;
    let h = Harness::new("model_stall", vec![0.25; 4000], false).await;
    let client = h.handle.clone();
    let revision = client.snapshot().unwrap().settings.revision;
    let pending =
        tokio::spawn(async move { client.select_model(Some(SMALL.into()), revision).await });
    h.wait(|s| s.worker.phase == WorkerPhase::Loading).await;
    let input = h.handle.native_input();
    let epoch = input.interrupt(Interruption::SystemSleep, true);
    h.wait(|s| s.worker.phase == WorkerPhase::Stopped && s.model_operation.is_none())
        .await;
    assert_eq!(
        pending.await.unwrap().unwrap_err().code,
        "SessionInterrupted"
    );
    assert!(input.wait_quiesced(epoch, Duration::ZERO));
    assert!(h
        .handle
        .snapshot()
        .unwrap()
        .settings
        .active_model_id
        .is_none());
    h.models.delete(SMALL).await.unwrap(); // no child/model lease remains
    h.shutdown().await;
}

#[tokio::test]
async fn interruption_resume_revalidates_model_files_before_ready() {
    use super::super::Interruption;
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let input = h.handle.native_input();
    input.interrupt(Interruption::SystemSleep, true);
    h.wait(|s| s.worker.phase == WorkerPhase::Stopped && s.model_name.is_none())
        .await;
    {
        let lease = h.models.lease(SMALL).await.unwrap();
        let artifact = &lease.bundle.model.artifacts[0];
        std::fs::write(lease.bundle.directory.join(&artifact.filename), b"broken").unwrap();
    }
    input.interrupt(Interruption::SystemSleep, false);
    let s = h
        .wait(|s| s.settings.active_model_id.is_none() && s.model_operation.is_none())
        .await;
    assert!(!s.can_start);
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 0);
    h.shutdown().await;
}

#[tokio::test]
async fn native_release_cancels_preparation_despite_repeats_and_full_ipc_queue() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    h.source.ready.store(false, Ordering::Release);
    let input = h.handle.native_input();
    input.key(true);
    h.wait(|s| s.session.phase == Phase::Preparing).await;
    for _ in 0..1024 {
        input.key(true);
        let (reply, _) = oneshot::channel();
        let _ = h.handle.0.sender.try_send(Command::ClearResults(reply));
    }
    input.key(false);
    h.source.ready.store(true, Ordering::Release);
    let s = h.wait(|s| s.session.session_id.is_none()).await;
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 1);
    assert!(s.recent_results.is_empty());
    assert_eq!(s.usage_today, 0);
    h.shutdown().await;
}

#[tokio::test]
async fn native_cancel_requires_new_press_and_old_release_cannot_stop_toggle() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let input = h.handle.native_input();
    input.key(true);
    let first = h
        .wait(|s| s.session.phase == Phase::Recording)
        .await
        .session
        .session_id
        .unwrap();
    h.handle.native_stop(first, true).unwrap();
    h.wait(|s| s.can_start).await;
    for _ in 0..1000 {
        input.key(true);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 1);
    let toggle = h.record().await;
    input.key(false);
    h.handle.native_stop(first, true).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        h.handle.snapshot().unwrap().session.session_id,
        Some(toggle)
    );
    assert_eq!(h.handle.snapshot().unwrap().session.phase, Phase::Recording);
    h.handle.native_stop(toggle, true).unwrap();
    h.wait(|s| s.can_start).await;
    input.key(true);
    h.wait(|s| s.session.phase == Phase::Recording).await;
    input.key(false);
    let s = h.wait(|s| s.can_start).await;
    assert_eq!(s.recent_results.len(), 1);
    assert_eq!(s.usage_today, 1);
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 3);
    h.shutdown().await;
}

#[tokio::test]
async fn native_press_while_unready_is_not_replayed_when_model_becomes_ready() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    let input = h.handle.native_input();
    input.key(true);
    h.wait(|s| s.last_error.is_some()).await;
    h.select(SMALL).await;
    input.key(true);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 0);
    input.key(false);
    tokio::time::sleep(Duration::from_millis(50)).await;
    input.key(true);
    let id = h
        .wait(|s| s.session.phase == Phase::Recording)
        .await
        .session
        .session_id
        .unwrap();
    h.handle.native_stop(id, true).unwrap();
    h.wait(|s| s.can_start).await;
    input.key(false);
    h.shutdown().await;
}

#[tokio::test]
async fn not_ready_never_opens_capture_and_preparing_stop_never_transcribes() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    assert!(h
        .handle
        .start(Uuid::new_v4(), StartMode::Toggle)
        .await
        .is_err());
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 0);
    h.select(SMALL).await;
    h.source.ready.store(false, Ordering::Release);
    let request = Uuid::new_v4();
    let id = h.handle.start(request, StartMode::Toggle).await.unwrap();
    assert_eq!(
        h.handle.start(request, StartMode::Toggle).await.unwrap(),
        id
    );
    h.handle.stop(id, false).await.unwrap();
    let s = h.wait(|s| s.session.session_id.is_none()).await;
    assert!(s.recent_results.is_empty());
    assert_eq!(s.usage_today, 0);
    assert_eq!(
        h.handle.start(request, StartMode::Toggle).await.unwrap(),
        id
    );
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 1);
    h.shutdown().await;
}
#[tokio::test]
async fn model_switch_failure_restores_previous_and_leases_require_unload() {
    let h = Harness::new("switch_fail", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    assert!(h
        .handle
        .select_model(
            Some(TURBO.into()),
            h.handle.snapshot().unwrap().settings.revision
        )
        .await
        .is_err());
    let s = h.wait(|s| s.can_start).await;
    assert_eq!(s.settings.active_model_id.as_deref(), Some(SMALL));
    assert_eq!(s.worker.model_id.as_deref(), Some(SMALL));
    assert!(h.models.delete(SMALL).await.is_err());
    // Use the same actor-owned mutation as the desktop command. Completion
    // must include the registry view refresh, allowing an immediate unload.
    h.handle.delete_model(TURBO.into()).await.unwrap();
    assert!(h.handle.snapshot().unwrap().can_start);
    h.handle
        .select_model(None, s.settings.revision)
        .await
        .unwrap();
    assert!(h
        .handle
        .snapshot()
        .unwrap()
        .settings
        .active_model_id
        .is_none());
    h.handle.delete_model(SMALL.into()).await.unwrap();
    h.shutdown().await;
}
#[tokio::test]
async fn full_five_minute_fixture_is_one_result_one_charge_and_keeps_frozen_revision() {
    let h = Harness::new("normal", vec![0.25; 4_800_000], false).await;
    h.select(SMALL).await;
    let frozen = h.handle.snapshot().unwrap().settings.revision;
    let id = h.record().await;
    assert!(h
        .handle
        .select_model(Some(TURBO.into()), frozen)
        .await
        .is_err());
    assert!(h.handle.download_model(TURBO.into()).await.is_err());
    assert!(h.handle.set_device(None, frozen).await.is_err());
    h.handle
        .database(move |s| {
            s.update_settings(
                frozen,
                SettingsPatch {
                    language: Some("zh".into()),
                    ..Default::default()
                },
                super::super::super::now(),
            )
        })
        .await
        .unwrap();
    let s = h.finish(id).await;
    assert_eq!(s.recent_results.len(), 1);
    assert_eq!(s.usage_today, 1);
    let record = &s.recent_results[0];
    assert_eq!(record.duration_ms, 300_000);
    assert_eq!(record.dictionary_revision, frozen);
    assert!(s.settings.revision > frozen);
    h.handle.stop(id, false).await.unwrap();
    h.handle.stop(id, true).await.unwrap();
    assert_eq!(h.handle.snapshot().unwrap().usage_today, 1);
    assert_eq!(
        h.handle.result_text(id, "raw".into()).await.unwrap(),
        "Synthetic test result"
    );
    assert!(h
        .handle
        .database(|s| s.history("", None))
        .await
        .unwrap()
        .items
        .is_empty());
    h.shutdown().await;
}
#[tokio::test]
async fn cancel_during_large_transfer_discards_a_late_completed_frame() {
    let h = Harness::new("late_completed", vec![0.25; 4_800_000], false).await;
    h.select(SMALL).await;
    let id = h.record().await;
    h.handle.stop(id, false).await.unwrap();
    h.wait(|s| s.session.phase == Phase::Transcribing).await;
    h.handle.stop(id, true).await.unwrap();
    let s = h
        .wait(|s| s.session.session_id.is_none() && s.can_start)
        .await;
    assert!(s.recent_results.is_empty());
    assert_eq!(s.usage_today, 0);
    h.shutdown().await;
}
#[tokio::test]
async fn slow_database_does_not_delay_cancel_or_accept_model_changes() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let id = h.record().await;
    let (entered, entry) = tokio::sync::oneshot::channel();
    let db = h.handle.clone();
    let task = tokio::spawn(async move {
        db.database(move |_| {
            let _ = entered.send(());
            std::thread::sleep(Duration::from_millis(400));
            Ok(())
        })
        .await
    });
    entry.await.unwrap();
    let start = Instant::now();
    h.handle.stop(id, true).await.unwrap();
    assert!(start.elapsed() < Duration::from_millis(200));
    assert!(h.handle.reload_model().await.is_err());
    task.await.unwrap().unwrap();
    h.wait(|s| s.session.session_id.is_none()).await;
    h.shutdown().await;
}
#[tokio::test]
async fn toggling_history_off_then_on_during_recording_cannot_restore_consent() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let rev = h.handle.snapshot().unwrap().settings.revision;
    h.handle
        .database(move |s| {
            s.update_settings(
                rev,
                SettingsPatch {
                    history_enabled: Some(true),
                    ..Default::default()
                },
                super::super::super::now(),
            )
        })
        .await
        .unwrap();
    let id = h.record().await;
    h.handle
        .database(|s| {
            let rev = s.settings()?.revision;
            s.update_settings(
                rev,
                SettingsPatch {
                    history_enabled: Some(false),
                    ..Default::default()
                },
                super::super::super::now(),
            )?;
            let rev = s.settings()?.revision;
            s.update_settings(
                rev,
                SettingsPatch {
                    history_enabled: Some(true),
                    ..Default::default()
                },
                super::super::super::now(),
            )
        })
        .await
        .unwrap();
    let s = h.finish(id).await;
    assert_eq!(s.usage_today, 1);
    assert_eq!(s.recent_results.len(), 1);
    assert!(h
        .handle
        .database(|s| s.history("", None))
        .await
        .unwrap()
        .items
        .is_empty());
    let id = h.record().await;
    h.finish(id).await;
    assert_eq!(
        h.handle
            .database(|s| s.history("", None))
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    h.shutdown().await;
}
#[tokio::test]
async fn database_failure_keeps_generated_text_and_blocks_unaccounted_next_recording() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let id = h.record().await;
    *h.store.lock().unwrap() = Err(db::unavailable());
    let s = h.finish(id).await;
    assert_eq!(s.recent_results.len(), 1);
    assert_eq!(s.storage_warning.as_ref().unwrap().code, "UsageNotSaved");
    let payload = serde_json::to_value(&s).unwrap();
    assert_eq!(payload["storageWarning"]["code"], "UsageNotSaved");
    assert!(payload["storageWarning"]["userMessage"].is_string());
    assert!(!s.can_start);
    assert_eq!(
        h.handle.result_text(id, "raw".into()).await.unwrap(),
        "Synthetic test result"
    );
    assert!(h
        .handle
        .start(Uuid::new_v4(), StartMode::Toggle)
        .await
        .is_err());
    h.shutdown().await;
}
#[tokio::test]
async fn recent_results_cap_and_clear_do_not_erase_usage() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    for _ in 0..11 {
        let id = h.record().await;
        h.finish(id).await;
    }
    assert_eq!(h.handle.snapshot().unwrap().recent_results.len(), 10);
    h.handle.clear_results().await.unwrap();
    let s = h.handle.snapshot().unwrap();
    assert!(s.recent_results.is_empty());
    assert_eq!(s.usage_today, 11);
    h.shutdown().await;
}
#[tokio::test]
#[ignore = "uses real signed cached Whisper files and the real CPU worker, never the microphone"]
async fn real_worker_public_speech_through_actor() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".cache/asr-fixtures");
    let samples = hound::WavReader::open(root.join("jfk.wav"))
        .unwrap()
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect();
    let h = Harness::new("", samples, true).await;
    h.select(SMALL).await;
    let id = h.record().await;
    let s = h.finish(id).await;
    assert_eq!(s.recent_results.len(), 1);
    assert!(s.recent_results[0]
        .raw_text
        .to_ascii_lowercase()
        .contains("ask not what your country"));
    assert_eq!(s.recent_results[0].duration_ms, 11000);
    assert_eq!(s.usage_today, 1);
    eprintln!("Actor public fixture → real CPU ASR: {} ms; memory result, frozen metadata and one quota event verified", s.recent_results[0].asr_ms);
    h.shutdown().await;
}

#[tokio::test]
async fn shutdown_during_loading_reaps_worker_and_rejects_pending_selection() {
    let h = Harness::new("model_stall", vec![0.25; 4000], false).await;
    let client = h.handle.clone();
    let revision = client.snapshot().unwrap().settings.revision;
    let selection =
        tokio::spawn(async move { client.select_model(Some(SMALL.into()), revision).await });
    h.wait(|s| s.worker.phase == WorkerPhase::Loading).await;
    h.shutdown().await;
    assert!(tokio::time::timeout(Duration::from_secs(2), selection)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    assert_eq!(
        std::fs::read_to_string(h._temp.path().join("spawn-count")).unwrap(),
        "1"
    );
    h.models.delete(SMALL).await.unwrap();
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalidated_active_bundle_cannot_record_and_can_be_unloaded() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    {
        let lease = h.models.lease(SMALL).await.unwrap();
        let file = &lease.bundle.model.artifacts[0].filename;
        std::fs::write(lease.bundle.directory.join(file), b"bad!").unwrap();
    }
    assert!(h.handle.reload_model().await.is_err());
    h.wait(|s| s.settings.active_model_id.is_none() && !s.can_start)
        .await;
    assert!(h
        .handle
        .start(Uuid::new_v4(), StartMode::Toggle)
        .await
        .is_err());
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 0);
    // Clearing the persisted selection can be published while the invalidation
    // refresh is still queued. Wait for the public configuration-ready state,
    // not just absence of a selected model, before requesting another mutation.
    let s = h
        .wait(|s| s.readiness_action == ReadinessAction::Models)
        .await;
    h.handle
        .select_model(None, s.settings.revision)
        .await
        .unwrap();
    h.wait(|s| s.worker.phase == WorkerPhase::Stopped && s.model_name.is_none())
        .await;
    h.shutdown().await;
}

#[tokio::test]
async fn failed_selection_commit_retires_candidate_before_restoring_old_worker() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let revision = h.handle.snapshot().unwrap().settings.revision;
    *h.store.lock().unwrap() = Err(db::unavailable());
    assert!(h
        .handle
        .select_model(Some(TURBO.into()), revision)
        .await
        .is_err());
    let s = h
        .wait(|s| s.model_operation.is_none() && s.worker.phase == WorkerPhase::Ready)
        .await;
    assert_eq!(s.worker.model_id.as_deref(), Some(SMALL));
    assert!(!s.can_start);
    assert!(h.models.delete(SMALL).await.is_err());
    h.shutdown().await;
}

#[tokio::test]
#[ignore = "real native engines and public Chinese audio; no microphone"]
async fn real_sensevoice_whisper_switch_through_actor() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let samples = hound::WavReader::open(root.join(".cache/sensevoice/model/test_wavs/zh.wav"))
        .unwrap()
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect();
    let h = Harness::new("both", samples, true).await;
    for (index, model) in ["sensevoice-small-int8", SMALL, "sensevoice-small-int8"]
        .into_iter()
        .enumerate()
    {
        h.select(model).await;
        let id = h.record().await;
        let s = h.finish(id).await;
        let r = &s.recent_results[0];
        assert_eq!(r.model_id, model);
        assert_eq!(s.settings.active_model_id.as_deref(), Some(model));
        assert_eq!(s.usage_today, (index + 1) as u32);
        if model == "sensevoice-small-int8" {
            assert!(r.raw_text.contains("9点"));
            assert!(r.needs_review);
        }
        assert!(!r.raw_text.is_empty());
        eprintln!(
            "Actor {model}: {} ms; result and single quota verified",
            r.asr_ms
        );
    }
    h.shutdown().await;
}

#[tokio::test]
async fn local_import_respects_active_session_and_preserves_current_model_and_history() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let source = h._temp.path().join("import-source");
    std::fs::create_dir(&source).unwrap();
    let lease = h.models.lease(TURBO).await.unwrap();
    let files: Vec<_> = lease
        .bundle
        .model
        .artifacts
        .iter()
        .map(|a| {
            let path = source.join(&a.filename);
            std::fs::copy(lease.bundle.directory.join(&a.filename), &path).unwrap();
            path
        })
        .collect();
    drop(lease);
    h.handle.delete_model(TURBO.into()).await.unwrap();
    let ready = h.handle.snapshot().unwrap();
    let selection = h.models.prepare_import(TURBO, files.clone()).unwrap();
    let id = h.record().await;
    assert_eq!(
        h.handle
            .import_model(TURBO.into(), selection)
            .await
            .unwrap_err()
            .code,
        "Busy"
    );
    assert!(!h.models.snapshot().busy);
    h.handle.stop(id, true).await.unwrap();
    h.wait(|s| s.can_start).await;
    h.handle
        .import_model(
            TURBO.into(),
            h.models.prepare_import(TURBO, files.clone()).unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let models = h.models.snapshot();
            if !models.busy
                && models.models.iter().any(|m| {
                    m.model_id == TURBO && m.state == speakmore_models::InstallState::Installed
                })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let after = h.wait(|s| s.can_start && s.revision > ready.revision).await;
    assert_eq!(after.settings.active_model_id.as_deref(), Some(SMALL));
    assert_eq!(after.worker.model_id.as_deref(), Some(SMALL));
    assert!(after.recent_results.is_empty());
    assert_eq!(after.usage_today, 0);
    assert!(!after.settings.history_enabled);
    for path in files {
        assert_eq!(std::fs::read(path).unwrap(), b"test");
    }
    h.shutdown().await;
}

#[tokio::test]
async fn background_reservation_yields_to_fn_and_preserves_single_toggle() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let cancel = Arc::new(AtomicBool::new(false));
    let yielded = Arc::new(AtomicBool::new(false));
    let lease = h
        .handle
        .reserve_background(cancel.clone(), yielded.clone())
        .await
        .unwrap();
    let input = h.handle.native_input();
    input.native_toggle();
    for _ in 0..30 {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(cancel.load(Ordering::Acquire));
    assert!(yielded.load(Ordering::Acquire));
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 0);
    drop(lease);
    h.wait(|s| s.session.phase == Phase::Recording).await;
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 1);
    input.native_toggle();
    h.wait(|s| s.can_start).await;
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 1);
    h.shutdown().await;
}

#[tokio::test]
async fn meeting_recording_can_take_background_lease_without_waiting_for_old_worker() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let old_cancel = Arc::new(AtomicBool::new(false));
    let old_yielded = Arc::new(AtomicBool::new(false));
    let old_lease = h
        .handle
        .reserve_background(old_cancel.clone(), old_yielded.clone())
        .await
        .unwrap();
    // This is the handoff used by meetings::Service::begin: the recording owns
    // the existing reservation before the old model worker finishes cancelling.
    let recording_lease = old_lease.clone();
    old_yielded.store(true, Ordering::Release);
    old_cancel.store(true, Ordering::Release);
    h.handle.set_background_interrupt(None).unwrap();
    assert!(h.handle.reserve_settings_operation().await.is_err());
    assert!(old_cancel.load(Ordering::Acquire));
    assert!(old_yielded.load(Ordering::Acquire));
    drop(old_lease);
    assert!(h.handle.reserve_settings_operation().await.is_err());
    let input = h.handle.native_input();
    input.native_toggle();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 0);
    drop(recording_lease);
    h.wait(|s| s.can_start).await;
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 0);
    input.native_toggle();
    h.wait(|s| s.session.phase == Phase::Recording).await;
    assert_eq!(h.source.starts.load(Ordering::SeqCst), 1);
    input.native_toggle();
    h.wait(|s| s.can_start).await;
    h.shutdown().await;
}

#[tokio::test]
async fn display_sleep_cancels_capture_but_keeps_the_loaded_worker() {
    use super::super::Interruption;
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let spawns = || std::fs::read_to_string(h._temp.path().join("spawn-count")).unwrap();
    let input = h.handle.native_input();
    h.record().await;
    let epoch = input.interrupt(Interruption::DisplaySleep, true);
    let s = h.wait(|s| s.session.session_id.is_none()).await;
    assert!(input.wait_quiesced(epoch, Duration::from_secs(2)));
    assert!(s.capture_suspended && !s.can_start);
    assert_eq!(s.last_error.unwrap().code, "SessionInterrupted");
    assert!(s.recent_results.is_empty());
    input.interrupt(Interruption::DisplaySleep, false);
    let s = h.wait(|s| s.can_start).await;
    assert_eq!(s.worker.model_id.as_deref(), Some(SMALL));
    assert_eq!(spawns(), "1");
    assert!(h.models.delete(SMALL).await.is_err(), "lease stays held");
    let id = h.record().await;
    assert_eq!(h.finish(id).await.usage_today, 1);
    // Real sleep still reaps the worker and revalidates the files on resume.
    input.interrupt(Interruption::SystemSleep, true);
    h.wait(|s| s.worker.phase == WorkerPhase::Stopped && s.model_name.is_none())
        .await;
    input.interrupt(Interruption::SystemSleep, false);
    h.wait(|s| s.can_start).await;
    assert_eq!(spawns(), "2");
    h.shutdown().await;
}

#[tokio::test]
async fn full_storage_queue_defers_persistence_instead_of_reentering_native_input() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    let raw = rusqlite::Connection::open(h._temp.path().join("store.sqlite3")).unwrap();
    raw.execute("UPDATE settings SET polish_enabled=1", [])
        .unwrap();
    drop(raw);
    h.handle.database(|_| Ok(())).await.unwrap();
    // No polisher is installed, so refinement resolves as soon as raw text is saved.
    let id = h.record().await;
    let (entered, entry) = oneshot::channel();
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let client = h.handle.clone();
    let blocked = tokio::spawn(async move {
        client
            .database(move |_| {
                let _ = entered.send(());
                gate.recv_timeout(Duration::from_secs(30))
                    .map_err(|_| db::unavailable())
            })
            .await
    });
    entry.await.unwrap();
    for _ in 0..40 {
        h.handle
            .0
            .sender
            .try_send(Command::Database(Box::new(|_| Box::new(|_| {}))))
            .unwrap();
    }
    h.handle.stop(id, false).await.unwrap();
    h.wait(|s| s.session.phase == Phase::Polishing).await;
    release.send(()).unwrap();
    blocked.await.unwrap().unwrap();
    let s = h.wait(|s| s.can_start && s.usage_today == 1).await;
    assert_eq!(s.recent_results.len(), 1);
    assert!(s.storage_warning.is_none());
    h.shutdown().await;
}

#[tokio::test]
async fn failed_switch_rolls_back_to_the_running_model_after_its_selection_was_cleared() {
    let h = Harness::new("switch_fail", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    {
        let lease = h.models.lease(SMALL).await.unwrap();
        let file = &lease.bundle.model.artifacts[0].filename;
        std::fs::write(lease.bundle.directory.join(file), b"bad!").unwrap();
    }
    // Another lease holder finds the damage; the registry clears the persisted
    // selection while the worker still runs the loaded bundle.
    assert!(h.models.lease(SMALL).await.is_err());
    let s = h
        .wait(|s| {
            s.settings.active_model_id.is_none()
                && s.readiness_code == super::ReadinessCode::ModelConfigurationMismatch
        })
        .await;
    let switch = h
        .handle
        .select_model(Some(TURBO.into()), s.settings.revision);
    assert!(tokio::time::timeout(Duration::from_secs(20), switch)
        .await
        .expect("model operation settled")
        .is_err());
    let s = h
        .wait(|s| {
            s.model_operation.is_none()
                && s.readiness_code == super::ReadinessCode::ModelConfigurationMismatch
        })
        .await;
    assert_eq!(s.worker.model_id.as_deref(), Some(SMALL));
    h.handle
        .select_model(None, s.settings.revision)
        .await
        .unwrap();
    h.wait(|s| s.worker.phase == WorkerPhase::Stopped && s.model_name.is_none())
        .await;
    h.shutdown().await;
}

#[tokio::test]
async fn long_session_does_not_leave_a_stale_input_check_that_rejects_the_next_press() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    h.source.device_slow.store(true, Ordering::Release);
    let id = h.record().await;
    tokio::time::sleep(Duration::from_millis(10_500)).await;
    h.finish(id).await;
    let next = h
        .handle
        .start(Uuid::new_v4(), StartMode::Toggle)
        .await
        .unwrap();
    h.wait(|s| s.session.phase == Phase::Recording).await;
    assert_eq!(h.finish(next).await.usage_today, 2);
    h.shutdown().await;
}

#[tokio::test]
async fn read_only_queries_and_unchanged_state_do_not_republish() {
    let h = Harness::new("normal", vec![0.25; 4000], false).await;
    h.select(SMALL).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = h.wait(|s| s.can_start).await.revision;
    let count = h
        .handle
        .query(|s| Ok(s.history("", None)?.items.len()))
        .await
        .unwrap();
    assert_eq!(count, 0);
    let (reply, response) = oneshot::channel();
    h.handle
        .0
        .sender
        .try_send(Command::ClearResults(reply))
        .unwrap();
    response.await.unwrap().unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(h.handle.snapshot().unwrap().revision, before);
    h.shutdown().await;
}
