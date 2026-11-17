//! Offline authorization. No network, clock, device identity, storage or IPC.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;
use zeroize::Zeroizing;

pub const MAX_TOKEN_BYTES: usize = 8192;
pub const SUPPORTED_MAJOR_VERSION: u32 = 1;
pub const SIGNING_DOMAIN: &[u8] = b"SpeakMore-License-v1\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    #[error("License token is malformed")]
    InvalidToken,
    #[error("License signing key is unknown")]
    UnknownKey,
    #[error("License signature is invalid")]
    InvalidSignature,
    #[error("License schema is unsupported")]
    UnsupportedSchema,
    #[error("License belongs to another product")]
    WrongProduct,
    #[error("License edition is unsupported")]
    WrongEdition,
    #[error("License is for another major version")]
    UnsupportedMajor,
    #[error("License claims are invalid")]
    InvalidClaims,
    #[error("License trust configuration is invalid")]
    InvalidTrust,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyFile {
    schema: u32,
    keys: Vec<KeySpec>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeySpec {
    key_id: String,
    public_key: String,
}

#[derive(Clone)]
pub struct TrustStore {
    keys: Vec<(String, VerifyingKey)>,
}
impl TrustStore {
    pub fn from_json(bytes: &[u8]) -> Result<Self, Failure> {
        if bytes.len() > 16_384 {
            return Err(Failure::InvalidTrust);
        }
        let file: KeyFile = serde_json::from_slice(bytes).map_err(|_| Failure::InvalidTrust)?;
        if file.schema != 1 || file.keys.is_empty() || file.keys.len() > 16 {
            return Err(Failure::InvalidTrust);
        }
        let mut keys = Vec::new();
        for spec in file.keys {
            if !valid_key_id(&spec.key_id) || keys.iter().any(|(id, _)| *id == spec.key_id) {
                return Err(Failure::InvalidTrust);
            }
            let encoded: [u8; 32] = URL_SAFE_NO_PAD
                .decode(&spec.public_key)
                .map_err(|_| Failure::InvalidTrust)?
                .try_into()
                .map_err(|_| Failure::InvalidTrust)?;
            let key = VerifyingKey::from_bytes(&encoded).map_err(|_| Failure::InvalidTrust)?;
            if key.is_weak() {
                return Err(Failure::InvalidTrust);
            }
            keys.push((spec.key_id, key));
        }
        Ok(Self { keys })
    }

    pub fn bundled() -> Result<Self, Failure> {
        Self::from_json(include_bytes!("../../../resources/license-trust.json"))
    }

    pub fn verify(&self, token: &str) -> Result<VerifiedLicense, Failure> {
        // Check the raw submitted length before accepting surrounding copy/paste whitespace.
        if token.len() > MAX_TOKEN_BYTES {
            return Err(Failure::InvalidToken);
        }
        let token = token.trim();
        let (payload, signature) = token.split_once('.').ok_or(Failure::InvalidToken)?;
        let payload = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(payload)
                .map_err(|_| Failure::InvalidToken)?,
        );
        let signature: [u8; 64] = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| Failure::InvalidToken)?
            .try_into()
            .map_err(|_| Failure::InvalidToken)?;
        // Serde's struct visitor rejects duplicates (including escaped duplicate names),
        // unknown fields, wrong primitive types, trailing data and excessive nesting.
        let claims: Claims = serde_json::from_slice(&payload).map_err(|_| Failure::InvalidToken)?;
        let key = self
            .keys
            .iter()
            .find(|(id, _)| *id == claims.key_id)
            .ok_or(Failure::UnknownKey)?;
        let mut signed = Zeroizing::new(Vec::with_capacity(SIGNING_DOMAIN.len() + payload.len()));
        signed.extend_from_slice(SIGNING_DOMAIN);
        signed.extend_from_slice(&payload);
        key.1
            .verify_strict(&signed, &Signature::from_bytes(&signature))
            .map_err(|_| Failure::InvalidSignature)?;
        claims.validate()?;
        Ok(VerifiedLicense {
            claims,
            token: Zeroizing::new(token.to_owned()),
        })
    }
}

fn valid_key_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Claims {
    schema: u32,
    key_id: String,
    license_id: String,
    product: String,
    edition: String,
    major_version: u32,
    issued_at: i64,
}
impl Claims {
    fn validate(&self) -> Result<(), Failure> {
        if self.schema != 1 {
            return Err(Failure::UnsupportedSchema);
        }
        if self.product != "speakmore" {
            return Err(Failure::WrongProduct);
        }
        if self.edition != "pro" {
            return Err(Failure::WrongEdition);
        }
        if self.major_version != SUPPORTED_MAJOR_VERSION {
            return Err(Failure::UnsupportedMajor);
        }
        let id = Uuid::parse_str(&self.license_id).map_err(|_| Failure::InvalidClaims)?;
        if id.is_nil()
            || id.to_string() != self.license_id
            || !valid_key_id(&self.key_id)
            || self.issued_at < 0
        {
            return Err(Failure::InvalidClaims);
        }
        Ok(())
    }
}

/// Possession proves this token passed this process's trusted-key verification.
/// Cannot be constructed from an IPC boolean or deserialized status object.
pub struct VerifiedLicense {
    claims: Claims,
    token: Zeroizing<String>,
}
impl VerifiedLicense {
    pub fn license_id(&self) -> &str {
        &self.claims.license_id
    }
    pub fn key_id(&self) -> &str {
        &self.claims.key_id
    }
    pub fn issued_at(&self) -> i64 {
        self.claims.issued_at
    }
    pub fn major_version(&self) -> u32 {
        self.claims.major_version
    }
    /// Native persistence only. Never include the signed token in IPC or diagnostics.
    pub fn encoded_token(&self) -> &str {
        &self.token
    }
}
impl fmt::Debug for VerifiedLicense {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("VerifiedLicense([REDACTED])")
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
pub mod fixtures;
#[cfg(test)]
mod tests;
