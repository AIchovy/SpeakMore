use super::*;
use speakmore_core::providers::CredentialRef;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Condvar,
};

#[derive(Default)]
pub(crate) struct TestVault {
    values: Mutex<HashMap<CredentialRef, String>>,
    hold: Mutex<bool>,
    wake: Condvar,
    read_hold: Mutex<bool>,
    read_wake: Condvar,
    writes: AtomicUsize,
    reads: AtomicUsize,
    delete_failure: AtomicBool,
    write_failure: AtomicBool,
}
impl TestVault {
    pub(crate) fn hold_writes(self: &Arc<Self>) -> impl Drop {
        struct Release(Arc<TestVault>);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.block_writes(false);
            }
        }
        self.block_writes(true);
        Release(self.clone())
    }
    pub(crate) fn block_writes(&self, value: bool) {
        *self.hold.lock().unwrap() = value;
        self.wake.notify_all();
    }
    /// Reads still count on entry, then wait until the guard drops.
    pub(crate) fn hold_reads(self: &Arc<Self>) -> impl Drop {
        struct Release(Arc<TestVault>);
        impl Drop for Release {
            fn drop(&mut self) {
                *self.0.read_hold.lock().unwrap() = false;
                self.0.read_wake.notify_all();
            }
        }
        *self.read_hold.lock().unwrap() = true;
        Release(self.clone())
    }
    pub(crate) fn writes(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }
    pub(crate) fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
    pub(crate) fn entries(&self) -> usize {
        self.values.lock().unwrap().len()
    }
    pub(crate) fn fail_delete(&self, value: bool) {
        self.delete_failure.store(value, Ordering::SeqCst);
    }
    pub(crate) fn fail_write_after_commit(&self, value: bool) {
        self.write_failure.store(value, Ordering::SeqCst);
    }
}
impl vault::Backend for Arc<TestVault> {
    fn read(&mut self, reference: &CredentialRef) -> Result<SecretKey> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let mut hold = self.read_hold.lock().unwrap();
        while *hold {
            hold = self.read_wake.wait(hold).unwrap();
        }
        drop(hold);
        let key = self
            .values
            .lock()
            .unwrap()
            .get(reference)
            .cloned()
            .ok_or_else(|| AppError::new("CredentialMissing", "合成凭据不存在。", true))?;
        SecretKey::new(key).map_err(provider_error)
    }
    fn write(&mut self, reference: &CredentialRef, key: &SecretKey) -> Result<()> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        let mut hold = self.hold.lock().unwrap();
        while *hold {
            hold = self.wake.wait(hold).unwrap();
        }
        self.values
            .lock()
            .unwrap()
            .insert(reference.clone(), key.expose().to_owned());
        if self.write_failure.load(Ordering::SeqCst) {
            Err(AppError::new(
                "CredentialUnavailable",
                "合成写入状态未知。",
                true,
            ))
        } else {
            Ok(())
        }
    }
    fn delete(&mut self, reference: &CredentialRef) -> Result<()> {
        if self.delete_failure.load(Ordering::SeqCst) {
            return Err(AppError::new(
                "CredentialDeleteUnconfirmed",
                "合成清理失败。",
                true,
            ));
        }
        self.values.lock().unwrap().remove(reference);
        Ok(())
    }
}

impl Service {
    pub(crate) fn expire_recovery_for_test(&self) {
        for review in self.state().recovery.values_mut() {
            review.observed = std::time::Instant::now() - std::time::Duration::from_secs(301);
        }
    }
    pub(crate) fn fixture(runtime: Handle) -> (Self, Arc<TestVault>) {
        let vault = Arc::new(TestVault::default());
        let service = Self {
            runtime,
            vault: vault::Vault::for_test(Box::new(vault.clone())),
            client: speakmore_providers::Client::new().unwrap(),
            meeting_client: speakmore_providers::Client::new().unwrap(),
            caption_client: speakmore_providers::Client::new().unwrap(),
            assistant_client: speakmore_providers::Client::new().unwrap(),
            slot: Arc::new(Semaphore::new(1)),
            state: Arc::new(Mutex::new(State::default())),
            notify: Arc::new(|| {}),
        };
        service
            .runtime
            .install_polisher(service.session_access())
            .unwrap();
        (service, vault)
    }
}
