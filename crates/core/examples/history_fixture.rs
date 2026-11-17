//! Development-only history fixture for an isolated application identity.
//! Never invoked by the desktop application, and never bundled for users.
use speakmore_core::{
    storage::Store,
    types::{ResultRecord, SettingsPatch},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("output database path required")?,
    );
    if path.exists() {
        return Err("refusing to overwrite an existing database".into());
    }
    std::fs::create_dir_all(path.parent().ok_or("invalid path")?)?;
    let mut store = Store::open(&path)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;
    store.update_settings(
        1,
        SettingsPatch {
            history_enabled: Some(true),
            ..Default::default()
        },
        now,
    )?;
    for i in 0..75 {
        let raw = match i {
            74 => "=1+1,\"引号\"\n这是导出验证用的合成记录，不是真实听写。".into(),
            73 => {
                "<script>alert('fixture')</script>\n普通文字中的链接 https://example.com 不应执行。"
                    .into()
            }
            72 => "长段验证，保留自然换行和完整内容。\n".repeat(180),
            _ => format!("测试记录 {i:02} · literal_% · 仅用于验证搜索、分页和导出。"),
        };
        let r = ResultRecord {
            session_id: uuid::Uuid::new_v4(),
            raw_text: raw,
            polished_text: (i == 71).then(|| "整理后的合成示例，仍需用户检查。".into()),
            model_id: "ui-test-fixture".into(),
            model_sha256: "a".repeat(64),
            dictionary_revision: 1,
            language: "zh".into(),
            duration_ms: 300_000,
            asr_ms: 1,
            needs_review: true,
            polish_ms: None,
            polish_reason: None,
            polish_outcome: if i == 71 { "applied" } else { "off" }.into(),
            delivery_status: "ready_for_copy".into(),
            delivery_reason: None,
            app_identity: None,
            created_at: now - (75 - i) * 1000,
        };
        store.persist_generated(&r, "2026-09-15", true, false)?;
    }
    println!("Created 75 synthetic history records; no microphone or ASR was used.");
    Ok(())
}
