//! In-memory platform only. No system clipboard, keyboard events or AX access.
use super::*;
use crate::{
    clipboard::{Request, Service, Writer},
    target::{Driver, Service as Targets, Watch},
};
use std::sync::{
    atomic::{AtomicU8, AtomicUsize},
    Condvar, Mutex,
};
pub(crate) const NORMAL: u8 = 0;
pub(crate) const MODIFIERS: u8 = 1;
pub(crate) const TARGET_AFTER_WRITE: u8 = 2;
pub(crate) const CLIPBOARD_CHANGED: u8 = 3;
pub(crate) const DISPATCH_UNCERTAIN: u8 = 4;
pub(crate) const WRITE_UNCERTAIN: u8 = 5;
pub(crate) struct State {
    pub(crate) mode: AtomicU8,
    pub(crate) qualified: AtomicBool,
    pub(crate) reason: Mutex<Option<ManualReason>>,
    pub(crate) checks: AtomicUsize,
    pub(crate) writes: AtomicUsize,
    pub(crate) posts: AtomicUsize,
    pub(crate) entering_submit: AtomicUsize,
    pub(crate) blocked: Mutex<bool>,
    pub(crate) wake: Condvar,
    pub(crate) before_submit: Mutex<Option<Box<dyn Fn() + Send>>>,
    stamp: Mutex<Option<Stamp>>,
}
impl State {
    pub(crate) fn new(mode: u8) -> Arc<Self> {
        Arc::new(Self {
            mode: AtomicU8::new(mode),
            qualified: AtomicBool::new(true),
            reason: Mutex::new(None),
            checks: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
            posts: AtomicUsize::new(0),
            entering_submit: AtomicUsize::new(0),
            blocked: Mutex::new(false),
            wake: Condvar::new(),
            before_submit: Mutex::new(None),
            stamp: Mutex::new(None),
        })
    }
    pub(crate) fn release(&self) {
        *self.blocked.lock().unwrap() = false;
        self.wake.notify_all();
    }
}
struct Platform(Arc<State>);
impl Driver for Platform {
    fn capture(&mut self, _: u64, _: Instant) -> std::result::Result<Box<dyn Watch>, ManualReason> {
        Ok(Box::new(Platform(self.0.clone())))
    }
}
impl Watch for Platform {
    fn qualified(&self) -> bool {
        self.0.qualified.load(Ordering::SeqCst)
    }
    fn check(&mut self, _: Instant) -> std::result::Result<(), ManualReason> {
        self.0.reason.lock().unwrap().map_or(Ok(()), Err)
    }
    fn recheck(&mut self, deadline: Instant) -> std::result::Result<(), ManualReason> {
        self.0.checks.fetch_add(1, Ordering::SeqCst);
        self.check(deadline)
    }
}
impl Writer for Platform {
    fn write(&mut self, text: &str, request: &Request) -> Result<()> {
        self.prepare(text, request).map(|_| ())
    }
    fn modifiers(&mut self, request: &Request) -> Result<bool> {
        request.begin()?;
        Ok(self.0.mode.load(Ordering::SeqCst) != MODIFIERS)
    }
    fn prepare(&mut self, text: &str, request: &Request) -> Result<Stamp> {
        assert!(!text.is_empty());
        request.begin()?;
        let generation = self.0.writes.fetch_add(1, Ordering::SeqCst) + 1;
        let stamp = request.stamp(generation as isize);
        *self.0.stamp.lock().unwrap() = Some(stamp.clone());
        match self.0.mode.load(Ordering::SeqCst) {
            TARGET_AFTER_WRITE => {
                *self.0.reason.lock().unwrap() = Some(ManualReason::TargetChanged)
            }
            CLIPBOARD_CHANGED => *self.0.stamp.lock().unwrap() = None,
            WRITE_UNCERTAIN => return Err(AppError::new("CopyUncertain", "fixture", false)),
            _ => {}
        }
        Ok(stamp)
    }
    fn owns(&mut self, stamp: &Stamp, request: &Request) -> Result<bool> {
        request.begin()?;
        Ok(self
            .0
            .stamp
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|current| {
                current.generation == stamp.generation && current.marker == stamp.marker
            }))
    }
    fn submit(&mut self, stamp: &Stamp, request: &Request) -> Result<Status> {
        request.begin()?;
        request.confirm_target();
        self.0.entering_submit.fetch_add(1, Ordering::SeqCst);
        let mut blocked = self.0.blocked.lock().unwrap();
        while *blocked {
            blocked = self.0.wake.wait(blocked).unwrap();
        }
        if let Some(hook) = self.0.before_submit.lock().unwrap().as_ref() {
            hook();
        }
        if self
            .0
            .stamp
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(|s| s.marker != stamp.marker)
        {
            return Ok(Status::ReadyForCopy);
        }
        if !request.claim_submission() {
            return Ok(Status::CopiedOnly);
        }
        assert!(
            !request.claim_submission(),
            "native submission must be claimed once"
        );
        self.0.posts.fetch_add(1, Ordering::SeqCst);
        if self.0.mode.load(Ordering::SeqCst) == DISPATCH_UNCERTAIN {
            return Err(AppError::new("CopyUncertain", "fixture", false));
        }
        Ok(Status::Submitted)
    }
}
pub(crate) fn services(state: &Arc<State>) -> (Targets, Arc<Service>) {
    let target_state = state.clone();
    let clipboard_state = state.clone();
    (
        Targets::for_test(move || Box::new(Platform(target_state))),
        Arc::new(Service::for_test(move || {
            Box::new(Platform(clipboard_state))
        })),
    )
}
pub(crate) async fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(4);
    while !predicate() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
