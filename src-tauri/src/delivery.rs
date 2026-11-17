//! One bounded automatic attempt. Text/target stay in memory; the actor persists
//! an uncertain intent before this task starts, and persists its final receipt.
#[cfg(test)]
pub(crate) mod fixture;
use crate::{
    clipboard::{Reservation, Stamp},
    session_runtime::NativeInput,
    target::Lease,
};
use speakmore_core::{
    delivery::{ManualReason, Status},
    error::{AppError, Result},
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

/// Emitted only after a durable history row changes. No transcript or target data.
#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeliveryUpdate {
    pub(crate) session_id: Uuid,
    pub(crate) delivery_status: Status,
    pub(crate) delivery_reason: Option<ManualReason>,
}

pub(crate) struct Attempt {
    pub(crate) session: Uuid,
    pub(crate) target: Arc<Lease>,
    policy: Arc<speakmore_core::app_rules::Watch>,
    native: NativeInput,
    epoch: u64,
    revision: u64,
    generation: u64,
    cancelled: AtomicBool,
    claimed: AtomicBool,
}
impl Attempt {
    pub(crate) fn new(
        session: Uuid,
        target: Arc<Lease>,
        policy: Arc<speakmore_core::app_rules::Watch>,
        native: NativeInput,
        epoch: u64,
        revision: u64,
        generation: u64,
    ) -> Arc<Self> {
        Arc::new(Self {
            session,
            target,
            policy,
            native,
            epoch,
            revision,
            generation,
            cancelled: AtomicBool::new(false),
            claimed: AtomicBool::new(false),
        })
    }
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub(crate) fn live(&self) -> bool {
        !self.cancelled.load(Ordering::Acquire)
            && !self.policy.manual_only()
            && self.target.qualified().is_ok()
            && self.native.permits_delivery(
                self.session,
                self.epoch,
                self.revision,
                self.generation,
            )
    }
    /// May block for one new target read when the last heartbeat is stale, so
    /// a claim landing between heartbeats is not turned into a copy-only result.
    pub(crate) fn confirm(&self) {
        if self.live() {
            self.target.refresh();
        }
    }
    pub(crate) fn claim(&self) -> bool {
        self.live()
            && self.target.fresh()
            && self.target.settled()
            && self
                .claimed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }
}
struct CancelOnDrop(Arc<Attempt>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
pub(crate) struct Outcome {
    pub(crate) status: Status,
    pub(crate) reason: Option<ManualReason>,
    pub(crate) error: Option<AppError>,
}
impl Outcome {
    pub(crate) fn ready(reason: Option<ManualReason>, error: Option<AppError>) -> Self {
        Self {
            status: Status::ReadyForCopy,
            reason,
            error,
        }
    }
}
pub(crate) enum Event {
    Claim {
        session: Uuid,
        reply: oneshot::Sender<bool>,
    },
    Finished {
        session: Uuid,
        outcome: Outcome,
    },
}
fn stopped() -> AppError {
    AppError::new("DeliveryStopped", "自动输入已停止，文字仍可复制。", false)
}
async fn recheck(attempt: &Attempt) -> std::result::Result<(), ManualReason> {
    if !attempt.live() {
        return Err(attempt
            .target
            .qualified()
            .err()
            .unwrap_or(ManualReason::TargetUnavailable));
    }
    let check = attempt.target.recheck()?;
    loop {
        if !attempt.live() {
            return Err(attempt
                .target
                .qualified()
                .err()
                .unwrap_or(ManualReason::TargetUnavailable));
        }
        if let Some(result) = check.poll() {
            result?;
            return attempt.target.qualified();
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
async fn fallback(
    reservation: &mut Reservation,
    stamp: Stamp,
    reason: Option<ManualReason>,
    error: Option<AppError>,
) -> Outcome {
    let status = match reservation.owns(stamp).await {
        Ok(true) => Status::CopiedOnly,
        Ok(false) => Status::ReadyForCopy,
        Err(_) => Status::Uncertain,
    };
    Outcome {
        status,
        reason,
        error,
    }
}
pub(crate) async fn run(
    mut reservation: Reservation,
    text: String,
    attempt: Arc<Attempt>,
    events: mpsc::Sender<Event>,
) {
    let _cancel = CancelOnDrop(attempt.clone());
    let outcome = perform(&mut reservation, text, &attempt, &events).await;
    // No operation after perform may inject or overwrite. Release the shared
    // writer slot before notifying the actor (a hung OS task retains its clone).
    drop(reservation);
    let _ = events
        .send(Event::Finished {
            session: attempt.session,
            outcome,
        })
        .await;
}
async fn perform(
    reservation: &mut Reservation,
    text: String,
    attempt: &Arc<Attempt>,
    events: &mpsc::Sender<Event>,
) -> Outcome {
    if text.is_empty()
        || text.len() > speakmore_core::MAX_TEXT_BYTES
        || text.contains('\0')
        || text.chars().count() > speakmore_core::MAX_AUTO_PASTE_CHARS
    {
        return Outcome::ready(Some(ManualReason::TextTooLong), None);
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        if !attempt.live() {
            return Outcome::ready(attempt.target.qualified().err(), Some(stopped()));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Outcome::ready(
                None,
                Some(AppError::new(
                    "ModifiersHeld",
                    "修饰键尚未释放，请手动粘贴。",
                    false,
                )),
            );
        }
        // A poll near the end of the window still gets a full reply budget: a
        // late reply is a held modifier, not an unreadable input state.
        match reservation
            .call(
                None,
                remaining.max(Duration::from_millis(100)),
                |writer, request| writer.modifiers(request),
            )
            .await
        {
            Ok(true) => break,
            Ok(false) => tokio::time::sleep(Duration::from_millis(10).min(remaining)).await,
            Err(error) => {
                return Outcome::ready(
                    (error.code == "AccessibilityRequired")
                        .then_some(ManualReason::AccessibilityRequired),
                    Some(AppError::new(
                        "InputStateUnavailable",
                        "无法确认按键状态或输入权限，未自动输入；文字仍可复制。",
                        false,
                    )),
                )
            }
        }
    }
    if let Err(reason) = recheck(attempt).await {
        return Outcome::ready(Some(reason), None);
    }
    let stamp = match reservation.write(text, attempt.clone()).await {
        Ok(stamp) => stamp,
        Err(error) => {
            return Outcome {
                status: if error.code == "CopyUncertain" {
                    Status::Uncertain
                } else {
                    Status::ReadyForCopy
                },
                reason: attempt.target.qualified().err(),
                error: Some(error),
            }
        }
    };
    if let Err(reason) = recheck(attempt).await {
        return fallback(reservation, stamp, Some(reason), None).await;
    }
    match reservation.owns(stamp.clone()).await {
        Ok(true) => {}
        Ok(false) => {
            return Outcome::ready(
                None,
                Some(AppError::new(
                    "ClipboardChanged",
                    "剪贴板已被更改，未提交粘贴。",
                    false,
                )),
            )
        }
        Err(error) => {
            return Outcome {
                status: Status::Uncertain,
                reason: None,
                error: Some(error),
            }
        }
    }
    let (reply, response) = oneshot::channel();
    let claimed = tokio::time::timeout(Duration::from_millis(300), async {
        events
            .send(Event::Claim {
                session: attempt.session,
                reply,
            })
            .await
            .ok()?;
        response.await.ok()
    })
    .await
    .ok()
    .flatten()
        == Some(true);
    if !claimed || !attempt.live() {
        return fallback(
            reservation,
            stamp,
            attempt.target.qualified().err(),
            Some(stopped()),
        )
        .await;
    }
    let owned = stamp.clone();
    let submitted: Result<Status> = reservation
        .call(
            Some(attempt.clone()),
            Duration::from_secs(3),
            move |writer, request| writer.submit(&owned, request),
        )
        .await;
    match submitted {
        Ok(status) => Outcome {
            status,
            reason: None,
            error: None,
        },
        Err(error) if error.code == "CopyUncertain" => Outcome {
            status: Status::Uncertain,
            reason: None,
            error: Some(error),
        },
        Err(error) => {
            let reason = if error.code == "AccessibilityRequired" {
                Some(ManualReason::AccessibilityRequired)
            } else {
                attempt.target.qualified().err()
            };
            fallback(reservation, stamp, reason, Some(error)).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture::*;
    async fn outcome(mode: u8, text: &str) -> (Outcome, Arc<State>) {
        let state = State::new(mode);
        let (targets, clipboard) = services(&state);
        let id = Uuid::new_v4();
        let target = Arc::new(targets.begin(id, 0, Instant::now()));
        until(|| target.qualified().is_ok()).await;
        let attempt = Attempt::new(
            id,
            target,
            speakmore_core::app_rules::Policies::default().watch(None),
            NativeInput::default(),
            0,
            0,
            0,
        );
        let reservation = clipboard.reserve().unwrap();
        assert_eq!(
            clipboard
                .copy("another explicit copy".into())
                .await
                .unwrap_err()
                .code,
            "ClipboardBusy"
        );
        let (events, mut received) = mpsc::channel(2);
        let work = tokio::spawn(run(reservation, text.into(), attempt.clone(), events));
        let answer = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match received.recv().await.unwrap() {
                    Event::Claim { session, reply } => {
                        assert_eq!(session, id);
                        let _ = reply.send(true);
                    }
                    Event::Finished { session, outcome } => {
                        assert_eq!(session, id);
                        break outcome;
                    }
                }
            }
        })
        .await
        .unwrap();
        work.await.unwrap();
        assert!(!attempt.claim());
        (answer, state)
    }
    #[tokio::test]
    async fn automatic_delivery_matrix_never_retries_or_confuses_copy_with_submission() {
        for (mode, expected, writes, posts) in [
            (NORMAL, Status::Submitted, 1, 1),
            (MODIFIERS, Status::ReadyForCopy, 0, 0),
            (TARGET_AFTER_WRITE, Status::CopiedOnly, 1, 0),
            (CLIPBOARD_CHANGED, Status::ReadyForCopy, 1, 0),
            (DISPATCH_UNCERTAIN, Status::Uncertain, 1, 1),
            (WRITE_UNCERTAIN, Status::Uncertain, 1, 0),
        ] {
            let (answer, state) = outcome(mode, "synthetic text").await;
            assert_eq!(answer.status, expected, "scenario {mode}");
            assert_eq!(state.writes.load(Ordering::SeqCst), writes);
            assert_eq!(state.posts.load(Ordering::SeqCst), posts);
            if mode == TARGET_AFTER_WRITE {
                assert_eq!(answer.reason, Some(ManualReason::TargetChanged));
            }
        }
        let (long, state) = outcome(NORMAL, &"a".repeat(4001)).await;
        assert_eq!(long.reason, Some(ManualReason::TextTooLong));
        assert_eq!(state.writes.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn cancelled_or_late_native_dispatch_keeps_slot_and_cannot_post() {
        let state = State::new(NORMAL);
        *state.blocked.lock().unwrap() = true;
        let (targets, clipboard) = services(&state);
        let id = Uuid::new_v4();
        let target = Arc::new(targets.begin(id, 0, Instant::now()));
        until(|| target.qualified().is_ok()).await;
        let attempt = Attempt::new(
            id,
            target,
            speakmore_core::app_rules::Policies::default().watch(None),
            NativeInput::default(),
            0,
            0,
            0,
        );
        let (events, mut received) = mpsc::channel(2);
        let work = tokio::spawn(run(
            clipboard.reserve().unwrap(),
            "fixture".into(),
            attempt.clone(),
            events,
        ));
        let Event::Claim { reply, .. } = received.recv().await.unwrap() else {
            panic!("expected claim")
        };
        reply.send(true).unwrap();
        until(|| state.entering_submit.load(Ordering::SeqCst) == 1).await;
        let Event::Finished { outcome, .. } = received.recv().await.unwrap() else {
            panic!("expected receipt")
        };
        assert_eq!(outcome.status, Status::Uncertain);
        work.await.unwrap();
        assert_eq!(
            clipboard.copy("next".into()).await.unwrap_err().code,
            "ClipboardBusy"
        );
        state.release();
        until(|| state.blocked.try_lock().is_ok()).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(state.posts.load(Ordering::SeqCst), 0);
        clipboard
            .copy("next explicit request".into())
            .await
            .unwrap();
    }
}
