//! Offline candidate integration. Only the capture source is synthetic; model
//! import, SQLite, actor, VAD and packaged workers are real. No WebView or mic.
use super::*;
use serde_json::{json, Value};
use speakmore_models::InstallState;
use speakmore_protocol::{
    SENSEVOICE_TILED_BYTES, SENSEVOICE_TILED_PROFILE, SENSEVOICE_TILED_SHA256,
};
use std::{fs, sync::mpsc::TryRecvError};

const ORIGINAL: &str = "sensevoice-small-int8";
const PREFIX: &str = "SPEAKMORE_ACTOR_EVIDENCE ";
fn mark(value: Value) {
    eprintln!("{PREFIX}{value}");
}
fn stage(name: &str) {
    mark(json!({"stage": name}));
}
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

#[derive(Default)]
struct AudioQueue {
    next: Mutex<Option<Vec<f32>>>,
    busy: AtomicBool,
}
struct QueuedSource(Arc<AudioQueue>);
struct QueuedCapture {
    queue: Arc<AudioQueue>,
    samples: Mutex<Option<Vec<f32>>>,
    id: Uuid,
    intent: std::sync::atomic::AtomicU8,
    ready: AtomicBool,
}
impl capture::Source for QueuedSource {
    fn permissions(&self) -> crate::permissions::Status {
        crate::permissions::Status {
            microphone: crate::permissions::Microphone::Authorized,
            accessibility: false,
            requesting_microphone: false,
        }
    }
    fn device_probe(
        &self,
        _: Option<String>,
    ) -> Box<dyn FnOnce() -> speakmore_audio::error::Result<()> + Send> {
        Box::new(|| Ok(()))
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
        let samples = self
            .0
            .next
            .lock()
            .unwrap()
            .take()
            .expect("queue public audio before starting");
        Ok(Box::new(QueuedCapture {
            queue: self.0.clone(),
            samples: Mutex::new(Some(samples)),
            id,
            intent: std::sync::atomic::AtomicU8::new(0),
            ready: AtomicBool::new(false),
        }))
    }
}
impl capture::Capture for QueuedCapture {
    fn stop(&self) {
        let _ = self
            .intent
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
    }
    fn cancel(&self) {
        self.intent.store(2, Ordering::Release);
    }
    fn level(&self) -> f32 {
        0.0
    }
    fn try_recv(&self) -> std::result::Result<CaptureEvent, TryRecvError> {
        let intent = self.intent.load(Ordering::Acquire);
        if intent != 0 {
            if let Some(samples) = self.samples.lock().unwrap().take() {
                self.queue.busy.store(false, Ordering::Release);
                let count = samples.len() as u64;
                let result = if intent == 2 {
                    Err(AudioError::Cancelled)
                } else {
                    Ok(ProcessedAudio {
                        samples,
                        input_frames: count,
                        input_rate: 16_000,
                        duration_ms: count / 16,
                        reached_limit: count == 4_800_000,
                    })
                };
                return Ok(CaptureEvent::Finished {
                    session_id: self.id,
                    result,
                    diagnostics: Diagnostics {
                        accepted_frames: count,
                        clipped_samples: 0,
                        scheduling_fallback: false,
                    },
                });
            }
        }
        if !self.ready.swap(true, Ordering::AcqRel) {
            return Ok(CaptureEvent::Ready {
                session_id: self.id,
                device_id: "public-audio-fixture".into(),
                configuration: AudioConfig {
                    sample_rate: 16_000,
                    channels: 1,
                    sample_format: Format::F32,
                },
                started_at: Instant::now(),
            });
        }
        Err(TryRecvError::Empty)
    }
}
impl Drop for QueuedCapture {
    fn drop(&mut self) {
        self.queue.busy.store(false, Ordering::Release);
    }
}

fn speech(long: bool) -> Vec<f32> {
    let raw = fs::read(workspace().join(".cache/sensevoice/model/test_wavs/en.wav")).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(&raw)),
        "eb1eb008904465b74c304aad8342e8c7d3c6e61ffe9f66adcaca9cf0f76a93f4"
    );
    let mut reader = hound::WavReader::new(raw.as_slice()).unwrap();
    assert_eq!(
        (
            reader.spec().sample_rate,
            reader.spec().channels,
            reader.spec().bits_per_sample
        ),
        (16_000, 1, 16)
    );
    let mut phrase: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect();
    if !long {
        return phrase;
    }
    phrase.extend([0.0; 16_000]);
    phrase.iter().copied().cycle().take(4_800_000).collect()
}
fn catalog() -> (TrustedCatalog, String, PathBuf) {
    let report_path = PathBuf::from(
        std::env::var_os("SPEAKMORE_TEST_TILED_REPORT")
            .expect("set the generated candidate report path"),
    );
    assert!(report_path.is_absolute() && report_path.ancestors().all(|p| !p.is_symlink()));
    assert_eq!(
        report_path.parent().unwrap().parent().unwrap(),
        workspace().join(".cache/sensevoice/graph-study")
    );
    let report: Value = serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["completed"], true);
    assert_eq!(report["block"], 256);
    assert_eq!(report["layers"], 70);
    assert_eq!(report["candidate_sha256"], SENSEVOICE_TILED_SHA256);
    assert_eq!(report["candidate_bytes"], SENSEVOICE_TILED_BYTES);
    let mut payload: Payload = serde_json::from_slice(include_bytes!(
        "../../../../resources/model-catalog.payload.json"
    ))
    .unwrap();
    let mut next = payload
        .models
        .iter()
        .find(|m| m.model_id == ORIGINAL)
        .unwrap()
        .clone();
    let artifact = next
        .artifacts
        .iter_mut()
        .find(|a| a.role == "model")
        .unwrap();
    assert_eq!(report["source_sha256"], artifact.sha256);
    artifact.filename = "model.tiled.onnx".into();
    artifact.sha256 = SENSEVOICE_TILED_SHA256.into();
    artifact.size_bytes = SENSEVOICE_TILED_BYTES;
    // Never a real download address; the test imports local files and has its
    // own signing trust. The production catalog is not changed or resigned.
    artifact.url = format!(
        "https://huggingface.co/speakmore/test/resolve/{}/model.tiled.onnx",
        artifact.revision
    );
    next.version = Some("tiled256-v1".into());
    next.engine_profile = SENSEVOICE_TILED_PROFILE.into();
    let id = next.installation_id();
    payload.models.push(next);
    let raw = serde_json::to_vec(&payload).unwrap();
    let key = SigningKey::from_bytes(&[81; 32]);
    let envelope = Envelope {
        key_id: "actor-candidate-only".into(),
        payload: STANDARD.encode(&raw),
        signature: STANDARD.encode(key.sign(&raw).to_bytes()),
    };
    let catalog = TrustedCatalog::verify(
        &serde_json::to_vec(&envelope).unwrap(),
        &TrustKey {
            key_id: envelope.key_id,
            public_key: STANDARD.encode(key.verifying_key().to_bytes()),
        },
    )
    .unwrap();
    let trust: TrustKey =
        serde_json::from_slice(include_bytes!("../../../../resources/model-trust.json")).unwrap();
    let baseline = TrustedCatalog::verify(
        include_bytes!("../../../../resources/model-catalog.signed.json"),
        &trust,
    )
    .unwrap();
    catalog.preserves(&baseline).unwrap();
    (
        catalog,
        id,
        report_path.parent().unwrap().join("model.tiled.onnx"),
    )
}
struct Desktop {
    handle: Handle,
    models: ModelManager,
    audio: Arc<AudioQueue>,
}
impl Desktop {
    async fn open(path: &Path, catalog: TrustedCatalog) -> Self {
        let store = Arc::new(Mutex::new(Ok(speakmore_core::storage::Store::open(
            &path.join("store.sqlite3"),
        )
        .unwrap())));
        let manager = ModelManager::open(
            path.join("models"),
            catalog,
            Arc::new(crate::model_registry::SqliteModels {
                store: store.clone(),
                changed: Box::new(|| {}),
            }),
            Arc::new(|_| {}),
        )
        .unwrap();
        manager.initialize().await.unwrap();
        let workers = workspace()
            .join("target/release/bundle/macos/SpeakMore.app/Contents/Resources/workers");
        let audio = Arc::new(AudioQueue::default());
        let handle = Handle::launch_with_source(
            store,
            Ok(manager.clone()),
            Configuration {
                cpu: speakmore_worker_supervisor::Executable {
                    path: workers.join("speakmore-asr-worker"),
                    backend: speakmore_worker_supervisor::Backend::Cpu,
                },
                accelerated: None,
                sensevoice: Some(speakmore_worker_supervisor::Executable {
                    path: workers.join("speakmore-sensevoice-worker"),
                    backend: speakmore_worker_supervisor::Backend::Cpu,
                }),
            },
            Arc::new(|_| {}),
            Arc::new(|_| {}),
            Box::new(QueuedSource(audio.clone())),
        )
        .unwrap();
        let instance = Self {
            handle,
            models: manager,
            audio,
        };
        instance
            .wait(|s| {
                s.model_operation.is_none()
                    && s.readiness_code != ReadinessCode::UpdatingConfiguration
            })
            .await;
        instance
    }
    async fn wait(&self, predicate: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let begin = Instant::now();
        loop {
            let Ok(snapshot) = self.handle.snapshot() else {
                assert!(
                    begin.elapsed() < Duration::from_secs(100),
                    "actor initialization deadline"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            };
            if predicate(&snapshot) {
                return snapshot;
            }
            assert!(
                begin.elapsed() < Duration::from_secs(100),
                "actor deadline: {} {:?} {:?}",
                snapshot.readiness_reason,
                snapshot.worker.phase,
                snapshot.last_error
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    async fn select(&self, id: Option<&str>) {
        self.handle
            .select_model(
                id.map(str::to_string),
                self.handle.snapshot().unwrap().settings.revision,
            )
            .await
            .unwrap();
        self.wait(|s| {
            s.model_operation.is_none()
                && s.settings.active_model_id.as_deref() == id
                && (id.is_none() || s.can_start)
        })
        .await;
    }
    async fn import(&self, id: &str, files: Vec<PathBuf>) {
        let prepared = self.models.prepare_import(id, files).unwrap();
        self.handle.import_model(id.into(), prepared).await.unwrap();
        let begin = Instant::now();
        loop {
            let models = self.models.snapshot();
            let row = models.models.iter().find(|m| m.model_id == id).unwrap();
            if !models.busy {
                assert_eq!(row.state, InstallState::Installed, "{:?}", row.error);
                break;
            }
            assert!(begin.elapsed() < Duration::from_secs(90));
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.wait(|s| s.model_operation.is_none()).await;
    }
    async fn start(&self, long: bool) -> Uuid {
        assert!(self
            .audio
            .next
            .lock()
            .unwrap()
            .replace(speech(long))
            .is_none());
        let id = self
            .handle
            .start(Uuid::new_v4(), StartMode::Toggle)
            .await
            .unwrap();
        self.wait(|s| s.session.phase == Phase::Recording).await;
        id
    }
    async fn record(&self, long: bool, model: &str, quota: u32) -> ResultRecord {
        let id = self.start(long).await;
        self.handle.stop(id, false).await.unwrap();
        let s = self
            .wait(|s| s.session.session_id.is_none() && s.can_start && s.usage_today == quota)
            .await;
        let r = s
            .recent_results
            .iter()
            .find(|r| r.session_id == id)
            .unwrap()
            .clone();
        assert_eq!(
            s.recent_results
                .iter()
                .filter(|r| r.session_id == id)
                .count(),
            1
        );
        assert_eq!(r.model_id, model);
        assert_eq!(
            r.duration_ms,
            if long {
                300_000
            } else {
                speech(false).len() as u64 / 16
            }
        );
        assert_eq!(r.delivery_status, "ready_for_copy");
        assert!(r.polished_text.is_none());
        assert!(r.raw_text.to_lowercase().contains("tribal"));
        assert_eq!(
            r.model_sha256,
            self.models
                .lease(model)
                .await
                .unwrap()
                .bundle
                .model
                .artifacts
                .iter()
                .find(|a| a.role == "model")
                .unwrap()
                .sha256
        );
        mark(
            json!({"result":{"session_id":id,"model":model,"samples":r.duration_ms*16,"asr_ms":r.asr_ms,"text_bytes":r.raw_text.len(),"text_sha256":format!("{:x}",Sha256::digest(r.raw_text.as_bytes())),"quota":s.usage_today}}),
        );
        r
    }
    async fn close(self) {
        self.handle.shutdown();
        self.wait(|s| s.worker.phase == WorkerPhase::Stopped).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "real tiled candidate import, SQLite restart and packaged CPU workers; public audio, no GUI or microphone"]
async fn real_tiled_import_restart_five_minutes_and_recovery_through_actor() {
    stage("preflight");
    let (catalog, id, graph) = catalog();
    let temp = tempfile::tempdir().unwrap();
    let mut d = Desktop::open(temp.path(), catalog.clone()).await;
    let files = |installation: &str| {
        catalog
            .model(installation)
            .unwrap()
            .artifacts
            .iter()
            .map(|a| {
                if a.sha256 == SENSEVOICE_TILED_SHA256 {
                    graph.clone()
                } else {
                    workspace()
                        .join(if installation == SMALL {
                            ".cache/asr-fixtures"
                        } else {
                            ".cache/sensevoice/model"
                        })
                        .join(&a.filename)
                }
            })
            .collect()
    };
    stage("import_original");
    d.import(ORIGINAL, files(ORIGINAL)).await;
    stage("load_original");
    d.select(Some(ORIGINAL)).await;
    let before = d.handle.snapshot().unwrap().settings;
    stage("import_candidate");
    d.import(&id, files(&id)).await;
    assert_eq!(
        d.handle.snapshot().unwrap().settings,
        before,
        "import must not change current selection"
    );
    stage("selection_commit_failure");
    let fault = rusqlite::Connection::open(temp.path().join("store.sqlite3")).unwrap();
    fault.execute_batch("CREATE TRIGGER reject_candidate BEFORE UPDATE OF active_model_id ON settings WHEN new.active_model_id LIKE '%--%' BEGIN SELECT RAISE(ABORT,'candidate save failure'); END").unwrap();
    assert!(d
        .handle
        .select_model(Some(id.clone()), before.revision)
        .await
        .is_err());
    d.wait(|s| {
        s.can_start && s.model_operation.is_none() && s.worker.model_id.as_deref() == Some(ORIGINAL)
    })
    .await;
    assert_eq!(d.handle.snapshot().unwrap().settings, before);
    fault
        .execute_batch("DROP TRIGGER reject_candidate")
        .unwrap();
    drop(fault);
    stage("load_candidate");
    d.select(Some(&id)).await;
    assert_eq!(
        d.handle.delete_model(id.clone()).await.unwrap_err().code,
        "ModelInUse"
    );
    let selected = d.handle.snapshot().unwrap().settings.clone();
    stage("restart_with_candidate_selected");
    d.close().await;
    d = Desktop::open(temp.path(), catalog.clone()).await;
    d.wait(|s| s.can_start).await;
    assert_eq!(d.handle.snapshot().unwrap().settings, selected);
    assert_eq!(
        d.handle.snapshot().unwrap().worker.model_id.as_deref(),
        Some(id.as_str())
    );
    let mut expected = None;
    for repeat in 1..=3 {
        stage(&format!("candidate_en300_{repeat}"));
        let r = d.record(true, &id, repeat).await;
        assert!(r.needs_review);
        assert_eq!(r.raw_text.to_lowercase().matches("tribal").count(), 37);
        if let Some(ref text) = expected {
            assert_eq!(&r.raw_text, text);
        } else {
            expected = Some(r.raw_text);
        }
    }
    stage("cancel_processing");
    let cancelled = d.start(true).await;
    assert_eq!(
        d.handle
            .select_model(Some(ORIGINAL.into()), selected.revision)
            .await
            .unwrap_err()
            .code,
        "Busy"
    );
    d.handle.stop(cancelled, false).await.unwrap();
    d.wait(|s| s.session.phase == Phase::Transcribing).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let cancel_started = Instant::now();
    d.handle.stop(cancelled, true).await.unwrap();
    let ready = d
        .wait(|s| s.can_start && s.session.session_id.is_none())
        .await;
    assert_eq!(ready.usage_today, 3);
    assert_eq!(ready.recent_results.len(), 3);
    assert!(!d
        .handle
        .database(move |s| s.usage_recorded(cancelled))
        .await
        .unwrap());
    mark(json!({"cancel_to_ready_ms":cancel_started.elapsed().as_millis(),"quota_unchanged":true}));
    stage("after_cancel_short");
    d.record(false, &id, 4).await;
    stage("corrupt_candidate_restore_original");
    d.select(Some(ORIGINAL)).await;
    let before = d.handle.snapshot().unwrap().settings;
    let lease = d.models.lease(&id).await.unwrap();
    let tokens = lease.bundle.directory.join("tokens.txt");
    drop(lease);
    let original_tokens = fs::read(&tokens).unwrap();
    let mut bad = original_tokens.clone();
    bad[0] ^= 1;
    fs::write(&tokens, &bad).unwrap();
    assert!(d
        .handle
        .select_model(Some(id.clone()), before.revision)
        .await
        .is_err());
    d.wait(|s| s.can_start && s.model_operation.is_none()).await;
    assert_eq!(d.handle.snapshot().unwrap().settings, before);
    stage("after_rejected_candidate");
    d.record(false, ORIGINAL, 5).await;
    // Restore only this disposable installed test file. Reopen performs full
    // revalidation before selection; no corruption is injected into cached files.
    fs::write(&tokens, original_tokens).unwrap();
    stage("restart_original_after_repair");
    d.close().await;
    d = Desktop::open(temp.path(), catalog.clone()).await;
    d.wait(|s| s.can_start).await;
    assert_eq!(
        d.handle
            .snapshot()
            .unwrap()
            .settings
            .active_model_id
            .as_deref(),
        Some(ORIGINAL)
    );
    assert_eq!(d.handle.snapshot().unwrap().usage_today, 5);
    assert!(d.handle.snapshot().unwrap().recent_results.is_empty());
    stage("import_whisper");
    d.import(SMALL, files(SMALL)).await;
    stage("switch_whisper");
    d.select(Some(SMALL)).await;
    d.record(false, SMALL, 6).await;
    stage("return_candidate");
    d.select(Some(&id)).await;
    d.record(false, &id, 7).await;
    stage("unload_and_remove_candidate");
    d.select(None).await;
    d.handle.delete_model(id.clone()).await.unwrap();
    assert!(d.models.lease(&id).await.is_err());
    assert!(d.models.lease(ORIGINAL).await.is_ok());
    assert!(graph.is_file());
    stage("final_restart");
    d.close().await;
    d = Desktop::open(temp.path(), catalog).await;
    let s = d.handle.snapshot().unwrap();
    assert!(s.settings.active_model_id.is_none());
    assert_eq!(s.usage_today, 7);
    assert!(s.recent_results.is_empty());
    let (history, integrity) = d
        .handle
        .database(|s| {
            Ok((
                s.history("", None)?.items.len(),
                s.settings()?.history_enabled,
            ))
        })
        .await
        .unwrap();
    assert_eq!((history, integrity), (0, false));
    d.close().await;
    let db = rusqlite::Connection::open(temp.path().join("store.sqlite3")).unwrap();
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM usage_events", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        7
    );
    mark(
        json!({"completed":true,"candidate_id":id,"candidate_sha256":SENSEVOICE_TILED_SHA256,"full300_completed":3,"successful_results":7,"history_rows":0,"usage_events":7,"sqlite_integrity":"ok","microphone_used":false,"gui_included":false}),
    );
    stage("finished");
}
