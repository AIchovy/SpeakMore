//! Explicit child-process diagnostic, not a product entry point. Uses synthetic
//! bytes, the real installer/SqliteModels/Store, and an isolated marked directory.
//! A parent may SIGKILL this test at public progress/registry boundaries or fill
//! a bounded test volume. It never opens a microphone, worker or user database.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use speakmore_core::types::{DictionaryInput, ResultRecord, SettingsPatch};
use speakmore_models::{
    catalog::{Envelope, Model, Payload, TrustKey, TrustedCatalog},
    InstallState, ModelManager,
};
use std::{
    fs,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use uuid::Uuid;

mod database;

const PREFIX: &str = "SPEAKMORE_FS_FAULT ";
const NEXT: &str = "filesystem-fault-next";
const NOW: i64 = 1_789_000_000_000;
const NEW_BYTES: usize = 32 * 1024 * 1024;

fn emit(value: Value) {
    println!("{PREFIX}{value}");
    std::io::stdout().flush().unwrap();
}
struct Gate {
    phase: String,
    fired: AtomicBool,
}
impl Gate {
    fn pause(&self, phase: &str) {
        if self.phase != phase || self.fired.swap(true, Ordering::AcqRel) {
            return;
        }
        emit(json!({"checkpoint": phase}));
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer).unwrap();
        assert_eq!(
            answer.trim(),
            "continue",
            "parent must continue or kill this explicit child"
        );
    }
}
struct GatedRegistry {
    actual: SqliteModels,
    gate: Arc<Gate>,
}
impl Registry for GatedRegistry {
    fn installed(&self, bundle: &InstalledBundle) -> Result<()> {
        if bundle.model.version.as_deref() == Some(NEXT) {
            self.gate.pause("before-registration");
        }
        self.actual.installed(bundle)?;
        if bundle.model.version.as_deref() == Some(NEXT) {
            self.gate.pause("after-registration");
        }
        Ok(())
    }
    fn removed(&self, id: &str) -> Result<()> {
        self.actual.removed(id)
    }
}

fn catalog() -> (TrustedCatalog, Model, Model) {
    let mut payload: Payload = serde_json::from_slice(include_bytes!(
        "../../../resources/model-catalog.payload.json"
    ))
    .unwrap();
    payload.models.truncate(1);
    let mut old = payload.models.remove(0);
    for artifact in &mut old.artifacts {
        artifact.size_bytes = 4096;
        artifact.sha256 = format!("{:x}", Sha256::digest(vec![0x51; 4096]));
    }
    let mut new = old.clone();
    new.version = Some(NEXT.into());
    for artifact in &mut new.artifacts {
        artifact.size_bytes = if artifact.role == "model" {
            NEW_BYTES as u64
        } else {
            4096
        };
        artifact.sha256 = format!(
            "{:x}",
            Sha256::digest(vec![0x72; artifact.size_bytes as usize])
        );
    }
    payload.models = vec![old.clone(), new.clone()];
    let key = SigningKey::from_bytes(&[81; 32]);
    let raw = serde_json::to_vec(&payload).unwrap();
    let envelope = Envelope {
        key_id: "filesystem-fault-public-test-only".into(),
        payload: STANDARD.encode(&raw),
        signature: STANDARD.encode(key.sign(&raw).to_bytes()),
    };
    let trust = TrustKey {
        key_id: envelope.key_id.clone(),
        public_key: STANDARD.encode(key.verifying_key().to_bytes()),
    };
    (
        TrustedCatalog::verify(&serde_json::to_vec(&envelope).unwrap(), &trust).unwrap(),
        old,
        new,
    )
}

fn fixture_root() -> PathBuf {
    let root = PathBuf::from(
        std::env::var_os("SPEAKMORE_FS_FAULT_ROOT").expect("explicit marked root required"),
    );
    let allowed = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".cache/fs-fault-study");
    assert!(root.is_absolute() && root.starts_with(&allowed));
    assert!(root.ancestors().all(|p| !p.is_symlink()));
    assert_eq!(root.canonicalize().unwrap(), root);
    assert_eq!(
        fs::read(root.join(".speakmore-fault-fixture")).unwrap(),
        b"synthetic-filesystem-check-v1\n"
    );
    root
}
fn paths(root: &Path, model: &Model) -> Vec<PathBuf> {
    let version = if model.version.is_some() {
        "new"
    } else {
        "old"
    };
    model
        .artifacts
        .iter()
        .map(|a| root.join("sources").join(version).join(&a.filename))
        .collect()
}
async fn settled(manager: &ModelManager) {
    tokio::time::timeout(Duration::from_secs(90), async {
        while manager.snapshot().busy {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("installer must release its task slot");
}

#[test]
#[ignore = "child process only; use scripts/diagnostics/macos-filesystem-faults.py"]
fn isolated_filesystem_child() {
    let root = fixture_root();
    let action = std::env::var("SPEAKMORE_FS_FAULT_ACTION").unwrap();
    let phase = std::env::var("SPEAKMORE_FS_FAULT_PHASE").unwrap_or_default();
    if action.starts_with("db-") {
        let gate = Gate {
            phase,
            fired: AtomicBool::new(false),
        };
        database::run(&root, &action, &gate);
        return;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (catalog, old, new) = catalog();
        let db = root.join("data.sqlite3");
        if action == "seed" {
            assert!(!db.exists());
            for model in [&old, &new] {
                for (a, path) in model.artifacts.iter().zip(paths(&root, model)) {
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(path).unwrap();
                    file.write_all(&vec![if model.version.is_some() { 0x72 } else { 0x51 }; a.size_bytes as usize]).unwrap();
                    file.sync_all().unwrap();
                }
            }
        } else {
            assert!(db.is_file());
        }
        let store = Arc::new(Mutex::new(Ok(Store::open(&db).unwrap())));
        let gate = Arc::new(Gate { phase, fired: AtomicBool::new(false) });
        let registry = Arc::new(GatedRegistry {
            actual: SqliteModels { store: store.clone(), changed: Box::new(|| {}) },
            gate: gate.clone(),
        });
        let observed = Arc::new(Mutex::new(None::<ModelManager>));
        let observe = observed.clone();
        let progress_gate = gate.clone();
        let next_id = new.installation_id();
        let notify = Arc::new(move |_| {
            let state = observe.lock().unwrap().as_ref().map(ModelManager::snapshot);
            if let Some(row) = state.as_ref().and_then(|s| s.models.iter().find(|r| r.model_id == next_id)) {
                if row.state == InstallState::Importing && row.received_bytes > 0 {
                    progress_gate.pause("copying");
                }
                if row.state == InstallState::Installing {
                    progress_gate.pause("before-rename");
                }
            }
        });
        let manager = ModelManager::open(root.join("models"), catalog, registry, notify).unwrap();
        *observed.lock().unwrap() = Some(manager.clone());
        manager.initialize().await.unwrap();
        match action.as_str() {
            "seed" => {
                manager.start_import(&old.installation_id(), manager.prepare_import(&old.installation_id(), paths(&root, &old)).unwrap()).unwrap();
                settled(&manager).await;
                assert_eq!(manager.snapshot().models[0].state, InstallState::Installed);
                {
                    let mut lock = store.lock().unwrap();
                    let store = lock.as_mut().unwrap();
                    // Synthetic prior selection, not an ASR/Ready qualification.
                    store.activate_model(Some(&old.installation_id()), Some(&old.bundle_digest()), None, NOW).unwrap();
                    let s = store.settings().unwrap();
                    store.update_settings(s.revision, SettingsPatch {
                        history_enabled: Some(true), delivery_mode: Some("manual".into()),
                        custom_prompt: Some("Synthetic filesystem fault fixture".into()),
                        ..Default::default()
                    }, NOW).unwrap();
                    store.upsert_dictionary(store.settings().unwrap().revision, DictionaryInput {
                        entry_id: None, term: "SyntheticTerm".into(), locale: "all".into(), enabled: true, pinned: true,
                    }, NOW).unwrap();
                    let record = ResultRecord {
                        session_id: Uuid::from_u128(1), raw_text: "Synthetic saved history, not microphone data.".into(),
                        polished_text: None, model_id: old.installation_id(), model_sha256: old.bundle_digest(),
                        dictionary_revision: store.settings().unwrap().revision, language: "en".into(),
                        duration_ms: 300_000, asr_ms: 1, needs_review: false, polish_ms: None,
                        polish_outcome: "off".into(), polish_reason: None,
                        delivery_status: "ready_for_copy".into(), delivery_reason: None, app_identity: None, created_at: NOW,
                    };
                    assert!(store.persist_generated(&record, "2026-09-16", true, false).unwrap());
                }
                fs::write(root.join("models/staging/keep-unknown.txt"), b"preserve unknown file").unwrap();
                emit(json!({"seeded": true, "old_id": old.installation_id(), "new_id": new.installation_id(), "old_digest": old.bundle_digest(), "new_digest": new.bundle_digest()}));
            }
            "import" => {
                manager.start_import(&new.installation_id(), manager.prepare_import(&new.installation_id(), paths(&root, &new)).unwrap()).unwrap();
                settled(&manager).await;
            }
            "reconcile" => (),
            _ => panic!("unknown diagnostic action"),
        }
        emit(json!({"done": action, "snapshot": manager.snapshot(), "settings": store.lock().unwrap().as_ref().unwrap().settings().unwrap()}));
        *observed.lock().unwrap() = None;
        drop(manager);
    });
}
