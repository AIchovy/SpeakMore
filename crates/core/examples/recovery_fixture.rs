//! Create a synthetic v1 database for a separate UI verification app identity.
//! Never invoked by the product or included in application bundles.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use rusqlite::{params, Connection};
    use std::{fs, path::PathBuf};
    let path = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("output database path required")?,
    );
    if !path.is_absolute() || path.exists() {
        return Err("requires a new absolute database path".into());
    }
    let parent = path.parent().ok_or("invalid path")?;
    fs::create_dir_all(parent)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    drop(options.open(&path)?);
    let conn = Connection::open(&path)?;
    conn.execute_batch(include_str!("../../../migrations/001_initial.sql"))?;
    conn.execute_batch("CREATE TABLE app_rules_v4(fixture TEXT); INSERT INTO app_rules_v4 VALUES('synthetic migration conflict'); UPDATE settings SET history_enabled=1,custom_prompt='合成恢复验证，不是真实用户数据';")?;
    conn.execute("INSERT INTO transcriptions(session_id,raw_text,model_id,model_sha256,dictionary_revision,language,duration_ms,asr_ms,polish_outcome,delivery_status,created_at) VALUES(?1,?2,'fixture',?3,1,'zh',1000,100,'off','ready_for_copy',1700000000000)", params![uuid::Uuid::new_v4().to_string(), "这是一条用于验证升级恢复的合成记录。", "a".repeat(64)])?;
    println!("Created one synthetic v1 record and an intentional migration 004 conflict; no user data or microphone used.");
    Ok(())
}
