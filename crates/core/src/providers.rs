//! Provider metadata and identifiers. This module never contains API key bytes.
use crate::error::{AppError, Result};
use uuid::Uuid;

pub const MAX_PROVIDERS: usize = 20;
pub const MAX_PENDING_CREDENTIALS: usize = 64;

#[derive(Clone)]
pub struct ProviderDraft {
    pub provider_id: Uuid,
    pub display_name: String,
    pub kind: String,
    pub endpoint: String,
    pub model_name: String,
    pub request_profile_json: String,
    pub consent_origin: Option<String>,
    pub credential_ref: Option<CredentialRef>,
}

#[derive(Clone)]
pub struct ProviderRecord {
    pub draft: ProviderDraft,
    pub revision: i64,
    pub tested_at: Option<i64>,
    pub test_checkpoint: Option<String>,
    pub updated_at: i64,
}

impl ProviderDraft {
    /// Structural storage validation; the native protocol adapter performs the
    /// full configuration and authentication checks before using a provider.
    pub fn validate(&self) -> Result<()> {
        let invalid = || AppError::invalid("AI 服务配置无效。");
        if self.provider_id.is_nil()
            || self.display_name.trim().is_empty()
            || self.display_name.len() > 160
            || self.model_name.trim().is_empty()
            || self.model_name.len() > 256
            || self.endpoint.len() > 2048
            || self.request_profile_json.len() > 4096
            || !["openai", "anthropic", "ollama", "openai_compatible"].contains(&self.kind.as_str())
            || [&self.display_name, &self.model_name, &self.endpoint]
                .iter()
                .any(|s| s.trim() != s.as_str() || s.chars().any(char::is_control))
            || self
                .credential_ref
                .as_ref()
                .is_some_and(|reference| reference.provider_id() != self.provider_id)
        {
            return Err(invalid());
        }
        let endpoint = url::Url::parse(&self.endpoint).map_err(|_| invalid())?;
        let local =
            self.kind == "ollama" && matches!(endpoint.host_str(), Some("127.0.0.1" | "[::1]"));
        if endpoint.as_str() != self.endpoint
            || endpoint.host().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.path() == "/"
            || endpoint.port() == Some(0)
            || (endpoint.scheme() != "https" && !(local && endpoint.scheme() == "http"))
            || self
                .consent_origin
                .as_ref()
                .is_some_and(|origin| origin != &endpoint.origin().ascii_serialization())
            || !serde_json::from_str::<serde_json::Value>(&self.request_profile_json)
                .is_ok_and(|v| v.is_object())
        {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Only exact SpeakMore-owned account identifiers may reach the system vault.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CredentialRef {
    provider: Uuid,
    revision: Uuid,
}

impl CredentialRef {
    pub fn new(provider: Uuid) -> Result<Self> {
        if provider.is_nil() {
            return Err(AppError::invalid("AI 服务标识无效。"));
        }
        Ok(Self {
            provider,
            revision: Uuid::new_v4(),
        })
    }
    pub fn parse(account: &str) -> Result<Self> {
        let invalid = || AppError::invalid("AI 服务凭据引用无效。");
        if account.len() != 75 {
            return Err(invalid());
        }
        let (provider, revision) = account.split_once(" / ").ok_or_else(invalid)?;
        let value = Self {
            provider: Uuid::parse_str(provider).map_err(|_| invalid())?,
            revision: Uuid::parse_str(revision).map_err(|_| invalid())?,
        };
        if value.provider.is_nil() || value.revision.is_nil() || value.account() != account {
            return Err(invalid());
        }
        Ok(value)
    }
    pub fn provider_id(&self) -> Uuid {
        self.provider
    }
    pub fn account(&self) -> String {
        format!("{} / {}", self.provider, self.revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn account_reference_is_owned_canonical_and_nonempty() {
        let reference = CredentialRef::new(Uuid::new_v4()).unwrap();
        assert_eq!(
            CredentialRef::parse(&reference.account()).unwrap(),
            reference
        );
        for invalid in [
            "",
            "some-other-account",
            " / ",
            &reference.account().to_uppercase(),
            &format!("{} / {}", Uuid::nil(), Uuid::new_v4()),
        ] {
            assert!(CredentialRef::parse(invalid).is_err());
        }
    }
}
