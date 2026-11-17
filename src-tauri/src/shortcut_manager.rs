//! OS registration and durable preferences form a staged transaction. Candidate
//! callbacks remain inactive until persistence succeeds; old callbacks remain
//! registered through that boundary. Never hold a state mutex across OS or DB work.
use serde::Serialize;
use speakmore_core::{
    error::{AppError, Result},
    shortcuts::{Action, Binding, Preferences},
};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

const ACTIONS: [Action; 2] = [Action::PushToTalk, Action::Cancel];
pub(crate) fn index(action: Action) -> usize {
    if action == Action::PushToTalk {
        0
    } else {
        1
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Registration {
    pub action: Action,
    pub accelerator: String,
    pub generation: u64,
}
pub(crate) trait Platform {
    fn register(&self, registration: &Registration) -> Result<()>;
    fn unregister(&self, registration: &Registration) -> Result<()>;
    fn activate(&self, action: Action, registration: Option<&Registration>);
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BindingStatus {
    pub action: Action,
    #[serde(flatten)]
    pub binding: Binding,
    pub registered: bool,
    pub error: Option<AppError>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    pub revision: u64,
    pub preferences: Preferences,
    pub bindings: Vec<BindingStatus>,
    pub error: Option<AppError>,
}
struct State {
    revision: u64,
    preferences: Preferences,
    registered: [Option<Registration>; 2],
    errors: [Option<AppError>; 2],
    orphans: Vec<Registration>,
    storage_error: Option<AppError>,
}
pub(crate) struct Manager {
    state: Mutex<State>,
    operation: tokio::sync::Mutex<()>,
    next_generation: AtomicU64,
}
impl Manager {
    pub fn new(preferences: Result<Preferences>) -> Self {
        let (preferences, storage_error) = match preferences {
            Ok(value) => (value, None),
            Err(error) => (Preferences::default(), Some(error)),
        };
        Self {
            state: Mutex::new(State {
                revision: 1,
                preferences,
                registered: [None, None],
                errors: [None, None],
                orphans: vec![],
                storage_error,
            }),
            operation: tokio::sync::Mutex::new(()),
            next_generation: AtomicU64::new(1),
        }
    }
    pub fn status(&self) -> Status {
        let state = self.state.lock().unwrap();
        Status {
            revision: state.revision,
            preferences: state.preferences.clone(),
            bindings: ACTIONS
                .iter()
                .map(|&action| BindingStatus {
                    action,
                    binding: state.preferences.binding(action).clone(),
                    registered: state.registered[index(action)].is_some(),
                    error: state.errors[index(action)].clone(),
                })
                .collect(),
            error: state
                .storage_error
                .clone()
                .or_else(|| (!state.orphans.is_empty()).then(cleanup_error)),
        }
    }
    fn candidate(&self, action: Action, accelerator: &str) -> Registration {
        Registration {
            action,
            accelerator: accelerator.into(),
            generation: self.next_generation.fetch_add(1, Ordering::Relaxed),
        }
    }
    fn cleanup(&self, platform: &impl Platform) -> Result<()> {
        let orphans = self.state.lock().unwrap().orphans.clone();
        for registration in orphans {
            platform
                .unregister(&registration)
                .map_err(|_| cleanup_error())?;
            let mut state = self.state.lock().unwrap();
            state.orphans.retain(|r| r != &registration);
            state.revision += 1;
        }
        Ok(())
    }
    fn discard(&self, platform: &impl Platform, registration: Registration) {
        if platform.unregister(&registration).is_err() {
            let mut state = self.state.lock().unwrap();
            state.orphans.push(registration);
            state.revision += 1;
        }
    }
    pub fn register_saved(&self, platform: &impl Platform) -> Result<()> {
        let _operation = self.operation.try_lock().map_err(|_| AppError::busy())?;
        self.cleanup(platform)?;
        for action in ACTIONS {
            let binding = {
                let state = self.state.lock().unwrap();
                if state.storage_error.is_some() {
                    return Ok(());
                }
                let binding = state.preferences.binding(action);
                if !binding.enabled || state.registered[index(action)].is_some() {
                    continue;
                }
                binding.clone()
            };
            let registration = self.candidate(action, &binding.accelerator);
            let result = platform.register(&registration);
            if result.is_ok() {
                platform.activate(action, Some(&registration));
            }
            let mut state = self.state.lock().unwrap();
            match result {
                Ok(()) => {
                    state.registered[index(action)] = Some(registration);
                    state.errors[index(action)] = None;
                }
                Err(error) => state.errors[index(action)] = Some(error),
            }
            state.revision += 1;
        }
        Ok(())
    }
    pub async fn replace<F: Future<Output = Result<()>>>(
        &self,
        platform: &impl Platform,
        action: Action,
        binding: Binding,
        expected: Preferences,
        persist: impl FnOnce(Preferences, Preferences) -> F,
    ) -> Result<()> {
        let next = expected.replace(action, binding)?;
        let _operation = self.operation.try_lock().map_err(|_| AppError::busy())?;
        self.cleanup(platform)?;
        let old = {
            let state = self.state.lock().unwrap();
            if state.storage_error.is_some() || state.preferences != expected {
                return Err(AppError::conflict());
            }
            state.registered[index(action)].clone()
        };
        let target = next.binding(action);
        let unchanged = target.enabled
            && old
                .as_ref()
                .is_some_and(|r| r.accelerator == target.accelerator);
        let candidate = if target.enabled && !unchanged {
            let registration = self.candidate(action, &target.accelerator);
            if let Err(error) = platform.register(&registration) {
                let mut state = self.state.lock().unwrap();
                state.errors[index(action)] = Some(error.clone());
                state.revision += 1;
                return Err(error);
            }
            Some(registration)
        } else {
            None
        };
        if let Err(error) = persist(expected, next.clone()).await {
            if let Some(candidate) = candidate {
                self.discard(platform, candidate);
            }
            return Err(error);
        }
        let active = if unchanged { old.clone() } else { candidate };
        platform.activate(action, active.as_ref());
        {
            let mut state = self.state.lock().unwrap();
            state.preferences = next;
            state.registered[index(action)] = active;
            state.errors[index(action)] = None;
            state.revision += 1;
        }
        if !unchanged {
            if let Some(old) = old {
                self.discard(platform, old);
            }
        }
        Ok(())
    }
}
fn cleanup_error() -> AppError {
    AppError::new(
        "ShortcutCleanupFailed",
        "部分旧快捷键尚未确认释放。请重试，或重新启动 SpeakMore。",
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Fake {
        registered: Mutex<Vec<Registration>>,
        active: Mutex<[Option<Registration>; 2]>,
        fail_register: Mutex<Option<String>>,
        fail_unregister: Mutex<Option<String>>,
    }
    impl Platform for Fake {
        fn register(&self, registration: &Registration) -> Result<()> {
            if self.fail_register.lock().unwrap().as_ref() == Some(&registration.accelerator) {
                return Err(AppError::busy());
            }
            let mut all = self.registered.lock().unwrap();
            assert!(!all
                .iter()
                .any(|r| r.accelerator == registration.accelerator));
            all.push(registration.clone());
            Ok(())
        }
        fn unregister(&self, registration: &Registration) -> Result<()> {
            if self.fail_unregister.lock().unwrap().as_ref() == Some(&registration.accelerator) {
                return Err(AppError::busy());
            }
            self.registered
                .lock()
                .unwrap()
                .retain(|r| r != registration);
            Ok(())
        }
        fn activate(&self, action: Action, registration: Option<&Registration>) {
            if let Some(r) = registration {
                assert!(self.registered.lock().unwrap().contains(r));
            }
            self.active.lock().unwrap()[index(action)] = registration.cloned();
        }
    }
    fn replacement() -> Binding {
        Binding {
            accelerator: "Control+Alt+KeyD".into(),
            enabled: true,
        }
    }
    #[tokio::test]
    async fn candidate_stays_inactive_until_save_and_both_actions_restore_independently() {
        let platform = Fake::default();
        let manager = Manager::new(Ok(Preferences::default()));
        manager.register_saved(&platform).unwrap();
        let old = platform.active.lock().unwrap()[0].clone().unwrap();
        assert_eq!(platform.registered.lock().unwrap().len(), 1);
        let observed = &platform;
        manager
            .replace(
                &platform,
                Action::PushToTalk,
                replacement(),
                Preferences::default(),
                |expected, next| async move {
                    tokio::task::yield_now().await;
                    assert_eq!(observed.active.lock().unwrap()[0], Some(old.clone()));
                    assert_eq!(observed.registered.lock().unwrap().len(), 2);
                    assert_eq!(expected, Preferences::default());
                    assert_eq!(next.push_to_talk, replacement());
                    Ok(())
                },
            )
            .await
            .unwrap();
        assert_eq!(platform.registered.lock().unwrap().len(), 1);
        let preferences = manager.status().preferences;
        let mut cancel = preferences.cancel.clone();
        cancel.enabled = true;
        manager
            .replace(
                &platform,
                Action::Cancel,
                cancel,
                preferences,
                |_, _| async { Ok(()) },
            )
            .await
            .unwrap();
        assert!(manager.status().bindings.iter().all(|s| s.registered));
        let restored = Manager::new(Ok(manager.status().preferences));
        let second_process = Fake::default();
        restored.register_saved(&second_process).unwrap();
        assert_eq!(second_process.registered.lock().unwrap().len(), 2);
        assert_eq!(restored.status().preferences.push_to_talk, replacement());
    }
    #[tokio::test]
    async fn registration_and_storage_failures_preserve_the_original_binding() {
        let platform = Fake::default();
        let manager = Manager::new(Ok(Preferences::default()));
        manager.register_saved(&platform).unwrap();
        let old = platform.active.lock().unwrap()[0].clone();
        *platform.fail_register.lock().unwrap() = Some(replacement().accelerator);
        assert!(manager
            .replace(
                &platform,
                Action::PushToTalk,
                replacement(),
                Preferences::default(),
                |_, _| async { panic!("must not save after registration failure") }
            )
            .await
            .is_err());
        assert_eq!(platform.active.lock().unwrap()[0], old);
        *platform.fail_register.lock().unwrap() = None;
        assert!(manager
            .replace(
                &platform,
                Action::PushToTalk,
                replacement(),
                Preferences::default(),
                |_, _| async { Err(AppError::busy()) }
            )
            .await
            .is_err());
        assert_eq!(platform.active.lock().unwrap()[0], old);
        assert_eq!(platform.registered.lock().unwrap().len(), 1);
        assert_eq!(manager.status().preferences, Preferences::default());
        let mut disabled = Preferences::default().push_to_talk;
        disabled.enabled = false;
        assert!(manager
            .replace(
                &platform,
                Action::PushToTalk,
                disabled.clone(),
                Preferences::default(),
                |_, _| async { Err(AppError::busy()) }
            )
            .await
            .is_err());
        assert_eq!(platform.active.lock().unwrap()[0], old);
        manager
            .replace(
                &platform,
                Action::PushToTalk,
                disabled,
                Preferences::default(),
                |_, _| async { Ok(()) },
            )
            .await
            .unwrap();
        assert!(platform.registered.lock().unwrap().is_empty());
        assert!(platform.active.lock().unwrap()[0].is_none());
        let restored = Manager::new(Ok(manager.status().preferences));
        restored.register_saved(&platform).unwrap();
        assert!(platform.registered.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn failed_cleanup_is_visible_inactive_and_blocks_additional_registrations() {
        let platform = Fake::default();
        let manager = Manager::new(Ok(Preferences::default()));
        manager.register_saved(&platform).unwrap();
        *platform.fail_unregister.lock().unwrap() =
            Some(Preferences::default().push_to_talk.accelerator);
        manager
            .replace(
                &platform,
                Action::PushToTalk,
                replacement(),
                Preferences::default(),
                |_, _| async { Ok(()) },
            )
            .await
            .unwrap();
        assert!(manager.status().error.is_some());
        assert_eq!(
            platform.active.lock().unwrap()[0]
                .as_ref()
                .unwrap()
                .accelerator,
            replacement().accelerator
        );
        let mut next = replacement();
        next.accelerator = "Control+Alt+KeyR".into();
        assert!(manager
            .replace(
                &platform,
                Action::PushToTalk,
                next,
                manager.status().preferences,
                |_, _| async { panic!("cleanup must precede new work") }
            )
            .await
            .is_err());
        assert_eq!(platform.registered.lock().unwrap().len(), 2);
        *platform.fail_unregister.lock().unwrap() = None;
        manager.register_saved(&platform).unwrap();
        assert!(manager.status().error.is_none());
        assert_eq!(platform.registered.lock().unwrap().len(), 1);
    }
    #[test]
    fn startup_conflict_keeps_saved_preference_and_retry_registers_it() {
        let platform = Fake::default();
        let manager = Manager::new(Ok(Preferences::default()));
        *platform.fail_register.lock().unwrap() =
            Some(Preferences::default().push_to_talk.accelerator);
        manager.register_saved(&platform).unwrap();
        assert!(!manager.status().bindings[0].registered);
        assert!(manager.status().bindings[0].error.is_some());
        let payload = serde_json::to_value(manager.status()).unwrap();
        assert_eq!(payload["bindings"][0]["error"]["code"], "Busy");
        assert_eq!(payload["bindings"][0]["error"]["retryable"], true);
        assert!(manager.status().preferences.push_to_talk.enabled);
        *platform.fail_register.lock().unwrap() = None;
        manager.register_saved(&platform).unwrap();
        assert!(manager.status().bindings[0].registered);
        assert!(manager.status().bindings[0].error.is_none());
    }
    #[test]
    fn storage_failure_retains_its_error_identity_in_status() {
        let manager = Manager::new(Err(AppError::new(
            "StorageUnavailable",
            "本地数据不可用。",
            true,
        )));
        let status = serde_json::to_value(manager.status()).unwrap();
        assert_eq!(status["error"]["code"], "StorageUnavailable");
        assert_eq!(status["error"]["userMessage"], "本地数据不可用。");
        assert_eq!(status["error"]["retryable"], true);
        assert_eq!(status["bindings"][0]["registered"], false);
    }
}
