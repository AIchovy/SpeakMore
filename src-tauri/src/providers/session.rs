use super::*;
use speakmore_core::{
    processing::{Reason, Watch},
    types::{ResultRecord, Settings},
};
use speakmore_providers::{Mode, NativeEligibility, Outcome, PolishInput};
use std::time::Instant;

/// No runtime Handle: the actor may own this without retaining itself in a cycle.
#[derive(Clone)]
pub(crate) struct SessionAccess {
    pub(super) vault: vault::Vault,
    pub(super) client: speakmore_providers::Client,
    pub(super) state: Arc<Mutex<State>>,
}

pub(crate) struct Resolution {
    pub(crate) text: Option<String>,
    pub(crate) reason: Option<Reason>,
    pub(crate) outcome: &'static str,
    pub(crate) elapsed_ms: u64,
}
impl Resolution {
    pub(crate) fn original(reason: Reason, skipped: bool) -> Self {
        Self {
            text: None,
            reason: Some(reason),
            outcome: if skipped { "skipped" } else { "fallback" },
            elapsed_ms: 0,
        }
    }
    pub(crate) fn apply(self, record: &mut ResultRecord) {
        record.polished_text = self.text;
        record.polish_reason = self.reason;
        record.polish_outcome = self.outcome.into();
        record.polish_ms = Some(self.elapsed_ms);
    }
}

pub(crate) struct Input {
    pub(crate) settings: Settings,
    pub(crate) record: Option<Arc<ProviderRecord>>,
    pub(crate) watch: Arc<Watch>,
    pub(crate) raw: String,
    pub(crate) writing_context: speakmore_core::writing::Context,
    pub(crate) writing_prompt: Option<String>,
}

pub(crate) fn is_cloud(record: Option<&ProviderRecord>) -> bool {
    record
        .and_then(|r| configuration(r).ok())
        .is_none_or(|c| !c.is_local())
}

impl SessionAccess {
    pub(super) async fn restore(
        &self,
        record: &ProviderRecord,
        settings: &Settings,
    ) -> Result<Arc<Tested>> {
        let config = configuration(record)?;
        check_destination(record, &config, settings)?;
        let restored = restored_test(record, &config)?;
        let (cached, generation) = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let cached = state
                .tested
                .get(&record.draft.provider_id)
                .filter(|tested| {
                    tested.revision == record.revision
                        && tested
                            .receipt
                            .checkpoint(&tested.provider, &test_binding(record))
                            .ok()
                            .as_deref()
                            == record.test_checkpoint.as_deref()
                })
                .cloned();
            (cached, state.generation)
        };
        if let Some(cached) = cached {
            return Ok(cached);
        }
        let key = match record.draft.credential_ref.clone() {
            Some(reference) => Some(self.vault.read(reference).await?),
            None => None,
        };
        let provider = Arc::new(PreparedProvider::new(config, key).map_err(provider_error)?);
        let receipt = restored.attach(&provider).map_err(provider_error)?;
        let tested = Arc::new(Tested {
            revision: record.revision,
            provider,
            receipt,
        });
        // Later sessions reuse this without another Keychain read. Lookups above
        // still require the current revision and checkpoint, so a stale entry
        // racing a save or test is never used; never replace a newer revision.
        // A delete or cache clear during the Keychain read keeps it out entirely.
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.generation == generation
            && state
                .tested
                .get(&record.draft.provider_id)
                .is_none_or(|current| current.revision <= record.revision)
        {
            state
                .tested
                .insert(record.draft.provider_id, tested.clone());
        }
        Ok(tested)
    }

    pub(crate) async fn run(&self, input: Input, cancel: CancellationToken) -> Resolution {
        let started = Instant::now();
        let revoked = input.watch.token();
        let attempt = async {
            let skip = |r| Resolution::original(r, true);
            // ASR quality flags remain on the result for the user to review.
            // They do not disable an explicitly enabled editing request.
            if input
                .raw
                .chars()
                .take(speakmore_providers::MAX_INPUT_SCALARS + 1)
                .count()
                > speakmore_providers::MAX_INPUT_SCALARS
            {
                return skip(Reason::TextTooLong);
            }
            let Some(record) = input.record.as_ref() else {
                return skip(Reason::ProviderUnavailable);
            };
            let tested = match self.restore(record, &input.settings).await {
                Ok(tested) => tested,
                Err(_) => return skip(Reason::ProviderUnavailable),
            };
            let default_style = match input.settings.polish_mode.as_str() {
                "email" => {
                    "Format clear email body paragraphs without adding a subject or signature."
                }
                "code_comment" => {
                    "Refine technical explanations without generating executable code."
                }
                "custom" if !input.settings.custom_prompt.trim().is_empty() => {
                    &input.settings.custom_prompt
                }
                _ => "Keep my voice and meaning; tidy spoken repetitions and punctuation.",
            };
            let style = input.writing_prompt.as_deref().unwrap_or(default_style);
            let context = input.writing_context.model_data();
            let mode = Mode::Contextual {
                style,
                context: &context,
            };
            let consent = tested.provider.confirm_destination();
            let policy = if input.settings.processing_policy == "allow_cloud" {
                ProcessingPolicy::AllowCloud(&consent)
            } else {
                ProcessingPolicy::LocalOnly
            };
            // The outer select includes credential preparation in the same budget.
            match self
                .client
                .polish(
                    PolishInput {
                        provider: &tested.provider,
                        receipt: &tested.receipt,
                        raw_text: &input.raw,
                        mode,
                        policy,
                        eligibility: if input.settings.polish_enabled {
                            NativeEligibility::Enabled
                        } else {
                            NativeEligibility::Disabled
                        },
                    },
                    &cancel,
                )
                .await
            {
                Outcome::Polished(text) => Resolution {
                    text: Some(text),
                    reason: None,
                    outcome: "applied",
                    elapsed_ms: 0,
                },
                Outcome::Original(reason) => Resolution::original(failure_reason(reason), false),
            }
        };
        let mut result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Resolution::original(Reason::Cancelled, false),
            _ = revoked.cancelled() => Resolution::original(Reason::PolicyRevoked, false),
            _ = tokio::time::sleep(speakmore_providers::DEADLINE) => Resolution::original(Reason::Deadline, false),
            result = attempt => result,
        };
        if cancel.is_cancelled() {
            result = Resolution::original(Reason::Cancelled, false);
        } else if revoked.is_cancelled() {
            result = Resolution::original(Reason::PolicyRevoked, false);
        }
        result.elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        result
    }
}

fn failure_reason(reason: speakmore_providers::Failure) -> Reason {
    use speakmore_providers::Failure as F;
    match reason {
        F::Cancelled => Reason::Cancelled,
        F::Deadline => Reason::Deadline,
        F::TextTooLong => Reason::TextTooLong,
        F::InvalidPrompt => Reason::InvalidPrompt,
        F::LocalOnly | F::DestinationConsentRequired | F::Disabled => Reason::PolicyRevoked,
        F::NetworkUnavailable | F::HttpStatus { .. } => Reason::NetworkUnavailable,
        F::ProfileNotTested
        | F::Busy
        | F::InvalidCredential
        | F::CredentialRequired
        | F::CredentialNotSupported
        | F::InvalidEndpoint
        | F::InvalidModel => Reason::ProviderUnavailable,
        _ => Reason::ResponseRejected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use speakmore_core::providers::{CredentialRef, ProviderDraft};

    #[tokio::test]
    async fn restored_provider_is_cached_after_one_keychain_read() {
        let memory = Arc::new(super::super::testing::TestVault::default());
        let vault = vault::Vault::for_test(Box::new(memory.clone()));
        let id = Uuid::new_v4();
        let reference = CredentialRef::new(id).unwrap();
        vault
            .write(
                reference.clone(),
                SecretKey::new("synthetic-key".into()).unwrap(),
            )
            .await
            .unwrap();
        let endpoint = "https://api.openai.com/v1/chat/completions";
        let mut record = ProviderRecord {
            draft: ProviderDraft {
                provider_id: id,
                display_name: "Fixture".into(),
                kind: "openai".into(),
                endpoint: endpoint.into(),
                model_name: "fixture-model".into(),
                request_profile_json: serde_json::to_string(&Protocol::Openai).unwrap(),
                consent_origin: Some("https://api.openai.com".into()),
                credential_ref: Some(reference),
            },
            revision: 3,
            tested_at: Some(1),
            test_checkpoint: None,
            updated_at: 1,
        };
        // Mirrors the native checkpoint written after an explicit successful test.
        let protocol = serde_json::to_vec(&Protocol::Openai).unwrap();
        let binding = test_binding(&record);
        let mut hash = Sha256::new();
        hash.update(b"SpeakMore-Provider-Test-v1\0");
        for field in [
            protocol.as_slice(),
            endpoint.as_bytes(),
            b"fixture-model",
            binding.as_bytes(),
        ] {
            hash.update((field.len() as u64).to_be_bytes());
            hash.update(field);
        }
        record.test_checkpoint = Some(format!("v1.{:x}", hash.finalize()));
        let temp = tempfile::tempdir().unwrap();
        let mut settings = speakmore_core::storage::Store::open(&temp.path().join("s.sqlite3"))
            .unwrap()
            .settings()
            .unwrap();
        settings.processing_policy = "allow_cloud".into();
        let access = SessionAccess {
            vault,
            client: speakmore_providers::Client::new().unwrap(),
            state: Arc::default(),
        };
        let first = access.restore(&record, &settings).await.unwrap();
        let second = access.restore(&record, &settings).await.unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(memory.reads(), 1);
        // A restore awaiting the Keychain while the provider is deleted (or the
        // cache cleared) returns its own result but never writes the key back.
        access.state.lock().unwrap().forget(&id);
        let hold = memory.hold_reads();
        let pending = tokio::spawn({
            let (access, record, settings) = (access.clone(), record.clone(), settings.clone());
            async move { access.restore(&record, &settings).await.map(|_| ()) }
        });
        while memory.reads() < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        access.state.lock().unwrap().forget(&id);
        drop(hold);
        pending.await.unwrap().unwrap();
        assert!(access.state.lock().unwrap().tested.is_empty());
        // A changed binding never reuses the cached credential.
        record.revision = 4;
        assert!(access.restore(&record, &settings).await.is_err());
        assert_eq!(memory.reads(), 2);
    }
}
