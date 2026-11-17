//! Short-lived presentation receipts never extend the recording session itself.
use serde::Serialize;
use speakmore_core::error::AppError;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Notice {
    pub session_id: Uuid,
    pub has_result: bool,
    pub delivery_status: Option<String>,
    pub delivery_reason: Option<speakmore_core::delivery::ManualReason>,
    pub error: Option<AppError>,
}
#[derive(Default)]
pub(crate) struct Presentation {
    active: Option<Uuid>,
    notice: Option<Notice>,
    expires: Option<Instant>,
}
impl Presentation {
    pub(crate) fn sync(
        &mut self,
        active: Option<Uuid>,
        results: &[Uuid],
        error: Option<&AppError>,
        now: Instant,
    ) -> bool {
        if active.is_some() {
            self.active = active;
            self.clear();
            return false;
        }
        if let Some(id) = self.active.take() {
            let has_result = results.contains(&id);
            if has_result || error.is_some() {
                self.notice = Some(Notice {
                    session_id: id,
                    has_result,
                    delivery_status: None,
                    delivery_reason: None,
                    error: if has_result { None } else { error.cloned() },
                });
                self.expires = Some(now + Duration::from_secs(8));
                return true;
            }
        }
        if self.expires.is_some_and(|deadline| now >= deadline) {
            self.clear();
        }
        false
    }
    pub(crate) fn get(&self) -> Option<Notice> {
        self.notice.clone()
    }
    pub(crate) fn dismiss(&mut self, id: Uuid) {
        if self.notice.as_ref().is_some_and(|n| n.session_id == id) {
            self.clear();
        }
    }
    pub(crate) fn clear(&mut self) {
        self.notice = None;
        self.expires = None;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_receipt_timer_and_dismiss_cannot_hide_a_new_session_or_receipt() {
        let mut p = Presentation::default();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let at = Instant::now();
        p.sync(Some(a), &[], None, at);
        assert!(p.sync(None, &[a], None, at));
        assert_eq!(p.get().unwrap().session_id, a);
        p.sync(Some(b), &[a], None, at + Duration::from_secs(1));
        assert!(p.get().is_none());
        p.dismiss(a);
        p.sync(None, &[b], None, at + Duration::from_secs(4));
        p.sync(None, &[b], None, at + Duration::from_secs(8));
        p.dismiss(a);
        assert_eq!(p.get().unwrap().session_id, b);
        p.sync(None, &[b], None, at + Duration::from_secs(12));
        assert!(p.get().is_none());
    }
    #[test]
    fn cancellation_has_no_receipt_and_failure_never_claims_a_result() {
        let mut p = Presentation::default();
        let at = Instant::now();
        p.sync(Some(Uuid::new_v4()), &[], None, at);
        assert!(!p.sync(None, &[], None, at));
        assert!(p.get().is_none());
        p.sync(Some(Uuid::new_v4()), &[], None, at);
        assert!(p.sync(
            None,
            &[],
            Some(&AppError::new("DeviceUnavailable", "麦克风不可用。", true)),
            at
        ));
        assert!(!p.get().unwrap().has_result);
        let serialized = serde_json::to_value(p.get().unwrap()).unwrap();
        assert_eq!(serialized["error"]["code"], "DeviceUnavailable");
        assert_eq!(serialized["error"]["retryable"], true);
        assert!(serialized.get("message").is_none());
        let id = Uuid::new_v4();
        p.sync(Some(id), &[], None, at);
        p.sync(None, &[id], Some(&AppError::busy()), at);
        assert!(p.get().unwrap().has_result);
        assert!(p.get().unwrap().error.is_none());
    }
}
