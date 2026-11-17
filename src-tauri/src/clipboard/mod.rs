//! One writer shared by explicit copy and a reserved automatic attempt. Never
//! reads previous clipboard text, restores old contents, or retries a write.
#[cfg(target_os = "macos")]
mod mac;

use speakmore_core::{
    delivery::Status,
    error::{AppError, Result},
};
use std::{
    sync::{
        atomic::{AtomicU8, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(3);
const QUEUED: u8 = 0;
const WRITING: u8 = 1;
const CANCELLED_BEFORE: u8 = 2;
const CANCELLED_AFTER: u8 = 3;

pub(crate) struct Request {
    marker: Uuid,
    deadline: Instant,
    phase: AtomicU8,
    guard: Option<Arc<crate::delivery::Attempt>>,
}
impl Request {
    pub(crate) fn begin(&self) -> Result<()> {
        if Instant::now() >= self.deadline || self.guard.as_ref().is_some_and(|g| !g.live()) {
            return Err(expired());
        }
        self.phase
            .compare_exchange(QUEUED, WRITING, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| expired())
    }
    pub(crate) fn check(&self) -> Result<()> {
        if self.phase.load(Ordering::Acquire) == WRITING
            && Instant::now() < self.deadline
            && self.guard.as_ref().is_none_or(|g| g.live())
        {
            Ok(())
        } else {
            Err(uncertain())
        }
    }
    pub(crate) fn stamp(&self, generation: isize) -> Stamp {
        Stamp {
            generation,
            marker: self.marker,
        }
    }
    /// Refresh a stale target observation before a claim; never fails by itself.
    pub(crate) fn confirm_target(&self) {
        if let (Ok(()), Some(guard)) = (self.check(), self.guard.as_ref()) {
            guard.confirm();
        }
    }
    pub(crate) fn claim_submission(&self) -> bool {
        self.check().is_ok() && self.guard.as_ref().is_some_and(|g| g.claim())
    }
    fn cancel(&self) -> bool {
        let old = self
            .phase
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| match state {
                QUEUED => Some(CANCELLED_BEFORE),
                WRITING => Some(CANCELLED_AFTER),
                _ => None,
            })
            .unwrap_or_else(|state| state);
        old == WRITING || old == CANCELLED_AFTER
    }
}
struct CancelOnDrop(Arc<Request>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
#[derive(Clone)]
pub(crate) struct Stamp {
    pub(crate) generation: isize,
    pub(crate) marker: Uuid,
}
pub(crate) trait Writer {
    fn write(&mut self, text: &str, request: &Request) -> Result<()>;
    fn modifiers(&mut self, request: &Request) -> Result<bool> {
        request.begin()?;
        Err(unavailable())
    }
    fn prepare(&mut self, _text: &str, _request: &Request) -> Result<Stamp> {
        Err(unavailable())
    }
    fn owns(&mut self, _stamp: &Stamp, request: &Request) -> Result<bool> {
        request.begin()?;
        Err(unavailable())
    }
    fn submit(&mut self, _stamp: &Stamp, _request: &Request) -> Result<Status> {
        Err(unavailable())
    }
}
struct Job {
    text: String,
    request: Arc<Request>,
    reply: oneshot::Sender<Result<()>>,
    // Held until the OS call returns, even when its caller times out or drops.
    _permit: OwnedSemaphorePermit,
}
type Work = Box<dyn FnOnce(&mut dyn Writer) + Send>;
enum Task {
    Copy(Job),
    Automatic(Work),
}
#[derive(Clone)]
pub(crate) struct Service {
    sender: mpsc::SyncSender<Task>,
    available: Arc<Semaphore>,
}
impl Service {
    #[cfg(test)]
    pub(crate) fn for_test(factory: impl FnOnce() -> Box<dyn Writer> + Send + 'static) -> Self {
        Self::spawn(factory).unwrap()
    }
    pub(crate) fn native() -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            Self::spawn(|| Box::new(mac::Writer::new()))
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self::spawn(|| Box::new(OtherPlatform))
        }
    }
    fn spawn(factory: impl FnOnce() -> Box<dyn Writer> + Send + 'static) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Task>(1);
        std::thread::Builder::new()
            .name("speakmore-clipboard".into())
            .spawn(move || {
                let mut writer = factory();
                while let Ok(task) = receiver.recv() {
                    let Task::Copy(job) = task else {
                        if let Task::Automatic(work) = task {
                            work(&mut *writer);
                        }
                        continue;
                    };
                    let result = if job.reply.is_closed() {
                        Err(expired())
                    } else {
                        writer.write(&job.text, &job.request)
                    };
                    drop(job._permit);
                    let _ = job.reply.send(result);
                }
            })
            .map_err(|_| unavailable())?;
        Ok(Self {
            sender,
            available: Arc::new(Semaphore::new(1)),
        })
    }
    pub(crate) async fn copy(&self, text: String) -> Result<()> {
        self.copy_with_timeout(text, TIMEOUT).await
    }
    pub(crate) fn reserve(&self) -> Result<Reservation> {
        let permit = self.available.clone().try_acquire_owned().map_err(|_| {
            AppError::new(
                "ClipboardBusy",
                "剪贴板正在处理其他操作，请手动复制。",
                true,
            )
        })?;
        Ok(Reservation {
            service: self.clone(),
            permit: Arc::new(permit),
            stalled: false,
        })
    }
    async fn copy_with_timeout(&self, text: String, timeout: Duration) -> Result<()> {
        if text.is_empty() || text.len() > speakmore_core::MAX_TEXT_BYTES || text.contains('\0') {
            return Err(AppError::new(
                "InvalidCopyText",
                "文字为空或超过复制长度上限。",
                false,
            ));
        }
        let permit = self.available.clone().try_acquire_owned().map_err(|_| {
            AppError::new("ClipboardBusy", "上一次复制尚未结束，请稍后再试。", true)
        })?;
        let request = Arc::new(Request {
            marker: Uuid::new_v4(),
            deadline: Instant::now() + timeout,
            phase: AtomicU8::new(QUEUED),
            guard: None,
        });
        let _cancel = CancelOnDrop(request.clone());
        let (reply, received) = oneshot::channel();
        self.sender
            .try_send(Task::Copy(Job {
                text,
                request: request.clone(),
                reply,
                _permit: permit,
            }))
            .map_err(|_| unavailable())?;
        match tokio::time::timeout_at(tokio::time::Instant::from_std(request.deadline), received)
            .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(if request.cancel() {
                uncertain()
            } else {
                unavailable()
            }),
            Err(_) => Err(if request.cancel() {
                uncertain()
            } else {
                expired()
            }),
        }
    }
}
pub(crate) struct Reservation {
    service: Service,
    permit: Arc<OwnedSemaphorePermit>,
    stalled: bool,
}
impl Reservation {
    pub(crate) async fn call<T: Send + 'static>(
        &mut self,
        guard: Option<Arc<crate::delivery::Attempt>>,
        timeout: Duration,
        action: impl FnOnce(&mut dyn Writer, &Request) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        if self.stalled {
            return Err(uncertain());
        }
        let request = Arc::new(Request {
            marker: Uuid::new_v4(),
            deadline: Instant::now() + timeout.min(TIMEOUT),
            phase: AtomicU8::new(QUEUED),
            guard,
        });
        let _cancel = CancelOnDrop(request.clone());
        let worker_request = request.clone();
        let permit = self.permit.clone();
        let (reply, received) = oneshot::channel();
        self.service
            .sender
            .try_send(Task::Automatic(Box::new(move |writer| {
                let _permit = permit;
                let result = if reply.is_closed() {
                    Err(expired())
                } else {
                    action(writer, &worker_request)
                };
                let _ = reply.send(result);
            })))
            .map_err(|_| unavailable())?;
        match tokio::time::timeout_at(tokio::time::Instant::from_std(request.deadline), received)
            .await
        {
            Ok(Ok(result)) => result,
            _ => {
                self.stalled = true;
                Err(if request.cancel() {
                    uncertain()
                } else {
                    expired()
                })
            }
        }
    }
    pub(crate) async fn write(
        &mut self,
        text: String,
        guard: Arc<crate::delivery::Attempt>,
    ) -> Result<Stamp> {
        self.call(Some(guard), TIMEOUT, move |writer, request| {
            let stamp = writer.prepare(&text, request)?;
            if stamp.marker != request.marker {
                return Err(uncertain());
            }
            Ok(stamp)
        })
        .await
    }
    pub(crate) async fn owns(&mut self, stamp: Stamp) -> Result<bool> {
        self.call(None, TIMEOUT, move |writer, request| {
            writer.owns(&stamp, request)
        })
        .await
    }
}
fn unavailable() -> AppError {
    AppError::new(
        "ClipboardUnavailable",
        "暂时无法写入剪贴板，文字仍保留在这里。",
        true,
    )
}
fn expired() -> AppError {
    AppError::new("CopyExpired", "复制请求已取消或超时，尚未开始写入。", true)
}
fn uncertain() -> AppError {
    AppError::new(
        "CopyUncertain",
        "无法确认复制结果，请检查剪贴板；文字仍保留在这里。",
        false,
    )
}
#[cfg(target_os = "macos")]
fn changed() -> AppError {
    AppError::new(
        "ClipboardChanged",
        "剪贴板已被其他操作修改，请确认后重新复制。",
        true,
    )
}

#[cfg(not(target_os = "macos"))]
struct OtherPlatform;
#[cfg(not(target_os = "macos"))]
impl Writer for OtherPlatform {
    fn write(&mut self, text: &str, request: &Request) -> Result<()> {
        request.begin()?;
        arboard::Clipboard::new()
            .and_then(|mut c| c.set_text(text))
            .map_err(|_| unavailable())?;
        request.check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{atomic::AtomicUsize, Condvar, Mutex};
    #[derive(Default)]
    struct Gate {
        blocked: Mutex<bool>,
        wake: Condvar,
        entered: AtomicUsize,
        writes: AtomicUsize,
    }
    impl Gate {
        fn wait(&self) {
            self.entered.fetch_add(1, Ordering::SeqCst);
            let mut blocked = self.blocked.lock().unwrap();
            while *blocked {
                blocked = self.wake.wait(blocked).unwrap();
            }
        }
        fn release(&self) {
            *self.blocked.lock().unwrap() = false;
            self.wake.notify_all();
        }
    }
    struct Fake(Arc<Gate>);
    impl Writer for Fake {
        fn write(&mut self, _: &str, request: &Request) -> Result<()> {
            request.begin()?;
            self.0.wait();
            request.check()?;
            self.0.writes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    async fn until(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !ready() {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    #[tokio::test]
    async fn timed_out_os_write_retains_single_slot_until_it_returns_without_retry() {
        let gate = Arc::new(Gate::default());
        *gate.blocked.lock().unwrap() = true;
        let worker_gate = gate.clone();
        let service = Arc::new(Service::spawn(move || Box::new(Fake(worker_gate))).unwrap());
        let client = service.clone();
        let first = tokio::spawn(async move {
            client
                .copy_with_timeout("first".into(), Duration::from_millis(150))
                .await
        });
        until(|| gate.entered.load(Ordering::SeqCst) == 1).await;
        assert_eq!(
            service.copy("second".into()).await.unwrap_err().code,
            "ClipboardBusy"
        );
        assert_eq!(first.await.unwrap().unwrap_err().code, "CopyUncertain");
        assert_eq!(
            service.copy("third".into()).await.unwrap_err().code,
            "ClipboardBusy"
        );
        assert_eq!(gate.entered.load(Ordering::SeqCst), 1);
        gate.release();
        until(|| service.available.available_permits() == 1).await;
        assert_eq!(gate.writes.load(Ordering::SeqCst), 0);
        service.copy("new explicit request".into()).await.unwrap();
        assert_eq!(gate.writes.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn expired_queued_request_never_reaches_os_writer_after_startup_unblocks() {
        let startup = Arc::new(Gate::default());
        *startup.blocked.lock().unwrap() = true;
        let writes = Arc::new(Gate::default());
        let (factory_gate, writer_gate) = (startup.clone(), writes.clone());
        let service = Service::spawn(move || {
            factory_gate.wait();
            Box::new(Fake(writer_gate))
        })
        .unwrap();
        assert_eq!(
            service
                .copy_with_timeout("expired".into(), Duration::from_millis(50))
                .await
                .unwrap_err()
                .code,
            "CopyExpired"
        );
        startup.release();
        until(|| service.available.available_permits() == 1).await;
        assert_eq!(writes.entered.load(Ordering::SeqCst), 0);
        assert_eq!(writes.writes.load(Ordering::SeqCst), 0);
        service.copy("new copy".into()).await.unwrap();
        assert_eq!(writes.writes.load(Ordering::SeqCst), 1);
    }
    #[tokio::test]
    async fn dropped_client_cancels_write_and_invalid_text_never_enters_queue() {
        let gate = Arc::new(Gate::default());
        *gate.blocked.lock().unwrap() = true;
        let worker_gate = gate.clone();
        let service = Arc::new(Service::spawn(move || Box::new(Fake(worker_gate))).unwrap());
        for text in [
            String::new(),
            "has\0nul".into(),
            "x".repeat(speakmore_core::MAX_TEXT_BYTES + 1),
        ] {
            assert_eq!(
                service.copy(text).await.unwrap_err().code,
                "InvalidCopyText"
            );
        }
        assert_eq!(gate.entered.load(Ordering::SeqCst), 0);
        let client = service.clone();
        let pending = tokio::spawn(async move { client.copy("abandoned".into()).await });
        until(|| gate.entered.load(Ordering::SeqCst) == 1).await;
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        assert_eq!(service.available.available_permits(), 0);
        gate.release();
        until(|| service.available.available_permits() == 1).await;
        assert_eq!(gate.writes.load(Ordering::SeqCst), 0);
    }
}
