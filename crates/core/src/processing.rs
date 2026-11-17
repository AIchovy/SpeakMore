//! Privacy changes revoke live attempts at the storage commit boundary. A later
//! opt-in applies to new sessions; it cannot revive an already cancelled request.
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, Weak};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// Compatibility with saved results from early builds; never emitted by new sessions.
    ProRequired,
    ProviderUnavailable,
    PolicyRevoked,
    Cancelled,
    Deadline,
    TextTooLong,
    NeedsReview,
    InvalidPrompt,
    NetworkUnavailable,
    ResponseRejected,
}
impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProRequired => "pro_required",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::PolicyRevoked => "policy_revoked",
            Self::Cancelled => "cancelled",
            Self::Deadline => "deadline",
            Self::TextTooLong => "text_too_long",
            Self::NeedsReview => "needs_review",
            Self::InvalidPrompt => "invalid_prompt",
            Self::NetworkUnavailable => "network_unavailable",
            Self::ResponseRejected => "response_rejected",
        }
    }
}

pub struct Watch {
    cloud: bool,
    cancel: CancellationToken,
}
impl Watch {
    pub fn token(&self) -> CancellationToken {
        self.cancel.clone()
    }
    pub fn revoked(&self) -> bool {
        self.cancel.is_cancelled()
    }
}
#[derive(Default)]
struct State {
    enabled: bool,
    cloud: bool,
    watches: Vec<Weak<Watch>>,
}
#[derive(Default)]
pub struct Policies(Mutex<State>);
impl Policies {
    pub fn watch(&self, cloud: bool) -> Arc<Watch> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let watch = Arc::new(Watch {
            cloud,
            cancel: CancellationToken::new(),
        });
        if !state.enabled || (cloud && !state.cloud) {
            watch.cancel.cancel();
        }
        state.watches.retain(|w| w.strong_count() > 0);
        state.watches.push(Arc::downgrade(&watch));
        watch
    }
    pub(crate) fn replace(&self, enabled: bool, cloud: bool) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.watches.retain(|weak| {
            let Some(watch) = weak.upgrade() else {
                return false;
            };
            if !enabled || (watch.cloud && !cloud) {
                watch.cancel.cancel();
            }
            true
        });
        state.enabled = enabled;
        state.cloud = cloud;
    }
}
