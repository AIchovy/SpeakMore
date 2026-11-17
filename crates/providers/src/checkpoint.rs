//! Native database checkpoints, not signed licences or remote-service guarantees.
//! Only save after an explicit successful test. Never accept a checkpoint from IPC.
use crate::{Configuration, Failure, PreparedProvider, Result, TestReceipt};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Revalidated metadata, bound to this process's immutable configuration.
/// Restoring does not read a key, contact a provider, or grant destination consent.
pub struct RestoredTest {
    configuration: Uuid,
}

impl TestReceipt {
    /// `binding` identifies the native provider ID, configuration revision and
    /// immutable credential reference; it must contain no secret key bytes.
    pub fn checkpoint(&self, provider: &PreparedProvider, binding: &str) -> Result<String> {
        if self.provider != provider.identity {
            return Err(Failure::ProfileNotTested);
        }
        fingerprint(&provider.config, binding)
    }
}

impl RestoredTest {
    /// The native controller reads this value only from its own database, whose
    /// test marker is written after a successful explicit protocol test. The hash
    /// detects stale metadata; it is not authentication against database tampering.
    pub fn verify(config: &Configuration, binding: &str, saved: &str) -> Result<Self> {
        if saved.len() != 67 || saved != fingerprint(config, binding)? {
            return Err(Failure::ProfileNotTested);
        }
        Ok(Self {
            configuration: config.identity,
        })
    }

    /// Call only after the native vault reads the immutable reference in `binding`.
    pub fn attach(self, provider: &PreparedProvider) -> Result<TestReceipt> {
        if self.configuration != provider.config.identity {
            return Err(Failure::ProfileNotTested);
        }
        Ok(TestReceipt {
            provider: provider.identity,
        })
    }
}

fn fingerprint(config: &Configuration, binding: &str) -> Result<String> {
    if binding.is_empty() || binding.len() > 256 || binding.chars().any(char::is_control) {
        return Err(Failure::ProfileNotTested);
    }
    let protocol = serde_json::to_vec(&config.protocol).map_err(|_| Failure::ProfileNotTested)?;
    let mut hash = Sha256::new();
    hash.update(b"SpeakMore-Provider-Test-v1\0");
    for field in [
        protocol.as_slice(),
        config.endpoint.as_str().as_bytes(),
        config.model.as_bytes(),
        binding.as_bytes(),
    ] {
        hash.update((field.len() as u64).to_be_bytes());
        hash.update(field);
    }
    Ok(format!("v1.{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConfigurationInput, Protocol};
    fn configuration(model: &str, endpoint: &str) -> Configuration {
        Configuration::validate(ConfigurationInput {
            protocol: Protocol::Ollama,
            model: model.into(),
            endpoint: endpoint.into(),
        })
        .unwrap()
    }
    #[test]
    fn checkpoint_survives_new_runtime_identity_but_not_configuration_or_credential_changes() {
        let endpoint = "http://127.0.0.1:11434/api/chat";
        let config = configuration("fixture", endpoint);
        let provider = PreparedProvider::new(config.clone(), None).unwrap();
        let receipt = TestReceipt::fixture(&provider);
        let binding = "native-provider-id:revision-3:immutable-key-reference";
        let saved = receipt.checkpoint(&provider, binding).unwrap();
        assert_eq!(saved.len(), 67);
        assert!(!saved.contains("fixture"));
        let restarted_config = configuration("fixture", endpoint);
        let restored = RestoredTest::verify(&restarted_config, binding, &saved).unwrap();
        let restarted = PreparedProvider::new(restarted_config, None).unwrap();
        assert!(restored.attach(&restarted).is_ok());
        for invalid in [
            "",
            "v1.",
            &format!("{saved}\n"),
            &saved.to_uppercase(),
            &"x".repeat(8192),
        ] {
            assert!(RestoredTest::verify(&config, binding, invalid).is_err());
        }
        assert!(
            RestoredTest::verify(&config, "different-provider-or-revision-or-key", &saved).is_err()
        );
        assert!(
            RestoredTest::verify(&configuration("other-model", endpoint), binding, &saved).is_err()
        );
        assert!(RestoredTest::verify(
            &configuration("fixture", "http://127.0.0.1:11435/api/chat"),
            binding,
            &saved
        )
        .is_err());
        assert!(RestoredTest::verify(
            &configuration("fixture", "http://127.0.0.1:11434/other"),
            binding,
            &saved
        )
        .is_err());
        let restored = RestoredTest::verify(&config, binding, &saved).unwrap();
        assert!(restored.attach(&restarted).is_err());
        assert!(receipt.checkpoint(&restarted, binding).is_err());
    }
    #[test]
    fn protocol_profile_and_native_binding_are_part_of_the_checkpoint() {
        let input = |protocol| ConfigurationInput {
            protocol,
            model: "fixture".into(),
            endpoint: "https://example.invalid/v1/chat/completions".into(),
        };
        let first = Configuration::validate(input(Protocol::OpenaiCompatible {
            budget_field: crate::BudgetField::MaxTokens,
            instruction_role: crate::InstructionRole::System,
        }))
        .unwrap();
        let other = Configuration::validate(input(Protocol::OpenaiCompatible {
            budget_field: crate::BudgetField::MaxCompletionTokens,
            instruction_role: crate::InstructionRole::Developer,
        }))
        .unwrap();
        let provider = PreparedProvider::new(first.clone(), None).unwrap();
        let saved = TestReceipt::fixture(&provider)
            .checkpoint(&provider, "provider:1:no-key")
            .unwrap();
        assert!(RestoredTest::verify(&other, "provider:1:no-key", &saved).is_err());
        for binding in ["", "control\n", &"a".repeat(257)] {
            assert!(RestoredTest::verify(&first, binding, &saved).is_err());
        }
    }
}
