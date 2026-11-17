use crate::{Failure, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use url::{Host, Url};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetField {
    MaxCompletionTokens,
    MaxTokens,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstructionRole {
    System,
    Developer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Protocol {
    Openai,
    Anthropic,
    Ollama,
    OpenaiCompatible {
        budget_field: BudgetField,
        instruction_role: InstructionRole,
    },
}

impl Protocol {
    pub fn default_endpoint(self) -> Option<&'static str> {
        match self {
            Self::Openai => Some("https://api.openai.com/v1/chat/completions"),
            Self::Anthropic => Some("https://api.anthropic.com/v1/messages"),
            Self::Ollama => Some("http://127.0.0.1:11434/api/chat"),
            Self::OpenaiCompatible { .. } => None,
        }
    }
}

// Input deliberately has no credential, consent, tested or entitlement field.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationInput {
    pub protocol: Protocol,
    pub endpoint: String,
    pub model: String,
}

#[derive(Clone)]
pub struct Configuration {
    pub(crate) protocol: Protocol,
    pub(crate) endpoint: Url,
    pub(crate) model: String,
    pub(crate) local: bool,
    // Opaque identity prevents a test/consent from surviving configuration changes.
    pub(crate) identity: Uuid,
}

impl Configuration {
    pub fn validate(input: ConfigurationInput) -> Result<Self> {
        let raw = &input.endpoint;
        if raw.len() > 2048
            || raw.chars().any(|c| c.is_whitespace() || c.is_control())
            || raw.contains('\\')
        {
            return Err(Failure::InvalidEndpoint);
        }
        let endpoint = Url::parse(raw).map_err(|_| Failure::InvalidEndpoint)?;
        // Reject even empty userinfo/query/fragment. Credentials belong only in headers.
        let authority = raw
            .split_once("://")
            .map(|(_, tail)| tail.split(['/', '?', '#']).next().unwrap_or_default())
            .ok_or(Failure::InvalidEndpoint)?;
        if authority.contains('@')
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || endpoint.host().is_none()
            || endpoint.port() == Some(0)
            || endpoint.path().is_empty()
            || endpoint.path() == "/"
        {
            return Err(Failure::InvalidEndpoint);
        }
        // URL parsers canonicalize decimal/hex/abbreviated IP addresses. They must
        // never become a local-only exemption: require the user's literal spelling.
        let literal_loopback = authority == "127.0.0.1"
            || authority.strip_prefix("127.0.0.1:").is_some_and(valid_port)
            || authority == "[::1]"
            || authority.strip_prefix("[::1]:").is_some_and(valid_port);
        let loopback_host = match endpoint.host() {
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        let local = input.protocol == Protocol::Ollama && literal_loopback && loopback_host;
        if endpoint.scheme() != "https" && !(local && endpoint.scheme() == "http") {
            return Err(Failure::InvalidEndpoint);
        }
        if input.model.is_empty()
            || input.model.len() > 256
            || input.model.trim() != input.model
            || input.model.chars().any(char::is_control)
        {
            return Err(Failure::InvalidModel);
        }
        Ok(Self {
            protocol: input.protocol,
            endpoint,
            model: input.model,
            local,
            identity: Uuid::new_v4(),
        })
    }

    pub fn is_local(&self) -> bool {
        self.local
    }

    /// Exact normalized destination for an explicit native settings consent view.
    /// Never include this value in diagnostic logs.
    pub fn endpoint(&self) -> &str {
        self.endpoint.as_str()
    }

    /// Called only after the native controller verifies a user's destination
    /// confirmation. This capability is not deserializable from WebView IPC.
    pub fn confirm_destination(&self) -> DestinationConsent {
        DestinationConsent {
            configuration: self.identity,
        }
    }
}

fn valid_port(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u16>().is_ok_and(|p| p != 0)
}

impl fmt::Debug for Configuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Configuration")
            .field("protocol", &self.protocol)
            .field("local", &self.local)
            .finish_non_exhaustive()
    }
}

pub struct SecretKey(Zeroizing<String>);

impl SecretKey {
    pub fn new(value: String) -> Result<Self> {
        let value = Zeroizing::new(value);
        if value.is_empty() || value.len() > 8192 || !value.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(Failure::InvalidCredential);
        }
        Ok(Self(value))
    }

    /// Native credential-store/HTTP integration only. Never return this value in
    /// IPC, diagnostics or a serializable provider configuration.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretKey([REDACTED])")
    }
}

/// Immutable configuration and its credential are moved together; no setter can
/// change a destination while retaining the old key or connection-test receipt.
pub struct PreparedProvider {
    pub(crate) config: Configuration,
    pub(crate) key: Option<SecretKey>,
    pub(crate) identity: Uuid,
}

impl PreparedProvider {
    /// Native caller must validate its current policy and recorded destination consent.
    pub fn confirm_destination(&self) -> DestinationConsent {
        self.config.confirm_destination()
    }
    pub fn new(config: Configuration, key: Option<SecretKey>) -> Result<Self> {
        match config.protocol {
            Protocol::Openai | Protocol::Anthropic if key.is_none() => {
                return Err(Failure::CredentialRequired);
            }
            Protocol::Ollama if config.local && key.is_some() => {
                return Err(Failure::CredentialNotSupported);
            }
            _ => {}
        }
        Ok(Self {
            config,
            key,
            identity: Uuid::new_v4(),
        })
    }
}

impl fmt::Debug for PreparedProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedProvider")
            .field("config", &self.config)
            .field("credential_configured", &self.key.is_some())
            .finish_non_exhaustive()
    }
}

pub struct DestinationConsent {
    configuration: Uuid,
}

pub enum ProcessingPolicy<'a> {
    LocalOnly,
    AllowCloud(&'a DestinationConsent),
}

impl ProcessingPolicy<'_> {
    pub(crate) fn check(&self, config: &Configuration) -> Result<()> {
        if config.local {
            return Ok(());
        }
        match self {
            Self::LocalOnly => Err(Failure::LocalOnly),
            Self::AllowCloud(consent) if consent.configuration == config.identity => Ok(()),
            _ => Err(Failure::DestinationConsentRequired),
        }
    }
}
