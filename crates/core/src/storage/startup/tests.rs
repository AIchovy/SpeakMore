use super::*;

fn old(path: &Path, version: i64) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(MIGRATION).unwrap();
    for sql in MIGRATIONS.iter().take((version - 1) as usize) {
        conn.execute_batch(sql).unwrap();
    }
    conn.execute_batch("PRAGMA wal_autocheckpoint=0; UPDATE settings SET custom_prompt='fixture <private> 中文',history_enabled=1,revision=9; INSERT INTO dictionary_entries(entry_id,term,normalized_key,locale,enabled,pinned,created_at,updated_at) VALUES('fixture','SpeakMore','speakmore','all',1,0,1,1);").unwrap();
    conn
}

fn code<T>(result: Result<T>) -> String {
    result.err().expect("expected failure").code
}

#[test]
fn activity_upgrade_recovers_available_counts_without_inventing_missing_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("activity.sqlite3");
    let original = old(&path, 8);
    let known = uuid::Uuid::new_v4().to_string();
    let unknown = uuid::Uuid::new_v4().to_string();
    original.execute("INSERT INTO usage_events(session_id,quota_day,created_at) VALUES(?1,'2023-11-14',1700000000000), (?2,'2023-11-14',1700000001000)", rusqlite::params![known, unknown]).unwrap();
    original.execute("INSERT INTO transcriptions(session_id,raw_text,model_id,model_sha256,dictionary_revision,language,duration_ms,asr_ms,polish_outcome,delivery_status,created_at,needs_review) VALUES(?1,'你好 A！', 'test',?2,1,'zh',2000,10,'off','ready_for_copy',1700000000000,0)",rusqlite::params![known, "a".repeat(64)]).unwrap();
    drop(original);
    let mut upgraded = Store::open(&path).unwrap();
    let stats = upgraded.activity_overview(0, i64::MAX).unwrap();
    assert_eq!(stats.days.iter().map(|d| d.count).sum::<u64>(), 2);
    assert_eq!(stats.days.iter().map(|d| d.characters).sum::<u64>(), 3);
    assert_eq!(
        stats.days.iter().map(|d| d.unknown_characters).sum::<u64>(),
        1
    );
    upgraded.clear_history().unwrap();
    drop(upgraded);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened
            .activity_overview(0, i64::MAX)
            .unwrap()
            .days
            .iter()
            .map(|d| d.count)
            .sum::<u64>(),
        2
    );
}

#[test]
fn upgrade_scrubs_previously_deleted_text_and_flushes_before_removing_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scrub.sqlite3");
    let wal = dir.path().join("scrub.sqlite3-wal");
    let marker = "deleted-before-upgrade-5d1c";
    let original = old(&path, 9);
    original.execute("INSERT INTO transcriptions(session_id,raw_text,model_id,model_sha256,dictionary_revision,language,duration_ms,asr_ms,polish_outcome,delivery_status,created_at,needs_review) VALUES(?1,?2,'test',?3,1,'en',1000,10,'off','ready_for_copy',1,0)", rusqlite::params![uuid::Uuid::new_v4().to_string(), format!("{marker} {}", "x".repeat(3000)), "a".repeat(64)]).unwrap();
    original
        .execute_batch(
            "PRAGMA wal_checkpoint(TRUNCATE); DELETE FROM transcriptions; PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .unwrap();
    drop(original);
    let contains = |file: &Path| {
        fs::read(file)
            .is_ok_and(|bytes| bytes.windows(marker.len()).any(|w| w == marker.as_bytes()))
    };
    assert!(contains(&path));
    let store = Store::open(&path).unwrap();
    assert_eq!(version(&store.conn).unwrap(), VERSION);
    assert_eq!(fs::metadata(&wal).map_or(0, |m| m.len()), 0);
    assert!(!contains(&path) && !contains(&wal));
    assert!(!store.migration_backup_retained());
    assert!(!recovery_directory(&path)
        .join(format!("before-v9-to-v{VERSION}.sqlite3"))
        .exists());
    let secure: i64 = store
        .conn
        .query_row("PRAGMA secure_delete", [], |r| r.get(0))
        .unwrap();
    assert_eq!(secure, 1);
}

#[test]
fn initial_ui_language_is_atomic_and_never_reapplied_on_reopen_or_upgrade() {
    for initial in ["en", "zh-CN"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.sqlite3");
        let opposite = if initial == "en" { "zh-CN" } else { "en" };
        let mut store = Store::open_with_initial_locale(&path, initial).unwrap();
        let first = store.settings().unwrap();
        assert_eq!(first.ui_locale, initial);
        assert_eq!(first.language, "auto");
        assert_eq!(first.revision, 1);
        assert!(!first.polish_enabled);
        assert_eq!(first.processing_policy, "local_only");
        store
            .update_settings(
                first.revision,
                crate::types::SettingsPatch {
                    ui_locale: Some(opposite.into()),
                    ..Default::default()
                },
                1,
            )
            .unwrap();
        let revision = store.settings().unwrap().revision;
        drop(store);
        let reopened = Store::open_with_initial_locale(&path, initial).unwrap();
        assert_eq!(reopened.settings().unwrap().ui_locale, opposite);
        assert_eq!(reopened.settings().unwrap().revision, revision);
        integrity(&reopened.conn).unwrap();

        let path = dir.path().join("old.sqlite3");
        let previous = old(&path, 6);
        previous
            .execute("UPDATE settings SET ui_locale=?1", [initial])
            .unwrap();
        drop(previous);
        let upgraded = Store::open_with_initial_locale(&path, opposite).unwrap();
        assert_eq!(upgraded.settings().unwrap().ui_locale, initial);
        assert_eq!(upgraded.settings().unwrap().revision, 9);
        assert_eq!(version(&upgraded.conn).unwrap(), VERSION);
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("invalid.sqlite3");
    assert_eq!(
        code(Store::open_with_initial_locale(&path, "fr")),
        "InvalidInput"
    );
    assert!(!path.exists());
}

#[test]
fn v6_polish_migration_preserves_settings_and_failed_final_validation_rolls_back() {
    for broken in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("polish.sqlite3");
        let original = old(&path, 6);
        original
            .execute_batch("UPDATE settings SET polish_enabled=1;")
            .unwrap();
        if broken {
            original
                .execute_batch("ALTER TABLE transcriptions RENAME COLUMN raw_text TO broken_raw;")
                .unwrap();
        }
        if broken {
            assert_eq!(code(Store::open(&path)), "DatabaseMigrationFailed");
            assert_eq!(version(&original).unwrap(), 6);
            assert!(original
                .prepare("SELECT polish_reason FROM transcriptions")
                .is_err());
            let backup = Connection::open(
                recovery_directory(&path).join(format!("before-v6-to-v{VERSION}.sqlite3")),
            )
            .unwrap();
            integrity(&backup).unwrap();
            assert_eq!(version(&backup).unwrap(), 6);
            assert!(Store::read_settings(&backup).unwrap().polish_enabled);
        } else {
            drop(original);
            let store = Store::open(&path).unwrap();
            assert_eq!(version(&store.conn).unwrap(), VERSION);
            assert!(store.settings().unwrap().polish_enabled);
            assert_eq!(
                store.settings().unwrap().custom_prompt,
                "fixture <private> 中文"
            );
            assert!(store
                .conn
                .prepare("SELECT polish_reason FROM transcriptions")
                .is_ok());
        }
    }
}

#[test]
fn v5_test_timestamps_upgrade_without_granting_durable_qualification() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("checkpoint.sqlite3");
    let original = old(&path, 5);
    let id = uuid::Uuid::new_v4().to_string();
    original.execute("INSERT INTO providers(provider_id,kind,endpoint,model_name,display_name,request_profile_json,tested_at,updated_at) VALUES(?1,'ollama','http://127.0.0.1:11434/api/chat','fixture','Saved local AI','{\"kind\":\"ollama\"}',88,77)", [&id]).unwrap();
    original
        .execute(
            "UPDATE settings SET active_provider_id=?1,polish_enabled=1",
            [&id],
        )
        .unwrap();
    drop(original);
    let store = Store::open(&path).unwrap();
    assert_eq!(version(&store.conn).unwrap(), VERSION);
    let record = store.providers().unwrap().pop().unwrap();
    assert_eq!(record.tested_at, Some(88));
    assert!(record.test_checkpoint.is_none());
    assert_eq!(record.draft.display_name, "Saved local AI");
    assert_eq!(record.updated_at, 77);
    assert_eq!(record.revision, 1);
    let settings = store.settings().unwrap();
    assert!(!settings.polish_enabled);
    assert_eq!(settings.active_provider_id, Some(id));
    assert!(settings.history_enabled);
    assert_eq!(settings.custom_prompt, "fixture <private> 中文");
    assert!(!store.migration_backup_retained());
}

#[test]
fn final_v6_migration_failure_rolls_back_column_and_keeps_v5_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("checkpoint-failure.sqlite3");
    let original = old(&path, 5);
    original.execute_batch("UPDATE settings SET polish_enabled=1; CREATE TRIGGER reject_checkpoint_disable BEFORE UPDATE OF polish_enabled ON settings BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert_eq!(code(Store::open(&path)), "DatabaseMigrationFailed");
    assert_eq!(version(&original).unwrap(), 5);
    assert!(original
        .prepare("SELECT test_checkpoint FROM providers")
        .is_err());
    assert!(Store::read_settings(&original).unwrap().polish_enabled);
    let backup = recovery_directory(&path).join(format!("before-v5-to-v{VERSION}.sqlite3"));
    let copied = dir.path().join("standalone.sqlite3");
    fs::copy(backup, &copied).unwrap();
    let restored = Connection::open(copied).unwrap();
    integrity(&restored).unwrap();
    assert_eq!(version(&restored).unwrap(), 5);
    assert!(Store::read_settings(&restored).unwrap().polish_enabled);
    assert_eq!(
        Store::read_settings(&restored).unwrap().custom_prompt,
        "fixture <private> 中文"
    );
}

#[test]
fn provider_upgrade_preserves_metadata_but_requires_new_test_and_explicit_polishing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("providers.sqlite3");
    let original = old(&path, 4);
    let provider = uuid::Uuid::new_v4();
    let local = uuid::Uuid::new_v4();
    original.execute("INSERT INTO providers(provider_id,kind,endpoint,model_name,consent_origin,tested_at,updated_at) VALUES(?1,'openai_compatible','https://example.invalid/v1/chat/completions','fixture-model','https://example.invalid',8,7)", [provider.to_string()]).unwrap();
    original.execute("INSERT INTO providers(provider_id,kind,endpoint,model_name,request_profile_json,tested_at,updated_at) VALUES(?1,'ollama','http://127.0.0.1:11434/api/chat','local-fixture','{\"kind\":\"ollama\"}',8,7)", [local.to_string()]).unwrap();
    original.execute("UPDATE settings SET active_provider_id=?1,polish_enabled=1,processing_policy='allow_cloud'", [provider.to_string()]).unwrap();
    drop(original);
    let store = Store::open(&path).unwrap();
    let settings = store.settings().unwrap();
    assert_eq!(version(&store.conn).unwrap(), VERSION);
    assert_eq!(settings.active_provider_id, Some(provider.to_string()));
    assert!(!settings.polish_enabled);
    assert_eq!(settings.processing_policy, "allow_cloud");
    assert_eq!(settings.custom_prompt, "fixture <private> 中文");
    assert_eq!(settings.revision, 9);
    let records = store.providers().unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| record.tested_at.is_none()));
    let compatible = records
        .iter()
        .find(|record| record.draft.provider_id == provider)
        .unwrap();
    assert_eq!(compatible.revision, 1);
    assert_eq!(compatible.updated_at, 7);
    assert_eq!(compatible.draft.model_name, "fixture-model");
    assert_eq!(
        compatible.draft.consent_origin.as_deref(),
        Some("https://example.invalid")
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&compatible.draft.request_profile_json).unwrap(),
        serde_json::json!({"kind":"openai_compatible", "budget_field":"max_tokens", "instruction_role":"system"})
    );
    let local = records
        .iter()
        .find(|record| record.draft.provider_id == local)
        .unwrap();
    assert_eq!(local.draft.display_name, "Ollama");
    assert_eq!(local.draft.request_profile_json, "{\"kind\":\"ollama\"}");
    assert!(store.pending_provider_credentials().unwrap().is_empty());
    assert!(!store.migration_backup_retained());
}

#[test]
fn provider_upgrade_failure_restores_v4_flags_and_keeps_a_standalone_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("providers.sqlite3");
    let original = old(&path, 4);
    let id = uuid::Uuid::new_v4().to_string();
    original.execute("INSERT INTO providers(provider_id,kind,endpoint,model_name,tested_at,updated_at) VALUES(?1,'ollama','http://127.0.0.1:11434/api/chat','fixture',8,7)", [&id]).unwrap();
    original
        .execute(
            "UPDATE settings SET active_provider_id=?1,polish_enabled=1",
            [&id],
        )
        .unwrap();
    // Fail the final CREATE after the migration has changed columns, profiles and settings.
    original.execute_batch("CREATE TABLE provider_credentials_pending (fixture TEXT); INSERT INTO provider_credentials_pending VALUES('keep');").unwrap();
    assert_eq!(code(Store::open(&path)), "DatabaseMigrationFailed");
    assert_eq!(version(&original).unwrap(), 4);
    assert!(original
        .prepare("SELECT display_name FROM providers")
        .is_err());
    assert!(Store::read_settings(&original).unwrap().polish_enabled);
    assert_eq!(
        original
            .query_row(
                "SELECT request_profile_json,tested_at FROM providers",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            )
            .unwrap(),
        ("{}".into(), 8)
    );
    let backup = recovery_directory(&path).join(format!("before-v4-to-v{VERSION}.sqlite3"));
    let isolated = dir.path().join("restored.sqlite3");
    fs::copy(&backup, &isolated).unwrap();
    let restored = Connection::open(&isolated).unwrap();
    integrity(&restored).unwrap();
    assert_eq!(version(&restored).unwrap(), 4);
    assert_eq!(
        Store::read_settings(&restored)
            .unwrap()
            .active_provider_id
            .as_deref(),
        Some(id.as_str())
    );
    assert!(Store::read_settings(&restored).unwrap().polish_enabled);
    assert_eq!(
        restored
            .query_row(
                "SELECT fixture FROM provider_credentials_pending",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "keep"
    );
    original
        .execute_batch("DROP TABLE provider_credentials_pending;")
        .unwrap();
    drop(original);
    let upgraded = Store::open(&path).unwrap();
    assert!(!upgraded.settings().unwrap().polish_enabled);
    assert!(!backup.exists());
}

#[test]
fn whole_upgrade_rolls_back_and_verified_backup_contains_uncheckpointed_wal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite3");
    let original = old(&path, 1);
    // Fail migration 004, after 002 and 003 have already executed in the transaction.
    original
        .execute_batch(
            "CREATE TABLE app_rules_v4 (fixture TEXT); INSERT INTO app_rules_v4 VALUES('keep');",
        )
        .unwrap();
    assert!(
        fs::metadata(path.with_file_name("data.sqlite3-wal"))
            .unwrap()
            .len()
            > 0
    );
    assert_eq!(code(Store::open(&path)), "DatabaseMigrationFailed");
    assert_eq!(version(&original).unwrap(), 1);
    assert!(original
        .prepare("SELECT needs_review FROM transcriptions")
        .is_err());
    let backup = recovery_directory(&path).join(format!("before-v1-to-v{VERSION}.sqlite3"));
    let restored = dir.path().join("isolated-restored.sqlite3");
    fs::copy(&backup, &restored).unwrap(); // A standalone backup must not require WAL sidecars.
    let restored = Connection::open(&restored).unwrap();
    integrity(&restored).unwrap();
    assert_eq!(version(&restored).unwrap(), 1);
    assert_eq!(
        restored
            .query_row("SELECT custom_prompt FROM settings", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "fixture <private> 中文"
    );
    assert_eq!(
        restored
            .query_row("SELECT term FROM dictionary_entries", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "SpeakMore"
    );
    assert_eq!(
        restored
            .query_row("SELECT fixture FROM app_rules_v4", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "keep"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(recovery_directory(&path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    // Retrying the same failing upgrade replaces one verified backup, without growing a list.
    assert_eq!(code(Store::open(&path)), "DatabaseMigrationFailed");
    assert_eq!(fs::read_dir(recovery_directory(&path)).unwrap().count(), 1);
    original.execute_batch("DROP TABLE app_rules_v4;").unwrap();
    drop(original);
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.settings().unwrap().custom_prompt,
        "fixture <private> 中文"
    );
    assert!(!store.migration_backup_retained());
    assert!(!backup.exists());
}

#[test]
fn backup_failure_stops_before_any_migration_or_existing_backup_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite3");
    let original = old(&path, 2);
    fs::write(recovery_directory(&path), "unrelated").unwrap();
    assert_eq!(code(Store::open(&path)), "DatabaseBackupFailed");
    assert_eq!(version(&original).unwrap(), 2);
    assert!(original
        .prepare("SELECT delivery_reason FROM transcriptions")
        .is_err());
    assert_eq!(
        fs::read_to_string(recovery_directory(&path)).unwrap(),
        "unrelated"
    );
    drop(original);
    // No false backup-success message or file paths / private values in errors.
    let error = Store::open(&path).err().unwrap();
    let encoded = serde_json::to_string(&error).unwrap();
    assert!(!encoded.contains("fixture"));
    assert!(!encoded.contains(&path.to_string_lossy().to_string()));
}

#[test]
fn newer_unknown_and_corrupt_databases_are_never_initialized_or_rejournaled() {
    let dir = tempfile::tempdir().unwrap();
    for (name, sql, error) in [
        (
            "newer",
            "PRAGMA user_version=99; CREATE TABLE secret(x); INSERT INTO secret VALUES('keep');",
            "NewerDatabase",
        ),
        (
            "unknown",
            "CREATE TABLE secret(x); INSERT INTO secret VALUES('keep');",
            "UnknownDatabase",
        ),
    ] {
        let path = dir.path().join(name);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(sql).unwrap();
        drop(conn);
        let before = fs::read(&path).unwrap();
        assert_eq!(code(Store::open(&path)), error);
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!recovery_directory(&path).exists());
    }
    let corrupt = dir.path().join("corrupt");
    fs::write(&corrupt, b"not a SQLite database: keep me").unwrap();
    assert_eq!(code(Store::open(&corrupt)), "DatabaseCorrupt");
    assert_eq!(
        fs::read(&corrupt).unwrap(),
        b"not a SQLite database: keep me"
    );
}

#[test]
fn healthy_reopen_cleans_crash_backups_but_preserves_unknown_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite3");
    drop(Store::open(&path).unwrap());
    assert!(!recovery_directory(&path).exists());
    let recovery = recovery_directory(&path);
    fs::create_dir(&recovery).unwrap();
    let pending = recovery.join(format!("pending-{}.sqlite3", uuid::Uuid::new_v4()));
    fs::write(&pending, b"interrupted copy").unwrap();
    let backup = recovery.join("before-v3-to-v4.sqlite3");
    fs::write(&backup, b"old fixture").unwrap();
    let unknown = recovery.join("my-notes.txt");
    fs::write(&unknown, b"keep").unwrap();
    let store = Store::open(&path).unwrap();
    assert!(!store.migration_backup_retained());
    assert!(!pending.exists());
    assert!(!backup.exists());
    assert_eq!(fs::read(&unknown).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn links_and_special_files_are_rejected_and_cleanup_failure_is_reported() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    drop(Store::open(&real).unwrap());
    assert_eq!(
        fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let link = dir.path().join("link");
    symlink(&real, &link).unwrap();
    assert_eq!(code(Store::open(&link)), "StorageUnavailable");
    fs::remove_file(&link).unwrap();
    fs::hard_link(&real, &link).unwrap();
    assert_eq!(code(Store::open(&link)), "StorageUnavailable");
    fs::remove_file(&link).unwrap();
    let recovery = recovery_directory(&real);
    fs::create_dir(&recovery).unwrap();
    symlink(&real, recovery.join("before-v3-to-v4.sqlite3")).unwrap();
    assert!(Store::open(&real).unwrap().migration_backup_retained());
    assert!(
        fs::symlink_metadata(recovery.join("before-v3-to-v4.sqlite3"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let old_path = dir.path().join("old");
    drop(old(&old_path, 3));
    symlink(&recovery, recovery_directory(&old_path)).unwrap();
    assert_eq!(code(Store::open(&old_path)), "DatabaseBackupFailed");
    assert_eq!(version(&Connection::open(&old_path).unwrap()).unwrap(), 3);
}

#[test]
fn post_migration_validation_failure_rolls_back_settings_and_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite3");
    let original = old(&path, 3);
    original
        .execute(
            "INSERT INTO shortcuts(action,accelerator,enabled) VALUES('push_to_talk','x',1)",
            [],
        )
        .unwrap();
    assert_eq!(code(Store::open(&path)), "DatabaseMigrationFailed");
    assert_eq!(version(&original).unwrap(), 3);
    assert!(original
        .prepare("SELECT display_name FROM app_rules")
        .is_err());
    assert!(recovery_directory(&path)
        .join(format!("before-v3-to-v{VERSION}.sqlite3"))
        .exists());
}

#[test]
fn missing_or_empty_primary_never_erases_existing_recovery_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.sqlite3");
    let recovery = recovery_directory(&path);
    fs::create_dir(&recovery).unwrap();
    let backup = recovery.join("before-v3-to-v4.sqlite3");
    fs::write(&backup, b"only remaining copy").unwrap();
    assert_eq!(code(Store::open(&path)), "DatabaseRecoveryRequired");
    assert!(!path.exists());
    fs::write(&path, []).unwrap();
    assert_eq!(code(Store::open(&path)), "DatabaseRecoveryRequired");
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    assert_eq!(fs::read(&backup).unwrap(), b"only remaining copy");
}

#[test]
fn current_schema_missing_required_table_keeps_recovery_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("data.sqlite3");
    drop(Store::open(&path).unwrap());
    let recovery = recovery_directory(&path);
    fs::create_dir(&recovery).unwrap();
    let backup = recovery.join("before-v3-to-v4.sqlite3");
    fs::write(&backup, b"keep recovery copy").unwrap();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("DROP TABLE dictionary_entries;")
        .unwrap();
    drop(conn);
    assert_eq!(code(Store::open(&path)), "UnknownDatabase");
    assert_eq!(fs::read(&backup).unwrap(), b"keep recovery copy");
}
