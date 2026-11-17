//! Development-only synthetic metadata. Refuses to overwrite an existing DB;
//! never creates, reads or removes a system key. Not invoked by the app.
use speakmore_core::{providers::ProviderDraft, storage::Store};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::path::PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("new output database path required")?,
    );
    if path.exists() {
        return Err("refusing to overwrite existing data".into());
    }
    std::fs::create_dir_all(path.parent().ok_or("invalid output path")?)?;
    let mut store = Store::open(&path)?;
    let mut shortcuts = store.shortcuts()?;
    let before = shortcuts.clone();
    shortcuts.push_to_talk.enabled = false;
    store.update_shortcuts(&before, &shortcuts, 1)?;
    for (name, profile) in [
        ("本机服务 · 合成验收", "{\"kind\":\"ollama\"}"),
        ("协议异常 · 合成验收", "{\"unknown\":\"fixture\"}"),
    ] {
        store.save_provider(
            store.settings()?.revision,
            None,
            ProviderDraft {
                provider_id: uuid::Uuid::new_v4(),
                display_name: name.into(),
                kind: "ollama".into(),
                endpoint: "http://127.0.0.1:11434/api/chat".into(),
                model_name: "synthetic-fixture".into(),
                request_profile_json: profile.into(),
                consent_origin: None,
                credential_ref: None,
            },
            1,
        )?;
    }
    drop(store);
    let conn = rusqlite::Connection::open(&path)?;
    conn.execute("INSERT INTO providers(provider_id,kind,endpoint,model_name,request_profile_json,display_name,credential_ref,updated_at) VALUES('synthetic-broken-id','ollama','http://127.0.0.1:11434/api/chat','fixture','{\"kind\":\"ollama\"}','标识异常 · 合成验收','synthetic-unrecognized-account',1)",[])?;
    conn.execute("INSERT INTO provider_credentials_pending(credential_ref,provider_id,purpose,created_at) VALUES('synthetic-unrecognized-pending-account','synthetic-unrecognized-owner','cleanup',1)",[])?;
    println!("Created isolated fixture: 1 healthy service, 2 invalid services, 1 unrecognized cleanup record; no system keys.");
    Ok(())
}
