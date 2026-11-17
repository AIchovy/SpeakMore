//! Synthetic database-only actions for the explicit child-process diagnostic.
use super::{emit, Gate, NOW};
use rusqlite::{backup::Backup, params, Connection};
use serde_json::json;
use speakmore_core::{
    storage::{recovery_directory, Store},
    types::ResultRecord,
    MAX_TEXT_BYTES,
};
use std::{fs, path::Path, time::Duration};
use uuid::Uuid;

const DAY: &str = "2026-09-16";
const SCHEMAS: [&str; 6] = [
    include_str!("../../../../migrations/001_initial.sql"),
    include_str!("../../../../migrations/002_result_review.sql"),
    include_str!("../../../../migrations/003_delivery_reason.sql"),
    include_str!("../../../../migrations/004_app_rules.sql"),
    include_str!("../../../../migrations/005_provider_credentials.sql"),
    include_str!("../../../../migrations/006_provider_test_checkpoint.sql"),
];

fn seed(root: &Path, legacy: bool) {
    let path = root.join("data.sqlite3");
    assert!(!path.exists());
    if !legacy {
        drop(Store::open(&path).unwrap());
    }
    let mut conn = Connection::open(&path).unwrap();
    if legacy {
        for sql in SCHEMAS {
            conn.execute_batch(sql).unwrap();
        }
    }
    let tx = conn.transaction().unwrap();
    tx.execute_batch("UPDATE settings SET history_enabled=1,history_limit=5000,delivery_mode='manual',custom_prompt='Synthetic database fault fixture';").unwrap();
    tx.execute("INSERT INTO dictionary_entries(entry_id,term,normalized_key,locale,enabled,pinned,created_at,updated_at) VALUES(?1,'SyntheticTerm','syntheticterm','all',1,1,?2,?2)",params![Uuid::from_u128(50).to_string(),NOW]).unwrap();
    tx.execute("INSERT INTO app_rules(platform,app_identity,manual_only,history_excluded,display_name) VALUES('macos','com.speakmore.syntheticfixture',1,1,'Synthetic fixture')",[]).unwrap();
    tx.execute("INSERT INTO license(id,signed_token,imported_at) VALUES(1,'obsolete-synthetic-license',?1)",[NOW]).unwrap();
    let rows = if legacy { 1024 } else { 1 };
    for index in 0..rows {
        let text = if legacy {
            format!("Synthetic backup row {index:04}: {}", "x".repeat(16_000))
        } else {
            "Synthetic preserved history".into()
        };
        let id = Uuid::from_u128(100 + index).to_string();
        tx.execute("INSERT INTO transcriptions(session_id,raw_text,model_id,model_sha256,dictionary_revision,language,duration_ms,asr_ms,polish_outcome,delivery_status,created_at,needs_review) VALUES(?1,?2,'synthetic-model',?3,1,'en',300000,1,'off','ready_for_copy',?4,0)",params![id,text,"a".repeat(64),NOW]).unwrap();
        tx.execute(
            "INSERT INTO usage_events(session_id,quota_day,created_at) VALUES(?1,?2,?3)",
            params![id, DAY, NOW],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    if legacy {
        // Seed an existing, valid recovery copy using SQLite's backup API. A
        // failed replacement must not destroy this independently useful copy.
        let recovery = recovery_directory(&path);
        fs::create_dir(&recovery).unwrap();
        fs::write(recovery.join("keep-unknown.txt"), b"preserve recovery note").unwrap();
        let mut backup = Connection::open(recovery.join("before-v6-to-v7.sqlite3")).unwrap();
        Backup::new(&conn, &mut backup)
            .unwrap()
            .run_to_completion(256, Duration::from_millis(1), None)
            .unwrap();
        backup
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")
            .unwrap();
        backup.close().unwrap();
    }
    conn.close().unwrap();
    emit(json!({"seeded_database":true,"version":version,"rows":rows}));
}

pub(super) fn run(root: &Path, action: &str, gate: &Gate) {
    let path = root.join("data.sqlite3");
    match action {
        "db-seed-current" => seed(root, false),
        "db-seed-v6" => seed(root, true),
        "db-open" => {
            assert!(path.is_file());
            gate.pause("before-open");
            match Store::open(&path) {
                Ok(store) => emit(
                    json!({"opened":true,"settings":store.settings().unwrap(),"backup_cleanup_pending":store.migration_backup_retained()}),
                ),
                Err(error) => emit(json!({"opened":false,"error":error})),
            }
        }
        "db-write-history" => {
            assert!(path.is_file());
            let mut store = Store::open(&path).unwrap();
            let record = ResultRecord {
                session_id: Uuid::from_u128(2),
                raw_text: "s".repeat(MAX_TEXT_BYTES),
                polished_text: None,
                model_id: "synthetic-model".into(),
                model_sha256: "a".repeat(64),
                dictionary_revision: 1,
                language: "en".into(),
                duration_ms: 300_000,
                asr_ms: 1,
                needs_review: false,
                polish_ms: None,
                polish_outcome: "off".into(),
                polish_reason: None,
                delivery_status: "ready_for_copy".into(),
                delivery_reason: None,
                app_identity: None,
                created_at: NOW + 1,
            };
            gate.pause("before-write");
            match store.persist_generated(&record, DAY, true, false) {
                Ok(saved) => emit(json!({"write_ok":true,"new_history":saved})),
                Err(error) => emit(json!({"write_ok":false,"error":error})),
            }
            gate.pause("after-write");
            emit(
                json!({"usage_recorded":store.usage_recorded(record.session_id),"history_present":store.find_result(record.session_id).map(|r|r.is_some())}),
            );
        }
        _ => panic!("unknown database diagnostic action"),
    }
}
