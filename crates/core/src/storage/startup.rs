//! Startup-only migration protection. No transcription or model bytes leave this process.
use super::{AppError, Connection, Result, Store, MIGRATION};
use rusqlite::{
    backup::{Backup, StepResult},
    OpenFlags, TransactionBehavior,
};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const VERSION: i64 = 10;
const MIGRATIONS: [&str; 9] = [
    include_str!("../../../../migrations/002_result_review.sql"),
    include_str!("../../../../migrations/003_delivery_reason.sql"),
    include_str!("../../../../migrations/004_app_rules.sql"),
    include_str!("../../../../migrations/005_provider_credentials.sql"),
    include_str!("../../../../migrations/006_provider_test_checkpoint.sql"),
    include_str!("../../../../migrations/007_polish_reason.sql"),
    include_str!("../../../../migrations/008_writing_rules.sql"),
    include_str!("../../../../migrations/009_activity.sql"),
    include_str!("../../../../migrations/010_storage_indexes.sql"),
];

pub fn recovery_directory(database: &Path) -> PathBuf {
    let mut name = database.file_name().unwrap_or_default().to_os_string();
    name.push(".recovery");
    database.with_file_name(name)
}

fn failure(code: &str) -> AppError {
    let message = match code {
        "DatabaseCorrupt" => "本地数据未通过完整性检查。原文件已保留，请先查看恢复说明。",
        "DatabaseBackupFailed" => "无法完成升级前备份，已停止数据升级。请检查可用空间和目录权限。",
        "DatabaseMigrationFailed" => "本地数据升级未完成，原文件已保留，未启动听写服务。",
        "DatabaseRecoveryRequired" => {
            "主数据库缺失或为空，但恢复目录中仍有文件。已停止初始化，请先检查备份。"
        }
        "NewerDatabase" => "数据来自更新版本，请使用对应版本的 SpeakMore。",
        "UnknownDatabase" => "本地数据结构无法识别，已保留原文件。",
        _ => "无法安全打开本地数据，请检查目录权限后重新打开应用。",
    };
    AppError::new(code, message, false)
}

fn regular(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
    }
    Ok(())
}

pub(super) fn canonical_path(path: &Path) -> Result<PathBuf> {
    // macOS /var and /tmp are legitimate directory aliases. Resolve the parent,
    // while retaining the final component so SQLite still rejects a linked database.
    let filename = path
        .file_name()
        .ok_or_else(|| failure("StorageUnavailable"))?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(parent
        .canonicalize()
        .map_err(|_| failure("StorageUnavailable"))?
        .join(filename))
}

pub(super) fn open_connection(path: &Path) -> Result<Connection> {
    if !path.exists() && recovery_has_files(path) {
        return Err(failure("DatabaseRecoveryRequired"));
    }
    let io = || -> std::io::Result<()> {
        // Create privately before SQLite opens it; never truncate an existing database.
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        match options.open(path) {
            Ok(file) => file.sync_all()?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => regular(path)?,
            Err(e) => return Err(e),
        }
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = path.as_os_str().to_os_string();
            sidecar.push(suffix);
            let sidecar = PathBuf::from(sidecar);
            match regular(&sidecar) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(path)?;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    };
    io().map_err(|_| failure("StorageUnavailable"))?;
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(Into::into)
}

fn version(conn: &Connection) -> Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| failure("DatabaseCorrupt"))
}

fn integrity(conn: &Connection) -> Result<()> {
    check(conn, "PRAGMA integrity_check(1)")?;
    let foreign: bool = conn
        .prepare("PRAGMA foreign_key_check")?
        .query([])?
        .next()?
        .is_some();
    if foreign {
        return Err(failure("DatabaseCorrupt"));
    }
    Ok(())
}

fn check(conn: &Connection, pragma: &str) -> Result<()> {
    let mut stmt = conn
        .prepare(pragma)
        .map_err(|_| failure("DatabaseCorrupt"))?;
    let mut rows = stmt.query([]).map_err(|_| failure("DatabaseCorrupt"))?;
    let valid = rows
        .next()
        .map_err(|_| failure("DatabaseCorrupt"))?
        .map(|row| row.get::<_, String>(0))
        .transpose()
        .map_err(|_| failure("DatabaseCorrupt"))?
        .as_deref()
        == Some("ok");
    if !valid {
        return Err(failure("DatabaseCorrupt"));
    }
    Ok(())
}

fn current_valid(conn: &Connection) -> Result<()> {
    if version(conn)? != VERSION {
        return Err(failure("UnknownDatabase"));
    }
    Store::read_settings(conn)?;
    Store::read_shortcuts(conn)?;
    super::app_rules::read(conn)?;
    super::writing::read(conn)?;
    // Validate the remaining required interfaces before cleanup can remove a recovery copy.
    // These prepare-only probes read no history, license tokens, or provider credentials.
    let tables: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('models','providers','settings','shortcuts','app_rules','dictionary_entries','transcriptions','usage_events','license','provider_credentials_pending')", [], |r| r.get(0))?;
    if tables != 10 {
        return Err(failure("UnknownDatabase"));
    }
    for sql in [
        "SELECT model_id,filename,revision,sha256,size_bytes,install_state,local_path,verified_at FROM models LIMIT 0",
        "SELECT provider_id,revision,kind,endpoint,model_name,credential_ref,request_profile_json,consent_origin,tested_at,updated_at,display_name,test_checkpoint FROM providers LIMIT 0",
        "SELECT credential_ref,provider_id,purpose,created_at FROM provider_credentials_pending LIMIT 0",
        "SELECT entry_id,term,normalized_key,locale,enabled,pinned,created_at,updated_at FROM dictionary_entries LIMIT 0",
        "SELECT session_id,raw_text,polished_text,model_id,model_sha256,dictionary_revision,language,duration_ms,asr_ms,polish_ms,polish_outcome,delivery_status,app_identity,created_at,needs_review,delivery_reason,polish_reason FROM transcriptions LIMIT 0",
        "SELECT session_id,quota_day,created_at FROM usage_events LIMIT 0",
        "SELECT id,kind,created_at,duration_ms,characters,mode,translated,saved FROM activity_events LIMIT 0",
        "SELECT id,signed_token,imported_at FROM license LIMIT 0",
    ] { conn.prepare(sql).map_err(|_| failure("UnknownDatabase"))?; }
    Ok(())
}

pub(super) fn prepare(
    conn: &mut Connection,
    path: Option<&Path>,
    initial_ui_locale: &str,
) -> Result<bool> {
    conn.busy_timeout(Duration::from_secs(3))?;
    let initial = version(conn)?;
    if initial > VERSION {
        return Err(failure("NewerDatabase"));
    }
    if initial < 0 {
        return Err(failure("UnknownDatabase"));
    }
    if initial == 0 {
        let objects: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;
        if objects != 0 {
            return Err(failure("UnknownDatabase"));
        }
        if path.is_some_and(recovery_has_files) {
            return Err(failure("DatabaseRecoveryRequired"));
        }
    }
    // Full checks guard an upgrade and its backup; routine launches only need quick_check.
    if initial < VERSION {
        integrity(conn)?;
    } else {
        check(conn, "PRAGMA quick_check(1)")?;
    }
    conn.set_prepared_statement_cache_capacity(32);
    // Zero freed pages so deleted history does not linger; trim the WAL after resets.
    conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA secure_delete=ON; PRAGMA checkpoint_fullfsync=ON; PRAGMA journal_size_limit=1048576;")?;
    if initial < VERSION {
        // macOS fsync leaves data in the drive cache; the upgrade must reach disk
        // before its backup can be removed.
        conn.execute_batch("PRAGMA fullfsync=ON;")?;
        // Reserve the sole writer before taking the backup, through validation and commit.
        // Backup reads through a separate connection because SQLite cannot back up a writer.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if version(&tx)? != initial {
            return Err(failure("DatabaseMigrationFailed"));
        }
        if let Some(path) = path.filter(|_| initial > 0) {
            cleanup_pending(path).map_err(|_| failure("DatabaseBackupFailed"))?;
            snapshot(path, initial).map_err(|_| failure("DatabaseBackupFailed"))?;
        }
        let migrate = || -> Result<()> {
            if initial == 0 {
                let sql = MIGRATION
                    .lines()
                    .filter(|line| {
                        !line.starts_with("PRAGMA foreign_keys")
                            && !line.starts_with("PRAGMA journal_mode")
                            && !line.starts_with("PRAGMA busy_timeout")
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                tx.execute_batch(&sql)?;
                // This participates in the first schema/settings transaction;
                // a later system-language change never overwrites user choice.
                tx.execute(
                    "UPDATE settings SET ui_locale=?1 WHERE id=1",
                    [initial_ui_locale],
                )?;
            }
            for (i, sql) in MIGRATIONS.iter().enumerate() {
                if initial < i as i64 + 2 {
                    tx.execute_batch(sql)?;
                    if i == 7 {
                        super::activity::backfill(&tx)?;
                    }
                }
            }
            integrity(&tx)?;
            current_valid(&tx)
        };
        migrate().map_err(|_| failure("DatabaseMigrationFailed"))?;
        tx.commit()
            .map_err(|_| failure("DatabaseMigrationFailed"))?;
    }
    current_valid(conn)?;
    // Never change journal mode for an unknown/newer/corrupt database.
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    let flushed = initial == VERSION || path.is_none() || {
        if (1..10).contains(&initial) {
            // Once: earlier versions left deleted text in free pages. Best effort.
            let _ = conn.execute_batch("VACUUM;");
        }
        // Move the upgrade into the main file before any backup becomes eligible for removal.
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
            r.get::<_, i64>(0)
        })
        .is_ok_and(|busy| busy == 0)
    };
    conn.execute_batch("PRAGMA fullfsync=OFF;")?;
    Ok(path.is_some_and(|path| !flushed || cleanup(path).is_err()))
}

fn recovery_has_files(path: &Path) -> bool {
    let directory = recovery_directory(path);
    match fs::symlink_metadata(&directory) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Ok(m) if m.is_dir() => {
            fs::read_dir(directory).map_or(true, |mut entries| entries.next().is_some())
        }
        _ => true,
    }
}

fn private_directory(path: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if !fs::symlink_metadata(path)?.is_dir() {
                return Err(std::io::ErrorKind::InvalidInput.into());
            }
        }
        Err(e) => return Err(e),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn snapshot(path: &Path, initial: i64) -> Result<()> {
    let directory = recovery_directory(path);
    private_directory(&directory).map_err(|_| failure("DatabaseBackupFailed"))?;
    let destination = directory.join(format!("before-v{initial}-to-v{VERSION}.sqlite3"));
    if fs::symlink_metadata(&destination).is_ok() {
        regular(&destination).map_err(|_| failure("DatabaseBackupFailed"))?;
    }
    let temporary = tempfile::Builder::new()
        .prefix(&format!("pending-{}", uuid::Uuid::new_v4()))
        .rand_bytes(0)
        .suffix(".sqlite3")
        .tempfile_in(&directory)
        .map_err(|_| failure("DatabaseBackupFailed"))?;
    let source = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    source.busy_timeout(Duration::from_secs(3))?;
    if version(&source)? != initial {
        return Err(failure("DatabaseBackupFailed"));
    }
    let mut target = Connection::open_with_flags(
        temporary.path(),
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    target.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")?;
    {
        let backup = Backup::new(&source, &mut target)?;
        let start = Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(30) {
                return Err(failure("DatabaseBackupFailed"));
            }
            match backup.step(256)? {
                StepResult::Done => break,
                StepResult::More => (),
                StepResult::Busy | StepResult::Locked => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => return Err(failure("DatabaseBackupFailed")),
            }
        }
    }
    // The backup preserves WAL's journal mode; consolidate it into one portable file.
    target.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;")?;
    integrity(&target)?;
    if version(&target)? != initial {
        return Err(failure("DatabaseBackupFailed"));
    }
    target
        .close()
        .map_err(|_| failure("DatabaseBackupFailed"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| failure("DatabaseBackupFailed"))?;
    temporary
        .persist(&destination)
        .map_err(|_| failure("DatabaseBackupFailed"))?;
    sync_directory(&directory).map_err(|_| failure("DatabaseBackupFailed"))?;
    if let Some(parent) = directory.parent() {
        sync_directory(parent).map_err(|_| failure("DatabaseBackupFailed"))?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(path)?.sync_all()?;
    }
    let _ = path;
    Ok(())
}

fn cleanup(path: &Path) -> std::io::Result<()> {
    cleanup_pending(path)?;
    let directory = recovery_directory(path);
    match fs::symlink_metadata(&directory) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(m) if m.is_dir() => (),
        _ => return Err(std::io::ErrorKind::InvalidInput.into()),
    }
    // Only this database's named migration backups; never recursively clear unknown files.
    // Version 4 introduced these names. Clean known earlier destinations as
    // well after a later healthy upgrade, so old history copies do not linger.
    for destination in 4..=VERSION {
        for initial in 1..destination {
            let file = directory.join(format!("before-v{initial}-to-v{destination}.sqlite3"));
            match regular(&file) {
                Ok(()) => fs::remove_file(file)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
            }
        }
    }
    sync_directory(&directory)
}

fn cleanup_pending(path: &Path) -> std::io::Result<()> {
    let directory = recovery_directory(path);
    match fs::symlink_metadata(&directory) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(m) if m.is_dir() => (),
        _ => return Err(std::io::ErrorKind::InvalidInput.into()),
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let stem = [
            ".sqlite3",
            ".sqlite3-wal",
            ".sqlite3-shm",
            ".sqlite3-journal",
        ]
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix));
        if let Some(id) = stem.and_then(|s| s.strip_prefix("pending-")) {
            if uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id) {
                regular(&entry.path())?;
                fs::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
