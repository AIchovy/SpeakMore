//! Test-only issuer. These public test keys are never trusted by bundled clients.
use super::*;
use ed25519_dalek::{Signer, SigningKey};

pub const KEY_ID: &str = "fixture-license-key";
pub fn trust() -> TrustStore {
    TrustStore::from_json(&serde_json::to_vec(&serde_json::json!({
        "schema":1,"keys":[{"key_id":KEY_ID,"public_key":URL_SAFE_NO_PAD.encode(SigningKey::from_bytes(&[39;32]).verifying_key().as_bytes())}]
    })).unwrap()).unwrap()
}
pub fn token() -> String {
    sign(&payload(), SIGNING_DOMAIN)
}
pub fn payload() -> String {
    format!(
        r#"{{"schema":1,"key_id":"{KEY_ID}","license_id":"627b5d45-b337-4873-b064-1dc7e4550980","product":"speakmore","edition":"pro","major_version":1,"issued_at":1789400000}}"#
    )
}
pub fn sign(payload: &str, domain: &[u8]) -> String {
    let mut bytes = domain.to_vec();
    bytes.extend_from_slice(payload.as_bytes());
    let signature = SigningKey::from_bytes(&[39; 32]).sign(&bytes);
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload),
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    )
}
