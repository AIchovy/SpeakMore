use crate::{
    error::{AppError, Result},
    MAX_RECORDING_MS,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Preparing,
    Recording,
    FinalizingAudio,
    Transcribing,
    Polishing,
    DeliveryCheck,
    Submitting,
    Cancelling,
    Finishing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartMode {
    Hold,
    Toggle,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub session_id: Option<Uuid>,
    pub revision: u64,
    pub phase: Phase,
    pub elapsed_ms: u64,
    pub max_duration_ms: u64,
    pub mode: Option<StartMode>,
}

#[derive(Debug)]
struct Active {
    id: Uuid,
    mode: StartMode,
    phase: Phase,
    recording_started_ms: Option<u64>,
    elapsed_ms: u64,
    target_eligible: bool,
    cancelled: bool,
    committed: bool,
}

/// Pure controller kernel. An actor owns this value; timestamps must be monotonic.
#[derive(Debug, Default)]
pub struct SessionController {
    active: Option<Active>,
    revision: u64,
    requests: VecDeque<(Uuid, Uuid, u64)>,
    hold_needs_release: bool,
}

impl SessionController {
    pub fn snapshot(&self, now_ms: u64) -> SessionSnapshot {
        let elapsed = self
            .active
            .as_ref()
            .map(|s| {
                if s.phase == Phase::Recording {
                    now_ms
                        .saturating_sub(s.recording_started_ms.unwrap_or(now_ms))
                        .min(MAX_RECORDING_MS)
                } else {
                    s.elapsed_ms
                }
            })
            .unwrap_or(0);
        SessionSnapshot {
            session_id: self.active.as_ref().map(|s| s.id),
            revision: self.revision,
            phase: self.active.as_ref().map(|s| s.phase).unwrap_or(Phase::Idle),
            elapsed_ms: elapsed,
            max_duration_ms: MAX_RECORDING_MS,
            mode: self.active.as_ref().map(|s| s.mode),
        }
    }

    pub fn start(
        &mut self,
        request_id: Uuid,
        mode: StartMode,
        now_ms: u64,
        ready: bool,
        eligible: bool,
    ) -> Result<Uuid> {
        self.requests
            .retain(|(_, _, created)| now_ms.saturating_sub(*created) < 600_000);
        if let Some((_, id, _)) = self
            .requests
            .iter()
            .find(|(request, _, _)| *request == request_id)
        {
            return Ok(*id);
        }
        if self.active.is_some() {
            return Err(AppError::busy());
        }
        if mode == StartMode::Hold && self.hold_needs_release {
            return Err(AppError::new(
                "KeyReleaseRequired",
                "请先松开快捷键。",
                true,
            ));
        }
        if !ready {
            return Err(AppError::new("NotReady", "设备或模型尚未就绪。", true));
        }
        let id = Uuid::new_v4();
        self.active = Some(Active {
            id,
            mode,
            phase: Phase::Preparing,
            recording_started_ms: None,
            elapsed_ms: 0,
            target_eligible: eligible,
            cancelled: false,
            committed: false,
        });
        self.requests.push_back((request_id, id, now_ms));
        if self.requests.len() > 128 {
            self.requests.pop_front();
        }
        self.revision += 1;
        Ok(id)
    }

    pub fn mic_ready(&mut self, id: Uuid, now_ms: u64) -> bool {
        let Some(s) = self
            .active
            .as_mut()
            .filter(|s| s.id == id && s.phase == Phase::Preparing && !s.cancelled)
        else {
            return false;
        };
        s.phase = Phase::Recording;
        s.recording_started_ms = Some(now_ms);
        self.revision += 1;
        true
    }

    pub fn release(&mut self, now_ms: u64) -> bool {
        self.key_released();
        let Some(id) = self
            .active
            .as_ref()
            .filter(|s| s.mode == StartMode::Hold)
            .map(|s| s.id)
        else {
            return false;
        };
        self.stop(id, now_ms)
    }

    /// The native actor stops by session identity before rearming the key.
    pub fn key_released(&mut self) {
        self.hold_needs_release = false;
    }

    pub fn stop(&mut self, id: Uuid, now_ms: u64) -> bool {
        let Some(s) = self.active.as_mut().filter(|s| s.id == id) else {
            return false;
        };
        match s.phase {
            Phase::Preparing => {
                s.phase = Phase::Finishing;
                s.cancelled = true;
            }
            Phase::Recording => {
                s.elapsed_ms = now_ms
                    .saturating_sub(s.recording_started_ms.unwrap_or(now_ms))
                    .min(MAX_RECORDING_MS);
                s.phase = Phase::FinalizingAudio;
            }
            _ => return false,
        }
        self.revision += 1;
        true
    }

    pub fn tick(&mut self, now_ms: u64) -> bool {
        let Some(s) = self.active.as_ref().filter(|s| s.phase == Phase::Recording) else {
            return false;
        };
        if now_ms.saturating_sub(s.recording_started_ms.unwrap_or(now_ms)) < MAX_RECORDING_MS {
            return false;
        }
        let (id, mode) = (s.id, s.mode);
        if mode == StartMode::Hold {
            self.hold_needs_release = true;
        }
        self.stop(id, now_ms)
    }

    pub fn advance(&mut self, id: Uuid, next: Phase) -> bool {
        let Some(s) = self.active.as_mut().filter(|s| s.id == id) else {
            return false;
        };
        let allowed = matches!(
            (s.phase, next),
            (Phase::FinalizingAudio, Phase::Transcribing)
                | (Phase::FinalizingAudio, Phase::Finishing)
                | (Phase::Transcribing, Phase::Polishing)
                | (Phase::Transcribing, Phase::DeliveryCheck)
                | (Phase::Transcribing, Phase::Finishing)
                | (Phase::Polishing, Phase::DeliveryCheck)
                | (Phase::DeliveryCheck, Phase::Finishing)
                | (Phase::Submitting, Phase::Finishing)
                | (Phase::Cancelling, Phase::Finishing)
        );
        if !allowed || (s.cancelled && next != Phase::Finishing) {
            return false;
        }
        s.phase = next;
        self.revision += 1;
        true
    }

    pub fn invalidate_target(&mut self, id: Uuid) {
        if let Some(s) = self
            .active
            .as_mut()
            .filter(|s| s.id == id && s.target_eligible)
        {
            s.target_eligible = false;
            self.revision += 1;
        }
    }

    /// Call immediately before the one OS dispatch, only after platform revalidation.
    pub fn claim_submission(&mut self, id: Uuid, platform_eligible: bool) -> bool {
        let Some(s) = self.active.as_mut().filter(|s| s.id == id) else {
            return false;
        };
        if s.phase != Phase::DeliveryCheck
            || s.cancelled
            || s.committed
            || !s.target_eligible
            || !platform_eligible
        {
            return false;
        }
        s.committed = true;
        s.phase = Phase::Submitting;
        self.revision += 1;
        true
    }

    pub fn cancel(&mut self, id: Uuid) -> bool {
        let Some(s) = self
            .active
            .as_mut()
            .filter(|s| s.id == id && !s.cancelled && !s.committed && s.phase != Phase::Finishing)
        else {
            return false;
        };
        s.cancelled = true;
        s.target_eligible = false;
        s.phase = Phase::Cancelling;
        if s.mode == StartMode::Hold {
            self.hold_needs_release = true;
        }
        self.revision += 1;
        true
    }

    pub fn finish(&mut self, id: Uuid) -> bool {
        if !self
            .active
            .as_ref()
            .is_some_and(|s| s.id == id && s.phase == Phase::Finishing)
        {
            return false;
        }
        self.active = None;
        self.revision += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn begin(c: &mut SessionController, mode: StartMode) -> Uuid {
        let id = c.start(Uuid::new_v4(), mode, 0, true, true).unwrap();
        assert!(c.mic_ready(id, 10));
        id
    }
    #[test]
    fn duplicate_start_and_busy_do_not_create_second_session() {
        let mut c = SessionController::default();
        let request = Uuid::new_v4();
        let id = c.start(request, StartMode::Hold, 0, true, true).unwrap();
        for _ in 0..20 {
            assert_eq!(
                c.start(request, StartMode::Hold, 1, true, true).unwrap(),
                id
            );
        }
        assert_eq!(
            c.start(Uuid::new_v4(), StartMode::Hold, 2, true, true)
                .unwrap_err()
                .code,
            "Busy"
        );
    }
    #[test]
    fn release_during_preparation_prevents_late_microphone() {
        let mut c = SessionController::default();
        let id = c
            .start(Uuid::new_v4(), StartMode::Hold, 0, true, true)
            .unwrap();
        assert!(c.release(1));
        assert!(!c.mic_ready(id, 2));
        assert!(c.finish(id));
        assert_eq!(c.snapshot(3).phase, Phase::Idle);
    }
    #[test]
    fn five_minute_boundary_and_repeat_stop_are_exact() {
        let mut c = SessionController::default();
        let id = begin(&mut c, StartMode::Hold);
        assert!(!c.tick(300_009));
        assert!(c.tick(300_010));
        assert!(!c.tick(300_011));
        assert!(!c.stop(id, 300_012));
        assert_eq!(c.snapshot(300_013).elapsed_ms, 300_000);
        assert!(c.advance(id, Phase::Finishing));
        assert!(c.finish(id));
        assert_eq!(
            c.start(Uuid::new_v4(), StartMode::Hold, 300_014, true, true)
                .unwrap_err()
                .code,
            "KeyReleaseRequired"
        );
        c.release(300_015);
        assert!(c
            .start(Uuid::new_v4(), StartMode::Hold, 300_016, true, true)
            .is_ok());
    }
    #[test]
    fn toggle_limit_does_not_require_key_release() {
        let mut c = SessionController::default();
        let id = begin(&mut c, StartMode::Toggle);
        assert!(c.tick(300_010));
        assert!(c.advance(id, Phase::Finishing));
        assert!(c.finish(id));
        assert!(c
            .start(Uuid::new_v4(), StartMode::Toggle, 300_020, true, true)
            .is_ok());
    }
    #[test]
    fn cancellation_and_old_callback_cannot_resurrect_session() {
        let mut c = SessionController::default();
        let id = begin(&mut c, StartMode::Toggle);
        assert!(c.cancel(id));
        assert!(!c.cancel(id));
        assert!(!c.advance(id, Phase::DeliveryCheck));
        assert!(c.advance(id, Phase::Finishing));
        assert!(c.finish(id));
        let next = begin(&mut c, StartMode::Toggle);
        assert!(!c.mic_ready(id, 20));
        assert!(!c.claim_submission(id, true));
        assert_eq!(c.snapshot(20).session_id, Some(next));
    }
    #[test]
    fn target_invalidation_is_permanent_and_submission_is_once() {
        for invalid in [false, true] {
            let mut c = SessionController::default();
            let id = begin(&mut c, StartMode::Toggle);
            c.stop(id, 1000);
            c.advance(id, Phase::Transcribing);
            c.advance(id, Phase::DeliveryCheck);
            if invalid {
                c.invalidate_target(id);
            }
            assert_eq!(c.claim_submission(id, true), !invalid);
            assert!(!c.claim_submission(id, true));
        }
    }
}
