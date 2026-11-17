//! Public delivery reasons contain no target identifiers or surrounding text.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    ReadyForCopy,
    CopiedOnly,
    Submitted,
    Uncertain,
}
impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadyForCopy => "ready_for_copy",
            Self::CopiedOnly => "copied_only",
            Self::Submitted => "submitted",
            Self::Uncertain => "uncertain",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManualReason {
    MainWindow,
    UserRequested,
    AccessibilityRequired,
    UnsupportedTarget,
    SensitiveTarget,
    ReadOnlyTarget,
    TargetChanged,
    TargetUnavailable,
    ProbeTimeout,
    NeedsReview,
    TextTooLong,
    CompatibilityUnverified,
}
impl ManualReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MainWindow => "main_window",
            Self::UserRequested => "user_requested",
            Self::AccessibilityRequired => "accessibility_required",
            Self::UnsupportedTarget => "unsupported_target",
            Self::SensitiveTarget => "sensitive_target",
            Self::ReadOnlyTarget => "read_only_target",
            Self::TargetChanged => "target_changed",
            Self::TargetUnavailable => "target_unavailable",
            Self::ProbeTimeout => "probe_timeout",
            Self::NeedsReview => "needs_review",
            Self::TextTooLong => "text_too_long",
            Self::CompatibilityUnverified => "compatibility_unverified",
        }
    }
}
