//! A single native credential thread. An OS call may outlive its caller, but it
//! retains the sole slot until it returns. Never start replacement vault threads.
use speakmore_core::{
    error::{AppError, Result},
    providers::CredentialRef,
};
use speakmore_providers::SecretKey;
use std::sync::{mpsc, Arc};
use tokio::sync::{oneshot, OwnedSemaphorePermit, Semaphore};
use zeroize::Zeroize;

pub(super) const SERVICE: &str = "SpeakMore";

pub(super) trait Backend: Send {
    fn read(&mut self, reference: &CredentialRef) -> Result<SecretKey>;
    fn write(&mut self, reference: &CredentialRef, key: &SecretKey) -> Result<()>;
    fn delete(&mut self, reference: &CredentialRef) -> Result<()>;
}

type Work = Box<dyn FnOnce(&mut dyn Backend, OwnedSemaphorePermit) + Send>;
struct Job {
    work: Work,
    permit: OwnedSemaphorePermit,
}

#[derive(Clone)]
pub(super) struct Vault {
    sender: mpsc::SyncSender<Job>,
    slot: Arc<Semaphore>,
}

impl Vault {
    #[cfg(test)]
    pub(super) fn for_test(backend: Box<dyn Backend>) -> Self {
        Self::spawn(backend).unwrap()
    }
    pub(super) fn native() -> Result<Self> {
        Self::spawn(Box::new(Native))
    }

    fn spawn(backend: Box<dyn Backend>) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Job>(1);
        std::thread::Builder::new()
            .name("speakmore-credentials".into())
            .spawn(move || {
                let mut backend = backend;
                while let Ok(Job { work, permit }) = receiver.recv() {
                    work(backend.as_mut(), permit);
                }
            })
            .map_err(|_| error("CredentialUnavailable"))?;
        Ok(Self {
            sender,
            slot: Arc::new(Semaphore::new(1)),
        })
    }

    pub(super) fn busy(&self) -> bool {
        self.slot.available_permits() == 0
    }

    async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut dyn Backend) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self
            .slot
            .clone()
            .try_acquire_owned()
            .map_err(|_| error("CredentialBusy"))?;
        let (reply, response) = oneshot::channel();
        self.sender
            .try_send(Job {
                permit,
                work: Box::new(move |backend, permit| {
                    // A queued request whose owner disappeared must not begin an OS call.
                    if !reply.is_closed() {
                        let result = work(backend);
                        drop(permit);
                        let _ = reply.send(result);
                    }
                }),
            })
            .map_err(|_| error("CredentialUnavailable"))?;
        response.await.map_err(|_| error("CredentialUnavailable"))?
    }

    pub(super) async fn read(&self, reference: CredentialRef) -> Result<SecretKey> {
        self.run(move |backend| backend.read(&reference)).await
    }
    pub(super) async fn write(&self, reference: CredentialRef, key: SecretKey) -> Result<()> {
        self.run(move |backend| backend.write(&reference, &key))
            .await
    }
    pub(super) async fn delete(&self, reference: CredentialRef) -> Result<()> {
        self.run(move |backend| backend.delete(&reference)).await
    }
}

struct Native;

#[cfg(any(target_os = "macos", windows))]
fn entry(reference: &CredentialRef) -> Result<keyring::Entry> {
    // Construct the exact native credential directly. A default/mock builder
    // elsewhere in the process can never redirect real credentials into a mock.
    #[cfg(target_os = "macos")]
    let credential =
        keyring::macos::MacCredential::new_with_target(Some("User"), SERVICE, &reference.account())
            .map_err(map_error)?;
    #[cfg(windows)]
    let credential =
        keyring::windows::WinCredential::new_with_target(None, SERVICE, &reference.account())
            .map_err(map_error)?;
    Ok(keyring::Entry::new_with_credential(Box::new(credential)))
}

#[cfg(any(target_os = "macos", windows))]
impl Backend for Native {
    fn read(&mut self, reference: &CredentialRef) -> Result<SecretKey> {
        let password = entry(reference)?.get_password().map_err(map_error)?;
        SecretKey::new(password).map_err(|_| error("CredentialInvalid"))
    }
    fn write(&mut self, reference: &CredentialRef, key: &SecretKey) -> Result<()> {
        entry(reference)?
            .set_password(key.expose())
            .map_err(map_error)
    }
    fn delete(&mut self, reference: &CredentialRef) -> Result<()> {
        let entry = entry(reference)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(error) => return Err(map_error(error)),
        }
        // keyring 3.6.3's macOS implementation ignores item.delete's OS status.
        // Only a subsequent NoEntry confirms deletion; never report success when
        // the credential is still present or the confirmation read was denied.
        match entry.get_secret() {
            Err(keyring::Error::NoEntry) => Ok(()),
            Ok(mut bytes) => {
                bytes.zeroize();
                Err(error("CredentialDeleteUnconfirmed"))
            }
            Err(error) => Err(map_error(error)),
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
impl Backend for Native {
    fn read(&mut self, _: &CredentialRef) -> Result<SecretKey> {
        Err(error("CredentialUnsupported"))
    }
    fn write(&mut self, _: &CredentialRef, _: &SecretKey) -> Result<()> {
        Err(error("CredentialUnsupported"))
    }
    fn delete(&mut self, _: &CredentialRef) -> Result<()> {
        Err(error("CredentialUnsupported"))
    }
}

#[cfg(any(target_os = "macos", windows))]
fn map_error(value: keyring::Error) -> AppError {
    use keyring::Error::*;
    let code = match value {
        NoEntry => "CredentialMissing",
        BadEncoding(mut bytes) => {
            bytes.zeroize();
            "CredentialInvalid"
        }
        Ambiguous(_) => "CredentialAmbiguous",
        Invalid(..) | TooLong(..) => "CredentialInvalid",
        NoStorageAccess(detail) | PlatformFailure(detail) => {
            #[cfg(target_os = "macos")]
            {
                match detail
                    .downcast_ref::<security_framework::base::Error>()
                    .map(|e| e.code())
                {
                    Some(-128) => "CredentialCancelled",
                    Some(-25293) => "CredentialDenied",
                    Some(-25308 | -25315) => "CredentialInteractionRequired",
                    _ => "CredentialUnavailable",
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = detail;
                "CredentialUnavailable"
            }
        }
        _ => "CredentialUnavailable",
    };
    error(code)
}

fn error(code: &str) -> AppError {
    let message = match code {
        "CredentialBusy" => "系统钥匙串仍在处理上一项操作，请稍后重试。",
        "CredentialMissing" => "保存的密钥已不存在，请重新输入。",
        "CredentialInvalid" => "保存的凭据格式无效，请重新输入密钥。",
        "CredentialAmbiguous" => "找到多个匹配凭据，暂时无法确定要使用的密钥。",
        "CredentialCancelled" => "已取消系统钥匙串授权。",
        "CredentialDenied" => "系统钥匙串拒绝访问，请检查授权后重试。",
        "CredentialInteractionRequired" => "钥匙串需要解锁或交互授权，请处理系统提示后重试。",
        "CredentialDeleteUnconfirmed" => "凭据仍在系统钥匙串中，清理尚未完成，请重试。",
        "CredentialUnsupported" => "当前平台尚未接入系统凭据存储。",
        _ => "无法访问系统钥匙串，请检查系统状态后重试。",
    };
    AppError::new(
        code,
        message,
        !matches!(code, "CredentialInvalid" | "CredentialUnsupported"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Mutex};

    #[derive(Default)]
    struct Memory {
        values: HashMap<CredentialRef, String>,
        thread: Option<std::thread::ThreadId>,
    }
    impl Memory {
        fn entered(&mut self) {
            let thread = std::thread::current().id();
            if let Some(previous) = self.thread {
                assert_eq!(previous, thread);
            }
            self.thread = Some(thread);
        }
    }
    impl Backend for Arc<Mutex<Memory>> {
        fn read(&mut self, reference: &CredentialRef) -> Result<SecretKey> {
            let mut memory = self.lock().unwrap();
            memory.entered();
            SecretKey::new(
                memory
                    .values
                    .get(reference)
                    .cloned()
                    .ok_or_else(|| error("CredentialMissing"))?,
            )
            .map_err(|_| error("CredentialInvalid"))
        }
        fn write(&mut self, reference: &CredentialRef, key: &SecretKey) -> Result<()> {
            let mut memory = self.lock().unwrap();
            memory.entered();
            memory
                .values
                .insert(reference.clone(), key.expose().to_owned());
            Ok(())
        }
        fn delete(&mut self, reference: &CredentialRef) -> Result<()> {
            let mut memory = self.lock().unwrap();
            memory.entered();
            memory.values.remove(reference);
            Ok(())
        }
    }

    #[tokio::test]
    async fn calls_use_one_native_thread_and_release_slot_before_acknowledgement() {
        let memory = Arc::new(Mutex::new(Memory::default()));
        let vault = Vault::for_test(Box::new(memory.clone()));
        let reference = CredentialRef::new(uuid::Uuid::new_v4()).unwrap();
        for _ in 0..10 {
            vault
                .write(
                    reference.clone(),
                    SecretKey::new("synthetic-key".into()).unwrap(),
                )
                .await
                .unwrap();
            assert!(!vault.busy());
            let key = vault.read(reference.clone()).await.unwrap();
            assert_eq!(key.expose(), "synthetic-key");
            vault.delete(reference.clone()).await.unwrap();
            assert_eq!(
                vault.read(reference.clone()).await.unwrap_err().code,
                "CredentialMissing"
            );
        }
        assert!(memory.lock().unwrap().values.is_empty());
    }

    struct Blocked {
        entered: Option<oneshot::Sender<()>>,
        release: mpsc::Receiver<()>,
    }
    impl Backend for Blocked {
        fn read(&mut self, _: &CredentialRef) -> Result<SecretKey> {
            let _ = self.entered.take().unwrap().send(());
            let _ = self.release.recv();
            SecretKey::new("synthetic-key".into()).map_err(|_| error("CredentialInvalid"))
        }
        fn write(&mut self, _: &CredentialRef, _: &SecretKey) -> Result<()> {
            panic!("busy write must not enter native backend")
        }
        fn delete(&mut self, _: &CredentialRef) -> Result<()> {
            panic!("busy delete must not enter native backend")
        }
    }
    #[tokio::test]
    async fn dropped_reader_does_not_release_a_still_blocked_native_slot() {
        let (entered, receiver) = oneshot::channel();
        let (release, released) = mpsc::channel();
        let vault = Vault::for_test(Box::new(Blocked {
            entered: Some(entered),
            release: released,
        }));
        let reference = CredentialRef::new(uuid::Uuid::new_v4()).unwrap();
        let worker = vault.clone();
        let read_reference = reference.clone();
        let task = tokio::spawn(async move { worker.read(read_reference).await });
        receiver.await.unwrap();
        task.abort();
        assert!(matches!(task.await, Err(error) if error.is_cancelled()));
        assert!(vault.busy());
        assert_eq!(
            vault.delete(reference).await.unwrap_err().code,
            "CredentialBusy"
        );
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while vault.busy() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_constructor_and_error_mapping_never_use_or_expose_mock_credentials() {
        let reference = CredentialRef::new(uuid::Uuid::new_v4()).unwrap();
        let entry = entry(&reference).unwrap();
        let credential = entry
            .get_credential()
            .downcast_ref::<keyring::macos::MacCredential>()
            .unwrap();
        assert_eq!(credential.service, SERVICE);
        assert_eq!(credential.account, reference.account());
        assert_eq!(credential.domain, keyring::macos::MacKeychainDomain::User);
        for (status, code) in [
            (-128, "CredentialCancelled"),
            (-25293, "CredentialDenied"),
            (-25308, "CredentialInteractionRequired"),
            (-25315, "CredentialInteractionRequired"),
            (-25291, "CredentialUnavailable"),
        ] {
            let error = map_error(keyring::macos::decode_error(
                security_framework::base::Error::from_code(status),
            ));
            assert_eq!(error.code, code);
            assert!(!error.user_message.contains(&status.to_string()));
        }
        let malformed = map_error(keyring::Error::BadEncoding(
            b"synthetic-private-key\xff".to_vec(),
        ));
        assert!(!serde_json::to_string(&malformed)
            .unwrap()
            .contains("synthetic-private-key"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "Explicit native Keychain test; creates and removes only one fresh synthetic SpeakMore account"]
    async fn native_keychain_synthetic_roundtrip_and_verified_removal() {
        let vault = Vault::native().unwrap();
        let reference = CredentialRef::new(uuid::Uuid::new_v4()).unwrap();
        assert_eq!(
            vault.read(reference.clone()).await.unwrap_err().code,
            "CredentialMissing"
        );
        let write = vault
            .write(
                reference.clone(),
                SecretKey::new("speakmore-synthetic-validation-only".into()).unwrap(),
            )
            .await;
        let read = vault.read(reference.clone()).await;
        let deletion = vault.delete(reference.clone()).await;
        // An OS write can fail after creating an item. Attempt cleanup even when
        // write/read failed, before any assertion can unwind this test.
        deletion.unwrap();
        write.unwrap();
        assert_eq!(
            read.unwrap().expose(),
            "speakmore-synthetic-validation-only"
        );
        assert_eq!(
            vault.read(reference).await.unwrap_err().code,
            "CredentialMissing"
        );
    }
}
