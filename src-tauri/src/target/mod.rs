//! A single bounded native probe thread. Target objects/context never leave it.
//! This service only observes targets; the delivery service owns paste submission.
#[cfg(target_os = "macos")]
pub(crate) mod diagnostics;
#[cfg(target_os = "macos")]
mod mac;
use speakmore_core::delivery::ManualReason;
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use uuid::Uuid;

const DEADLINE: Duration = Duration::from_millis(300);
const CHECK_EVERY: Duration = Duration::from_millis(100);
// Heartbeats end well before the lease deadline, so a slow read is reported as
// inconclusive instead of racing a lease reader into a sticky timeout.
const HEARTBEAT: Duration = Duration::from_millis(200);
type ProbeResult<T> = Result<T, ManualReason>;
/// Plain-data guard run by the paste writer right before posting. It never
/// touches the probe thread's native target objects.
pub(crate) type Sentinel = Arc<dyn Fn() -> bool + Send + Sync>;
pub(crate) trait Watch {
    // Only drivers that validate the original keyboard target may qualify.
    // AX text setters are not required for a normal paste into that target.
    fn qualified(&self) -> bool {
        false
    }
    fn check(&mut self, deadline: Instant) -> ProbeResult<()>;
    fn recheck(&mut self, deadline: Instant) -> ProbeResult<()> {
        self.check(deadline)
    }
    /// Heartbeat read. `Ok(false)` is inconclusive (a busy target or a starved
    /// probe): it neither refreshes nor fails the observation. Only definitive
    /// evidence fails a heartbeat; the final recheck stays strict.
    fn observe(&mut self, deadline: Instant) -> ProbeResult<bool> {
        match self.check(deadline) {
            Err(ManualReason::ProbeTimeout) => Ok(false),
            result => result.map(|()| true),
        }
    }
    fn sentinel(&self) -> Option<Sentinel> {
        None
    }
}
pub(crate) trait Driver {
    fn capture(&mut self, generation: u64, deadline: Instant) -> ProbeResult<Box<dyn Watch>>;
}
struct Job {
    id: Uuid,
    generation: u64,
    deadline: Instant,
    checked: Option<Instant>,
    reason: Option<ManualReason>,
    recheck: Option<Recheck>,
    qualified: bool,
    sentinel: Option<Sentinel>,
}
struct Recheck {
    id: Uuid,
    deadline: Instant,
    reply: mpsc::SyncSender<ProbeResult<()>>,
}
pub(crate) struct Check {
    session: Uuid,
    id: Uuid,
    deadline: Instant,
    shared: Arc<Hub>,
    reply: mpsc::Receiver<ProbeResult<()>>,
}
#[derive(Default)]
struct Shared {
    job: Option<Job>,
    closed: bool,
}
/// The probe thread blocks on `wake` while idle and between heartbeats.
#[derive(Default)]
struct Hub {
    state: Mutex<Shared>,
    wake: Condvar,
}
impl Hub {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}
pub(crate) struct Service(Arc<Hub>);
pub(crate) struct Lease {
    id: Uuid,
    shared: Arc<Hub>,
}
impl Service {
    #[cfg(test)]
    pub(crate) fn for_test(factory: impl FnOnce() -> Box<dyn Driver> + Send + 'static) -> Self {
        Self::spawn(factory).unwrap()
    }
    pub(crate) fn native(input: crate::session_runtime::NativeInput) -> Option<Self> {
        #[cfg(target_os = "macos")]
        {
            Self::spawn(move || Box::new(mac::Probe::new(input)))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = input;
            None
        }
    }
    fn spawn(factory: impl FnOnce() -> Box<dyn Driver> + Send + 'static) -> Option<Self> {
        let shared = Arc::new(Hub::default());
        let thread_shared = shared.clone();
        // Non-Send AX objects are created, used, and destroyed on this thread.
        std::thread::Builder::new()
            .name("speakmore-target".into())
            .spawn(move || {
                run(thread_shared, factory());
            })
            .ok()?;
        Some(Self(shared))
    }
    pub(crate) fn begin(&self, id: Uuid, generation: u64, requested_at: Instant) -> Lease {
        self.0.lock().job = Some(Job {
            id,
            generation,
            deadline: requested_at + DEADLINE,
            checked: None,
            reason: None,
            recheck: None,
            qualified: false,
            sentinel: None,
        });
        self.0.wake.notify_all();
        Lease {
            id,
            shared: self.0.clone(),
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        let mut state = self.0.lock();
        state.closed = true;
        state.job = None;
        self.0.wake.notify_all();
    }
}
impl Lease {
    #[cfg(feature = "target-audit")]
    pub(crate) fn audit_outcome(&self) -> Option<ProbeResult<()>> {
        // Apply the same sticky deadline as production lease readers.
        let _ = self.reason();
        let state = self.shared.lock();
        let Some(job) = state.job.as_ref().filter(|job| job.id == self.id) else {
            return Some(Err(ManualReason::TargetUnavailable));
        };
        job.reason.map(Err).or_else(|| job.checked.map(|_| Ok(())))
    }

    pub(crate) fn fresh(&self) -> bool {
        let state = self.shared.lock();
        state.job.as_ref().is_some_and(|j| {
            j.id == self.id
                && j.reason.is_none()
                && j.checked
                    .is_some_and(|at| at.elapsed() <= Duration::from_millis(100))
        })
    }
    pub(crate) fn qualified(&self) -> ProbeResult<()> {
        let reason = self.reason();
        let state = self.shared.lock();
        if reason == ManualReason::CompatibilityUnverified
            && state.job.as_ref().is_some_and(|j| {
                j.id == self.id && j.qualified && j.reason.is_none() && Instant::now() < j.deadline
            })
        {
            Ok(())
        } else {
            Err(reason)
        }
    }
    /// Last synchronous gate before a paste: replace a stale heartbeat with one
    /// new strict read. Any failure is sticky and seen by `fresh`/`qualified`.
    pub(crate) fn refresh(&self) {
        if !self.fresh() {
            if let Ok(check) = self.recheck() {
                let _ = check.wait();
            }
        }
    }
    /// Plain-data target guard, evaluated on the caller's thread.
    pub(crate) fn settled(&self) -> bool {
        let sentinel = self
            .shared
            .lock()
            .job
            .as_ref()
            .filter(|j| j.id == self.id)
            .map(|j| j.sentinel.clone());
        sentinel.is_some_and(|s| s.is_none_or(|s| s()))
    }
    pub(crate) fn end(&self) {
        let mut state = self.shared.lock();
        if state.job.as_ref().is_some_and(|j| j.id == self.id) {
            state.job = None;
            self.shared.wake.notify_all();
        }
    }
    pub(crate) fn invalidate(&self, reason: ManualReason) {
        let mut state = self.shared.lock();
        if let Some(job) = state.job.as_mut().filter(|j| j.id == self.id) {
            job.reason.get_or_insert(reason);
            self.shared.wake.notify_all();
        }
    }
    /// Queue a new read; a heartbeat already in flight cannot satisfy it.
    pub(crate) fn recheck(&self) -> ProbeResult<Check> {
        let mut state = self.shared.lock();
        let job = state
            .job
            .as_mut()
            .filter(|j| j.id == self.id)
            .ok_or(ManualReason::TargetUnavailable)?;
        if Instant::now() >= job.deadline {
            job.reason.get_or_insert(ManualReason::ProbeTimeout);
        }
        if let Some(reason) = job.reason {
            return Err(reason);
        }
        if job.recheck.is_some() {
            return Err(ManualReason::TargetUnavailable);
        }
        let id = Uuid::new_v4();
        let deadline = Instant::now() + DEADLINE;
        let (reply, received) = mpsc::sync_channel(1);
        job.recheck = Some(Recheck {
            id,
            deadline,
            reply,
        });
        self.shared.wake.notify_all();
        Ok(Check {
            session: self.id,
            id,
            deadline,
            shared: self.shared.clone(),
            reply: received,
        })
    }
    /// A healthy observation has no failure reason. Qualification is checked
    /// separately; the legacy reason remains for unqualified drivers.
    pub(crate) fn reason(&self) -> ManualReason {
        let mut state = self.shared.lock();
        let Some(job) = state.job.as_mut().filter(|j| j.id == self.id) else {
            return ManualReason::TargetUnavailable;
        };
        if Instant::now() > job.deadline {
            job.reason.get_or_insert(ManualReason::ProbeTimeout);
        }
        job.reason.unwrap_or(if job.checked.is_some() {
            ManualReason::CompatibilityUnverified
        } else {
            ManualReason::TargetUnavailable
        })
    }
}
impl Check {
    /// Nonblocking actor poll. The timeout is also sticky on the original lease.
    pub(crate) fn poll(&self) -> Option<ProbeResult<()>> {
        // An already completed answer still needs the caller's lease check: a
        // target invalidated after the reply must not be restored by this read.
        match self.reply.try_recv() {
            Ok(result) => return Some(self.settle(result)),
            Err(mpsc::TryRecvError::Disconnected) => {
                return Some(Err(ManualReason::TargetUnavailable))
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if Instant::now() < self.deadline {
            return None;
        }
        let mut state = self.shared.lock();
        if let Some(job) = state.job.as_mut().filter(|j| j.id == self.session) {
            job.reason.get_or_insert(ManualReason::ProbeTimeout);
            if job.recheck.as_ref().is_some_and(|r| r.id == self.id) {
                job.recheck = None;
            }
            return Some(Err(job.reason.unwrap()));
        }
        Some(Err(ManualReason::TargetUnavailable))
    }
    // A reply racing the end of its lease must not outlive that lease.
    fn settle(&self, result: ProbeResult<()>) -> ProbeResult<()> {
        result?;
        let state = self.shared.lock();
        match state.job.as_ref().filter(|j| j.id == self.session) {
            Some(job) => job.reason.map_or(Ok(()), Err),
            None => Err(ManualReason::TargetUnavailable),
        }
    }
    /// Blocking `poll` for a dedicated native thread, bounded by the read deadline.
    pub(crate) fn wait(&self) -> ProbeResult<()> {
        loop {
            if let Some(result) = self.poll() {
                return result;
            }
            match self
                .reply
                .recv_timeout(self.deadline.saturating_duration_since(Instant::now()))
            {
                Ok(result) => return self.settle(result),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(ManualReason::TargetUnavailable)
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.end();
    }
}
fn run(shared: Arc<Hub>, mut driver: Box<dyn Driver>) {
    let mut active: Option<(Uuid, Box<dyn Watch>)> = None;
    let mut polled = Instant::now();
    loop {
        let mut state = shared.lock();
        let request = loop {
            if state.closed {
                return;
            }
            let request = state.job.as_mut().and_then(|job| {
                if Instant::now() > job.deadline
                    || job
                        .recheck
                        .as_ref()
                        .is_some_and(|r| Instant::now() >= r.deadline)
                {
                    job.reason.get_or_insert(ManualReason::ProbeTimeout);
                }
                if let Some(reason) = job.reason {
                    if let Some(request) = job.recheck.take() {
                        let _ = request.reply.send(Err(reason));
                    }
                    return None;
                }
                Some((
                    job.id,
                    job.generation,
                    job.deadline,
                    job.recheck.as_ref().map(|r| (r.id, r.deadline)),
                ))
            });
            let Some(request) = request else {
                // Release a finished watch's native objects outside the lock.
                if active.is_some() {
                    break None;
                }
                state = shared.wake.wait(state).unwrap_or_else(|e| e.into_inner());
                continue;
            };
            let (id, _, _, recheck) = request;
            let due = polled + CHECK_EVERY;
            let now = Instant::now();
            if recheck.is_some() || now >= due || active.as_ref().is_none_or(|(old, _)| *old != id)
            {
                break Some(request);
            }
            state = shared
                .wake
                .wait_timeout(state, due - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        };
        drop(state);
        let Some((id, generation, deadline, recheck)) = request else {
            active = None;
            continue;
        };
        let first = active.as_ref().is_none_or(|(old, _)| *old != id);
        let heartbeat = !first && recheck.is_none();
        let deadline = match recheck {
            _ if first => deadline,
            Some((_, at)) => deadline.min(at),
            None => (Instant::now() + HEARTBEAT)
                .min(deadline.checked_sub(CHECK_EVERY).unwrap_or(deadline)),
        };
        let result = if first {
            active = None;
            driver
                .capture(generation, deadline)
                .map(|watch| active = Some((id, watch)))
                .map(|()| true)
        } else if recheck.is_some() {
            active.as_mut().unwrap().1.recheck(deadline).map(|()| true)
        } else {
            active.as_mut().unwrap().1.observe(deadline)
        };
        polled = Instant::now();
        #[cfg(target_os = "macos")]
        if let Err(reason) = &result {
            diagnostics::record_failure(
                if first {
                    "capture"
                } else if recheck.is_some() {
                    "recheck"
                } else {
                    "observe"
                },
                reason.as_str(),
            );
        }
        let mut state = shared.lock();
        if let Some(job) = state
            .job
            .as_mut()
            .filter(|j| j.id == id && j.reason.is_none())
        {
            let now = Instant::now();
            let late = now >= deadline;
            match result {
                Err(reason) if heartbeat || !late => job.reason = Some(reason),
                _ if late && !heartbeat => job.reason = Some(ManualReason::ProbeTimeout),
                Ok(true) if !late => {
                    job.checked = Some(now);
                    job.deadline = now + CHECK_EVERY + DEADLINE;
                    job.qualified = active.as_ref().is_some_and(|(_, w)| w.qualified());
                    if first {
                        job.sentinel = active.as_ref().and_then(|(_, w)| w.sentinel());
                    }
                }
                // Inconclusive heartbeat: the probe is alive; the last good
                // read stands and `fresh` stays false until a new one.
                _ => job.deadline = now + CHECK_EVERY + DEADLINE,
            }
            // Initial capture or an earlier heartbeat must not be passed
            // off as the independent final read requested by the actor.
            if !first
                && job
                    .recheck
                    .as_ref()
                    .is_some_and(|r| Some(r.id) == recheck.map(|r| r.0))
            {
                if let Some(request) = job.recheck.take() {
                    let _ = request.reply.send(job.reason.map_or(Ok(()), Err));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Condvar,
    };
    #[derive(Default)]
    struct Controls {
        blocked: Mutex<bool>,
        wake: Condvar,
        captures: AtomicUsize,
        checks: AtomicUsize,
        checks_blocked: Mutex<bool>,
        check_wake: Condvar,
        dropped: AtomicUsize,
        reason: Mutex<Option<ManualReason>>,
    }
    struct Fake(Arc<Controls>);
    impl Driver for Fake {
        fn capture(&mut self, _: u64, _: Instant) -> ProbeResult<Box<dyn Watch>> {
            self.0.captures.fetch_add(1, Ordering::SeqCst);
            let mut blocked = self.0.blocked.lock().unwrap();
            while *blocked {
                blocked = self.0.wake.wait(blocked).unwrap();
            }
            Ok(Box::new(Fake(self.0.clone())))
        }
    }
    impl Watch for Fake {
        fn check(&mut self, _: Instant) -> ProbeResult<()> {
            self.0.checks.fetch_add(1, Ordering::SeqCst);
            let mut blocked = self.0.checks_blocked.lock().unwrap();
            while *blocked {
                blocked = self.0.check_wake.wait(blocked).unwrap();
            }
            self.0.reason.lock().unwrap().map_or(Ok(()), Err)
        }
    }
    impl Drop for Fake {
        fn drop(&mut self) {
            self.0.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn until(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !predicate() {
            assert!(Instant::now() < deadline, "target probe did not settle");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn service(control: &Arc<Controls>) -> Service {
        let control = control.clone();
        Service::for_test(move || Box::new(Fake(control)))
    }
    #[test]
    fn final_recheck_requires_a_new_read_after_an_inflight_heartbeat() {
        let control = Arc::new(Controls::default());
        *control.checks_blocked.lock().unwrap() = true;
        let service = service(&control);
        let lease = service.begin(Uuid::new_v4(), 1, Instant::now());
        until(|| control.checks.load(Ordering::SeqCst) == 1);
        let check = lease.recheck().unwrap();
        assert!(check.poll().is_none());
        *control.checks_blocked.lock().unwrap() = false;
        control.check_wake.notify_all();
        let mut answer = None;
        until(|| {
            answer = check.poll();
            answer.is_some()
        });
        assert_eq!(answer, Some(Ok(())));
        assert!(control.checks.load(Ordering::SeqCst) >= 2);
        assert_eq!(lease.reason(), ManualReason::CompatibilityUnverified);
    }
    #[test]
    fn final_recheck_failure_is_sticky_and_not_replaced_by_cached_success() {
        let control = Arc::new(Controls::default());
        let service = service(&control);
        let lease = service.begin(Uuid::new_v4(), 1, Instant::now());
        until(|| lease.reason() == ManualReason::CompatibilityUnverified);
        *control.reason.lock().unwrap() = Some(ManualReason::TargetChanged);
        let check = lease.recheck().unwrap();
        let mut answer = None;
        until(|| {
            answer = check.poll();
            answer.is_some()
        });
        assert_eq!(answer, Some(Err(ManualReason::TargetChanged)));
        *control.reason.lock().unwrap() = None;
        assert_eq!(lease.reason(), ManualReason::TargetChanged);
        assert!(matches!(lease.recheck(), Err(ManualReason::TargetChanged)));
    }
    #[test]
    fn final_recheck_timeout_or_cancel_cannot_attach_to_next_session() {
        let control = Arc::new(Controls::default());
        *control.checks_blocked.lock().unwrap() = true;
        let service = service(&control);
        let lease = service.begin(Uuid::new_v4(), 1, Instant::now());
        until(|| control.checks.load(Ordering::SeqCst) == 1);
        let check = lease.recheck().unwrap();
        let mut answer = None;
        until(|| {
            answer = check.poll();
            answer.is_some()
        });
        assert_eq!(answer, Some(Err(ManualReason::ProbeTimeout)));
        assert_eq!(lease.reason(), ManualReason::ProbeTimeout);
        let next = service.begin(Uuid::new_v4(), 2, Instant::now());
        drop(lease);
        *control.checks_blocked.lock().unwrap() = false;
        control.check_wake.notify_all();
        until(|| next.reason() == ManualReason::CompatibilityUnverified);
        let abandoned = next.recheck().unwrap();
        drop(next);
        assert_eq!(abandoned.poll(), Some(Err(ManualReason::TargetUnavailable)));
    }
    #[test]
    fn queued_probe_budget_starts_at_key_press_and_expired_work_does_not_read_target() {
        let control = Arc::new(Controls::default());
        let service = service(&control);
        let lease = service.begin(
            Uuid::new_v4(),
            1,
            Instant::now() - DEADLINE - Duration::from_millis(1),
        );
        assert_eq!(lease.reason(), ManualReason::ProbeTimeout);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(control.captures.load(Ordering::SeqCst), 0);
    }
    #[test]
    fn target_change_is_permanent_and_cannot_be_repaired_by_returning_to_original() {
        let control = Arc::new(Controls::default());
        let service = service(&control);
        let lease = service.begin(Uuid::new_v4(), 10, Instant::now());
        until(|| lease.reason() == ManualReason::CompatibilityUnverified);
        *control.reason.lock().unwrap() = Some(ManualReason::TargetChanged);
        until(|| lease.reason() == ManualReason::TargetChanged);
        *control.reason.lock().unwrap() = None;
        until(|| control.dropped.load(Ordering::SeqCst) > 0);
        assert_eq!(lease.reason(), ManualReason::TargetChanged);
        assert_eq!(control.captures.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn inconclusive_heartbeats_keep_the_target_but_final_reads_stay_strict() {
        let control = Arc::new(Controls::default());
        let service = service(&control);
        let lease = service.begin(Uuid::new_v4(), 1, Instant::now());
        until(|| lease.fresh());
        *control.reason.lock().unwrap() = Some(ManualReason::ProbeTimeout);
        let checks = control.checks.load(Ordering::SeqCst);
        std::thread::sleep(CHECK_EVERY * 2 + DEADLINE);
        assert!(control.checks.load(Ordering::SeqCst) >= checks + 3);
        assert_eq!(lease.reason(), ManualReason::CompatibilityUnverified);
        assert!(!lease.fresh());
        assert!(lease.settled());
        *control.reason.lock().unwrap() = None;
        lease.refresh();
        assert!(lease.fresh());
        *control.reason.lock().unwrap() = Some(ManualReason::ProbeTimeout);
        assert_eq!(
            lease.recheck().unwrap().wait(),
            Err(ManualReason::ProbeTimeout)
        );
        assert_eq!(lease.reason(), ManualReason::ProbeTimeout);
        lease.end();
        assert!(!lease.settled());
    }
    #[test]
    fn hung_probe_has_one_thread_and_late_answer_cannot_revive_timed_out_sessions() {
        let control = Arc::new(Controls::default());
        *control.blocked.lock().unwrap() = true;
        let service = service(&control);
        let first = service.begin(Uuid::new_v4(), 1, Instant::now());
        until(|| control.captures.load(Ordering::SeqCst) == 1);
        until(|| first.reason() == ManualReason::ProbeTimeout);
        let second = service.begin(Uuid::new_v4(), 2, Instant::now());
        until(|| second.reason() == ManualReason::ProbeTimeout);
        assert_eq!(control.captures.load(Ordering::SeqCst), 1);
        *control.blocked.lock().unwrap() = false;
        control.wake.notify_all();
        until(|| control.dropped.load(Ordering::SeqCst) > 0);
        assert_eq!(second.reason(), ManualReason::ProbeTimeout);
        let next = service.begin(Uuid::new_v4(), 3, Instant::now());
        until(|| next.reason() == ManualReason::CompatibilityUnverified);
        assert_eq!(control.captures.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn old_session_cannot_cancel_or_invalidate_new_watch_and_shutdown_does_not_wait_for_probe() {
        let control = Arc::new(Controls::default());
        let service = service(&control);
        let old = service.begin(Uuid::new_v4(), 1, Instant::now());
        until(|| old.reason() == ManualReason::CompatibilityUnverified);
        let next = service.begin(Uuid::new_v4(), 2, Instant::now());
        old.invalidate(ManualReason::UserRequested);
        drop(old);
        until(|| next.reason() == ManualReason::CompatibilityUnverified);
        drop(next);
        *control.blocked.lock().unwrap() = true;
        let blocked = service.begin(Uuid::new_v4(), 3, Instant::now());
        until(|| control.captures.load(Ordering::SeqCst) == 3);
        let at = Instant::now();
        drop(service);
        assert!(at.elapsed() < Duration::from_millis(50));
        assert_eq!(blocked.reason(), ManualReason::TargetUnavailable);
        *control.blocked.lock().unwrap() = false;
        control.wake.notify_all();
        until(|| control.dropped.load(Ordering::SeqCst) >= 4);
    }
}

/// Metadata capture is independent of automatic paste qualification.
pub(crate) fn writing_context(
    input: crate::session_runtime::NativeInput,
    generation: u64,
    expected: Option<String>,
    at: Instant,
) -> Option<speakmore_core::writing::Context> {
    #[cfg(target_os = "macos")]
    {
        mac::writing_context(&input, generation, expected.as_deref(), at).ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (input, generation, expected, at);
        None
    }
}
