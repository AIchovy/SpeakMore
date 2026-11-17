pub(crate) mod session;
#[cfg(test)]
mod testing;
mod vault;

use crate::session_runtime::Handle;
use serde::{Deserialize, Serialize};
use speakmore_core::{
    error::{AppError, Result},
    providers::{ProviderDraft, ProviderRecord},
    storage::{RecoveryKind, RecoveryTarget},
};
use speakmore_providers::{
    Configuration, ConfigurationInput, PreparedProvider, ProcessingPolicy, Protocol, SecretKey,
    TestReceipt,
};
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::{oneshot, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone, Copy)]
pub(crate) enum TextTask {
    Meeting,
    Caption,
    Assistant,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum CredentialChange {
    Keep,
    Replace { key: String },
    Remove,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveInput {
    expected_revision: i64,
    provider_id: Option<Uuid>,
    provider_revision: Option<i64>,
    display_name: String,
    configuration: ConfigurationInput,
    credential: CredentialChange,
    confirm_destination: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Row {
    provider_id: Uuid,
    revision: i64,
    display_name: String,
    protocol: Protocol,
    endpoint: String,
    model: String,
    local: bool,
    credential_configured: bool,
    destination_confirmed: bool,
    tested_at: Option<i64>,
    test_valid: bool,
    prepared_this_run: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct View {
    revision: i64,
    active_provider_id: Option<String>,
    rows: Vec<Row>,
    recovery_rows: Vec<RecoveryRow>,
    total_providers: u64,
    truncated: bool,
    cleanup_pending: usize,
    busy: bool,
    operation: Option<&'static str>,
    cleanup_error: Option<AppError>,
}

struct Tested {
    revision: i64,
    provider: Arc<PreparedProvider>,
    receipt: TestReceipt,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecoveryRow {
    recovery_id: Uuid,
    display_name: Option<String>,
    kind: &'static str,
}
struct Review {
    target: RecoveryTarget,
    observed: std::time::Instant,
}

#[derive(Default)]
struct State {
    tested: HashMap<Uuid, Arc<Tested>>,
    /// Bumped whenever cached entries are dropped, so a restore that was
    /// awaiting the Keychain never writes a decrypted key back afterwards.
    generation: u64,
    operation: Option<&'static str>,
    cancel: Option<CancellationToken>,
    cleanup_error: Option<AppError>,
    recovery: HashMap<Uuid, Review>,
}
impl State {
    fn forget(&mut self, id: &Uuid) {
        self.generation += 1;
        self.tested.remove(id);
    }
    fn forget_all(&mut self) {
        self.generation += 1;
        self.tested.clear();
    }
}

struct OperationCompletion {
    service: Service,
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
}
impl Drop for OperationCompletion {
    fn drop(&mut self) {
        {
            let mut state = self.service.state();
            state.operation = None;
            state.cancel = None;
        }
        self.permit.take();
        (self.service.notify)();
    }
}

#[derive(Clone)]
pub(crate) struct Service {
    runtime: Handle,
    vault: vault::Vault,
    client: speakmore_providers::Client,
    // Each long-running workload has its own one-request slot so a meeting summary
    // or caption queue never turns a dictation polish into a silent fallback.
    meeting_client: speakmore_providers::Client,
    caption_client: speakmore_providers::Client,
    assistant_client: speakmore_providers::Client,
    slot: Arc<Semaphore>,
    state: Arc<Mutex<State>>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl Service {
    pub(crate) fn new(runtime: Handle, notify: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        let service = Self {
            runtime,
            vault: vault::Vault::native()?,
            client: speakmore_providers::Client::new().map_err(provider_error)?,
            meeting_client: speakmore_providers::Client::new().map_err(provider_error)?,
            caption_client: speakmore_providers::Client::new().map_err(provider_error)?,
            assistant_client: speakmore_providers::Client::new().map_err(provider_error)?,
            slot: Arc::new(Semaphore::new(1)),
            state: Arc::new(Mutex::new(State::default())),
            notify,
        };
        service.runtime.install_polisher(service.session_access())?;
        Ok(service)
    }

    pub(crate) fn session_access(&self) -> session::SessionAccess {
        session::SessionAccess {
            vault: self.vault.clone(),
            client: self.client.clone(),
            state: self.state.clone(),
        }
    }

    pub(crate) async fn enable_polishing(
        &self,
        expected: i64,
    ) -> Result<speakmore_core::types::Settings> {
        self.operation("enabling", move |service, _| async move {
            let (settings, records) = service
                .runtime
                .query(move |store| Ok((store.settings()?, store.providers()?)))
                .await?;
            if settings.revision != expected {
                return Err(AppError::conflict());
            }
            let record = records
                .into_iter()
                .find(|record| {
                    Some(record.draft.provider_id.to_string()) == settings.active_provider_id
                })
                .ok_or_else(|| provider_error(speakmore_providers::Failure::ProfileNotTested))?;
            let tested = service.restore(&record, &settings).await?;
            let settings = service
                .runtime
                .database(move |store| {
                    store.update_settings(
                        expected,
                        speakmore_core::types::SettingsPatch {
                            polish_enabled: Some(true),
                            ..Default::default()
                        },
                        crate::now(),
                    )
                })
                .await?;
            service
                .state()
                .tested
                .insert(record.draft.provider_id, tested);
            Ok(settings)
        })
        .await
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) async fn view(&self) -> Result<View> {
        let (settings, inventory) = self
            .runtime
            .query(|store| Ok((store.settings()?, store.provider_inventory()?)))
            .await?;
        let mut rows = Vec::new();
        let mut recovery = Vec::new();
        for entry in inventory.entries {
            let Some(record) = entry.record else {
                recovery.push((entry.target, entry.name));
                continue;
            };
            let Ok(config) = configuration(&record) else {
                recovery.push((entry.target, entry.name));
                continue;
            };
            let origin = origin(config.endpoint())?;
            let test_valid = restored_test(&record, &config).is_ok();
            let prepared_this_run = test_valid
                && self
                    .state()
                    .tested
                    .get(&record.draft.provider_id)
                    .is_some_and(|tested| {
                        tested.revision == record.revision
                            && tested
                                .receipt
                                .checkpoint(&tested.provider, &test_binding(&record))
                                .ok()
                                .as_deref()
                                == record.test_checkpoint.as_deref()
                    });
            rows.push(Row {
                provider_id: record.draft.provider_id,
                revision: record.revision,
                protocol: protocol(&record)?,
                endpoint: record.draft.endpoint.clone(),
                model: record.draft.model_name.clone(),
                display_name: record.draft.display_name,
                local: config.is_local(),
                credential_configured: record.draft.credential_ref.is_some(),
                destination_confirmed: config.is_local()
                    || record.draft.consent_origin.as_deref() == Some(origin.as_str()),
                tested_at: record.tested_at,
                test_valid,
                prepared_this_run,
            });
        }
        rows.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        recovery.extend(
            inventory
                .invalid_credentials
                .into_iter()
                .map(|target| (target, None)),
        );
        let mut state = self.state();
        let mut reviews = HashMap::new();
        let mut recovery_rows = Vec::new();
        for (target, display_name) in recovery {
            let recovery_id = state
                .recovery
                .iter()
                .find(|(_, review)| review.target == target)
                .map(|(id, _)| *id)
                .unwrap_or_else(Uuid::new_v4);
            let kind = match target.kind() {
                RecoveryKind::Provider => "provider",
                RecoveryKind::Credential => "credential",
            };
            reviews.insert(
                recovery_id,
                Review {
                    target,
                    observed: std::time::Instant::now(),
                },
            );
            recovery_rows.push(RecoveryRow {
                recovery_id,
                display_name,
                kind,
            });
        }
        state.recovery = reviews;
        Ok(View {
            revision: settings.revision,
            active_provider_id: settings.active_provider_id,
            rows,
            recovery_rows,
            total_providers: inventory.total_providers,
            truncated: inventory.truncated,
            cleanup_pending: inventory.cleanup_pending,
            busy: self.slot.available_permits() == 0 || self.vault.busy(),
            operation: state.operation,
            cleanup_error: state.cleanup_error.clone(),
        })
    }

    // Mutations are owned by this service after admission. Closing a WebView or
    // dropping an IPC waiter cannot abandon a half-finished Keychain transaction.
    async fn operation<T: Send + 'static, F: Future<Output = Result<T>> + Send + 'static>(
        &self,
        name: &'static str,
        run: impl FnOnce(Service, CancellationToken) -> F + Send + 'static,
    ) -> Result<T> {
        let permit = self
            .slot
            .clone()
            .try_acquire_owned()
            .map_err(|_| AppError::busy())?;
        if self.vault.busy() {
            return Err(AppError::new(
                "CredentialBusy",
                "系统钥匙串仍在处理上一项操作，请稍后重试。",
                true,
            ));
        }
        let service = self.clone();
        let cancel = CancellationToken::new();
        {
            let mut state = self.state();
            state.operation = Some(name);
            state.cancel = (name == "testing").then(|| cancel.clone());
        }
        (self.notify)();
        let (reply, response) = oneshot::channel();
        tauri::async_runtime::spawn(async move {
            let completion = OperationCompletion {
                service: service.clone(),
                permit: Some(permit),
            };
            let result = match service.runtime.reserve_settings_operation().await {
                Ok(_lease) => run(service.clone(), cancel).await,
                Err(error) => Err(error),
            };
            drop(completion);
            let _ = reply.send(result);
        });
        response.await.map_err(|_| {
            AppError::new(
                "ProviderUnavailable",
                "AI 服务配置未能完成，请重新打开设置检查。",
                true,
            )
        })?
    }

    pub(crate) async fn save(&self, input: SaveInput) -> Result<()> {
        // Immediately wrap/validate IPC key material; no key enters a Store closure.
        let remove = matches!(&input.credential, CredentialChange::Remove);
        let key = match input.credential {
            CredentialChange::Replace { key } => Some(SecretKey::new(key).map_err(provider_error)?),
            _ => None,
        };
        let config =
            Configuration::validate(input.configuration.clone()).map_err(provider_error)?;
        let model = input.configuration.model;
        let kind = kind(input.configuration.protocol).to_owned();
        let profile = serde_json::to_string(&input.configuration.protocol)
            .map_err(|_| AppError::invalid("AI 请求配置无效。"))?;
        let endpoint = config.endpoint().to_owned();
        let destination = origin(&endpoint)?;
        let local = config.is_local();
        let id = input.provider_id.unwrap_or_else(Uuid::new_v4);
        self.operation("saving", move |service, _| async move {
            let expected = input.expected_revision;
            let (settings, records) = service
                .runtime
                .query(|store| Ok((store.settings()?, store.providers()?)))
                .await?;
            if settings.revision != expected {
                return Err(AppError::conflict());
            }
            let previous = records.iter().find(|record| record.draft.provider_id == id);
            if previous.map(|record| record.revision) != input.provider_revision {
                return Err(AppError::conflict());
            }
            let previous_reference = previous.and_then(|r| r.draft.credential_ref.clone());
            let destination_changed = previous.is_some_and(|old| {
                old.draft.kind != kind
                    || origin(&old.draft.endpoint).ok().as_deref() != Some(destination.as_str())
            });
            if destination_changed && previous_reference.is_some() && key.is_none() && !remove {
                return Err(AppError::new(
                    "CredentialRebindRequired",
                    "服务类型或地址已改变，请重新输入密钥，或明确移除旧密钥。",
                    false,
                ));
            }
            let confirmed = local
                || input.confirm_destination
                || previous.is_some_and(|old| {
                    !destination_changed
                        && old.draft.consent_origin.as_deref() == Some(destination.as_str())
                });
            if !confirmed {
                return Err(AppError::new(
                    "DestinationConsentRequired",
                    "请先确认文字发送到此服务地址。",
                    false,
                ));
            }
            if local && key.is_some() {
                return Err(AppError::invalid("本地 Ollama 无需保存 API Key。"));
            }
            let mut draft = ProviderDraft {
                provider_id: id,
                display_name: input.display_name,
                kind,
                endpoint,
                model_name: model,
                request_profile_json: profile,
                consent_origin: (!local).then_some(destination),
                credential_ref: if remove || local {
                    None
                } else {
                    previous_reference
                },
            };
            draft.validate()?;
            if let Some(key) = key {
                let reference = service
                    .runtime
                    .database(move |store| {
                        store.reserve_provider_credential(expected, id, crate::now())
                    })
                    .await?;
                draft.credential_ref = Some(reference.clone());
                if let Err(error) = service.vault.write(reference, key).await {
                    service.cleanup().await;
                    return Err(error);
                }
            }
            let saved = service
                .runtime
                .database(move |store| {
                    store.save_provider(expected, input.provider_revision, draft, crate::now())
                })
                .await;
            // A failed commit is not assumed to mean "key unused". Cleanup reads
            // authoritative references before deleting any journaled credential.
            service.state().forget(&id);
            service.cleanup().await;
            saved.map(|_| ())
        })
        .await
    }

    pub(crate) async fn delete(&self, expected: i64, id: Uuid) -> Result<()> {
        self.operation("deleting", move |service, _| async move {
            service
                .runtime
                .database(move |store| store.delete_provider(expected, id, crate::now()))
                .await?;
            service.state().forget(&id);
            service.cleanup().await;
            Ok(())
        })
        .await
    }

    pub(crate) async fn discard_invalid(&self, expected: i64, recovery_id: Uuid) -> Result<()> {
        self.operation("recovering", move |service, _| async move {
            let target = {
                let mut state = service.state();
                let review = state
                    .recovery
                    .remove(&recovery_id)
                    .ok_or_else(recovery_expired)?;
                if review.observed.elapsed() > std::time::Duration::from_secs(300) {
                    return Err(recovery_expired());
                }
                review.target
            };
            service
                .runtime
                .database(move |store| store.discard_provider_entry(expected, target, crate::now()))
                .await?;
            // A malformed ID cannot be used to index the prepared cache.
            // Clear only the in-memory cache; healthy durable receipts survive.
            service.state().forget_all();
            service.cleanup().await;
            Ok(())
        })
        .await
    }

    pub(crate) async fn select(&self, expected: i64, id: Option<Uuid>) -> Result<()> {
        self.operation("selecting", move |service, _| async move {
            let prepared = if let Some(id) = id {
                let (settings, records) = service
                    .runtime
                    .query(|store| Ok((store.settings()?, store.providers()?)))
                    .await?;
                if settings.revision != expected {
                    return Err(AppError::conflict());
                }
                let record = records
                    .into_iter()
                    .find(|record| record.draft.provider_id == id)
                    .ok_or_else(AppError::conflict)?;
                Some((id, service.restore(&record, &settings).await?))
            } else {
                None
            };
            service
                .runtime
                .database(move |store| store.select_provider(expected, id, crate::now()))
                .await?;
            if let Some((id, prepared)) = prepared {
                service.state().tested.insert(id, prepared);
            }
            Ok(())
        })
        .await
    }

    // Restoration is lazy and local: construction/view do not touch Keychain or
    // contact a provider. The first explicit use revalidates every native binding.
    async fn restore(
        &self,
        record: &ProviderRecord,
        settings: &speakmore_core::types::Settings,
    ) -> Result<Arc<Tested>> {
        self.session_access().restore(record, settings).await
    }

    pub(crate) async fn validate_meeting_provider(&self, id: Uuid) -> Result<()> {
        self.runtime
            .query(move |db| {
                let record = db
                    .providers()?
                    .into_iter()
                    .find(|r| r.draft.provider_id == id)
                    .ok_or_else(|| AppError::invalid("服务不存在，请重新选择。"))?;
                let config = configuration(&record)?;
                check_destination(&record, &config, &db.settings()?)?;
                restored_test(&record, &config)?;
                Ok(())
            })
            .await
    }

    /// Meeting processing reuses saved credentials, destination policy and tested profiles.
    pub(crate) async fn generate_text(
        &self,
        id: Uuid,
        instructions: &str,
        data: &str,
        task: TextTask,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<String> {
        let cancel = CancellationToken::new();
        let remote = std::sync::atomic::AtomicBool::new(false);
        let run = async {
            let (settings, records) = self
                .runtime
                .query(|store| Ok((store.settings()?, store.providers()?)))
                .await?;
            let record = records
                .into_iter()
                .find(|r| r.draft.provider_id == id)
                .ok_or_else(|| {
                    AppError::new("TextProviderMissing", "AI 服务已移除，请重新选择。", true)
                })?;
            remote.store(
                !configuration(&record)?.is_local(),
                std::sync::atomic::Ordering::Release,
            );
            let tested = self.restore(&record, &settings).await?;
            let consent = tested.provider.confirm_destination();
            let policy = if settings.processing_policy == "allow_cloud" {
                ProcessingPolicy::AllowCloud(&consent)
            } else {
                ProcessingPolicy::LocalOnly
            };
            match task {
                TextTask::Meeting => {
                    self.meeting_client
                        .generate_json(
                            &tested.provider,
                            &tested.receipt,
                            data,
                            instructions,
                            policy,
                            &cancel,
                        )
                        .await
                }
                TextTask::Assistant => {
                    self.assistant_client
                        .generate_json(
                            &tested.provider,
                            &tested.receipt,
                            data,
                            instructions,
                            policy,
                            &cancel,
                        )
                        .await
                }
                TextTask::Caption => {
                    self.caption_client
                        .translate(
                            &tested.provider,
                            &tested.receipt,
                            data,
                            instructions,
                            policy,
                            &cancel,
                        )
                        .await
                }
            }
            .map_err(provider_error)
        };
        tokio::select! {
            biased;
            reason = async {
                loop {
                    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                        break AppError::new("Cancelled", "已停止整理，逐字稿已保留。", true);
                    }
                    if remote.load(std::sync::atomic::Ordering::Acquire)
                        && self.runtime.snapshot().is_ok_and(|s| s.settings.processing_policy != "allow_cloud") {
                        break AppError::new("ProviderLocalOnly", "云端处理已关闭，已停止整理并保留逐字稿。", true);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                }
            } => { cancel.cancel(); Err(reason) },
            result = tokio::time::timeout(std::time::Duration::from_secs(match task { TextTask::Meeting | TextTask::Assistant => 125, TextTask::Caption => 10 }), run) =>
                result.map_err(|_| AppError::new("TextProviderTimeout", "AI 服务响应超时，原文已保留，请重试。", true))?,
        }
    }

    pub(crate) async fn retry_cleanup(&self) -> Result<()> {
        self.operation("cleanup", |service, _| async move {
            service.cleanup().await;
            service.state().cleanup_error.clone().map_or(Ok(()), Err)
        })
        .await
    }

    async fn cleanup(&self) {
        let result = async {
            let pending = self
                .runtime
                .query(|store| store.pending_provider_credentials())
                .await?;
            for reference in pending {
                let inspect = reference.clone();
                if self
                    .runtime
                    .query(move |store| store.provider_credential_in_use(&inspect))
                    .await?
                {
                    return Err(AppError::new(
                        "CredentialInUse",
                        "此凭据仍被配置引用，暂不能清理。",
                        false,
                    ));
                }
                self.vault.delete(reference.clone()).await?;
                self.runtime
                    .database(move |store| store.acknowledge_provider_cleanup(&reference))
                    .await?;
            }
            Ok::<(), AppError>(())
        }
        .await;
        self.state().cleanup_error = result.err();
    }

    pub(crate) fn cancel_test(&self) {
        if let Some(cancel) = &self.state().cancel {
            cancel.cancel();
        }
    }

    pub(crate) async fn test(&self, id: Uuid, revision: i64) -> Result<()> {
        self.operation("testing", move |service, cancel| async move {
            service.state().forget(&id);
            service.runtime.database(move |store| store.invalidate_provider_test(id, revision, crate::now())).await?;
            let (settings, records) = service.runtime.query(|store| Ok((store.settings()?, store.providers()?))).await?;
            let record = records.into_iter().find(|r| r.draft.provider_id == id && r.revision == revision).ok_or_else(AppError::conflict)?;
            let config = configuration(&record)?;
            let local = config.is_local();
            check_destination(&record, &config, &settings)?;
            let consent = config.confirm_destination();
            let binding = test_binding(&record);
            let key = if let Some(reference) = record.draft.credential_ref {
                Some(tokio::select! { biased; _ = cancel.cancelled() => return Err(provider_error(speakmore_providers::Failure::Cancelled)), result = service.vault.read(reference) => result? })
            } else { None };
            let provider = Arc::new(PreparedProvider::new(config, key).map_err(provider_error)?);
            let policy = if local { ProcessingPolicy::LocalOnly } else { ProcessingPolicy::AllowCloud(&consent) };
            let receipt = service.client.test_connection(&provider, policy, &cancel).await.map_err(provider_error)?;
            let checkpoint = receipt.checkpoint(&provider, &binding).map_err(provider_error)?;
            if cancel.is_cancelled() { return Err(provider_error(speakmore_providers::Failure::Cancelled)); }
            let persist_cancel = cancel.clone();
            let commit_state = service.state.clone();
            let notify = service.notify.clone();
            service.runtime.database(move |store| {
                if !local && store.settings()?.processing_policy != "allow_cloud" { return Err(provider_error(speakmore_providers::Failure::LocalOnly)); }
                // Linearize cancellation before accepting the durable save. No
                // state mutex is held during SQL or OS work. Once saving begins,
                // cancellation cannot claim to revoke a committed test result.
                {
                    let mut state = commit_state.lock().unwrap_or_else(|e| e.into_inner());
                    if persist_cancel.is_cancelled() { return Err(provider_error(speakmore_providers::Failure::Cancelled)); }
                    state.cancel = None;
                    state.operation = Some("saving_test");
                }
                notify();
                store.mark_provider_tested(id, revision, &checkpoint, crate::now())
            }).await?;
            service.state().tested.insert(id, Arc::new(Tested { revision, provider, receipt }));
            Ok(())
        }).await
    }
}

fn test_binding(record: &ProviderRecord) -> String {
    format!(
        "{}:{}:{}",
        record.draft.provider_id,
        record.revision,
        record
            .draft
            .credential_ref
            .as_ref()
            .map(|reference| reference.account())
            .unwrap_or_else(|| "no-key".into())
    )
}
fn restored_test(
    record: &ProviderRecord,
    config: &Configuration,
) -> Result<speakmore_providers::RestoredTest> {
    if record.tested_at.is_none_or(|time| time < 0) || record.revision < 1 {
        return Err(provider_error(
            speakmore_providers::Failure::ProfileNotTested,
        ));
    }
    speakmore_providers::RestoredTest::verify(
        config,
        &test_binding(record),
        record.test_checkpoint.as_deref().unwrap_or_default(),
    )
    .map_err(provider_error)
}
fn check_destination(
    record: &ProviderRecord,
    config: &Configuration,
    settings: &speakmore_core::types::Settings,
) -> Result<()> {
    if !config.is_local() {
        if settings.processing_policy != "allow_cloud" {
            return Err(provider_error(speakmore_providers::Failure::LocalOnly));
        }
        if record.draft.consent_origin.as_deref() != Some(origin(config.endpoint())?.as_str()) {
            return Err(provider_error(
                speakmore_providers::Failure::DestinationConsentRequired,
            ));
        }
    }
    Ok(())
}

fn kind(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Openai => "openai",
        Protocol::Anthropic => "anthropic",
        Protocol::Ollama => "ollama",
        Protocol::OpenaiCompatible { .. } => "openai_compatible",
    }
}
fn recovery_expired() -> AppError {
    AppError::new(
        "ProviderRecoveryExpired",
        "配置已改变或确认已过期，请刷新服务列表后重试。",
        true,
    )
}
fn protocol(record: &ProviderRecord) -> Result<Protocol> {
    let protocol: Protocol = serde_json::from_str(&record.draft.request_profile_json)
        .map_err(|_| AppError::invalid("已保存的 AI 请求配置无效。"))?;
    if kind(protocol) != record.draft.kind {
        return Err(AppError::invalid("已保存的 AI 服务类型不一致。"));
    }
    Ok(protocol)
}
fn configuration(record: &ProviderRecord) -> Result<Configuration> {
    Configuration::validate(ConfigurationInput {
        protocol: protocol(record)?,
        endpoint: record.draft.endpoint.clone(),
        model: record.draft.model_name.clone(),
    })
    .map_err(provider_error)
}
fn origin(endpoint: &str) -> Result<String> {
    url::Url::parse(endpoint)
        .map(|url| url.origin().ascii_serialization())
        .map_err(|_| AppError::invalid("AI 服务地址无效。"))
}

fn provider_error(reason: speakmore_providers::Failure) -> AppError {
    use speakmore_providers::Failure::*;
    let (code, text) = match reason {
        InvalidEndpoint => (
            "ProviderEndpointInvalid",
            "请输入完整服务地址；云端仅支持 HTTPS，且地址不能包含密钥参数。",
        ),
        InvalidModel => (
            "ProviderModelInvalid",
            "请输入此服务实际可用的精确模型 ID。",
        ),
        CredentialRequired => ("CredentialMissing", "请先保存此服务的 API Key。"),
        ProfileNotTested => (
            "ProviderNotTested",
            "此服务没有与当前配置匹配的测试记录，请重新测试连接。",
        ),
        InvalidCredential | CredentialNotSupported => (
            "CredentialInvalid",
            "API Key 格式无效，或此服务不需要密钥。",
        ),
        LocalOnly => (
            "ProviderLocalOnly",
            "当前为仅本机处理；请先在隐私设置中明确允许云端文字处理。",
        ),
        DestinationConsentRequired => (
            "DestinationConsentRequired",
            "请先确认此服务的数据发送地址。",
        ),
        Cancelled => (
            "ProviderTestCancelled",
            "连接测试已取消；已经发送的测试句无法撤回。",
        ),
        Deadline => (
            "ProviderDeadline",
            "服务响应超时，请检查网络或选择响应更快的模型。",
        ),
        Busy => ("ProviderBusy", "AI 服务正在处理其他操作，请稍后重试。"),
        HttpStatus { status: 401 | 403 } => (
            "ProviderUnauthorized",
            "服务拒绝认证，请检查 API Key 与模型访问权限。",
        ),
        HttpStatus { status: 429 } => (
            "ProviderRateLimited",
            "服务额度不足或请求受限，请检查服务商账户后重试。",
        ),
        NetworkUnavailable => (
            "ProviderNetworkUnavailable",
            "无法安全连接服务，请检查地址、网络或本地服务状态。",
        ),
        RefusedResponse => ("ProviderRefused", "服务拒绝处理请求，未采用返回内容。"),
        _ => (
            "ProviderResponseInvalid",
            "服务响应不符合完整文本要求，请检查模型和请求配置。",
        ),
    };
    AppError::new(code, text, true)
}
