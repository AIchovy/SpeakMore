use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use speakmore_models::catalog::{validate, Envelope, Payload, TrustKey};
use std::{fs, io::Write, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 5 {
        return Err("usage: speakmore-model-catalog-sign <payload.json> <private-key> <envelope.json> <trust.json>".into());
    }
    let raw = fs::read(&args[1])?;
    if raw.len() > 512 * 1024 {
        return Err("payload too large".into());
    }
    validate(&serde_json::from_slice::<Payload>(&raw)?)?;
    let key_path = Path::new(&args[2]);
    let parent = key_path.parent().ok_or("key directory missing")?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let seed: [u8; 32] = if key_path.try_exists()? {
        if !fs::symlink_metadata(key_path)?.file_type().is_file() {
            return Err("key must be an ordinary file".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(key_path)?.permissions().mode() & 0o077 != 0 {
                return Err("key permissions must be 0600".into());
            }
        }
        fs::read(key_path)?
            .try_into()
            .map_err(|_| "invalid key length")?
    } else {
        let mut seed = [0; 32];
        getrandom::fill(&mut seed).map_err(|_| "secure random generation failed")?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(key_path)?;
        file.write_all(&seed)?;
        file.sync_all()?;
        seed
    };
    let key = SigningKey::from_bytes(&seed);
    let trust = TrustKey {
        key_id: "speakmore-models-2026-01".into(),
        public_key: STANDARD.encode(key.verifying_key().to_bytes()),
    };
    // Refuse accidental root replacement: a new root requires an explicit maintained rotation.
    if Path::new(&args[4]).try_exists()? {
        let old: TrustKey = serde_json::from_slice(&fs::read(&args[4])?)?;
        if old.key_id != trust.key_id || old.public_key != trust.public_key {
            return Err("existing model trust root differs".into());
        }
    }
    let envelope = Envelope {
        key_id: trust.key_id.clone(),
        signature: STANDARD.encode(key.sign(&raw).to_bytes()),
        payload: STANDARD.encode(raw),
    };
    fs::write(&args[3], serde_json::to_vec_pretty(&envelope)?)?;
    fs::write(&args[4], serde_json::to_vec_pretty(&trust)?)?;
    println!("Signed model catalog; private key was not printed.");
    Ok(())
}
