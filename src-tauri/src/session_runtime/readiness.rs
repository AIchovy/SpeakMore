use super::ReadinessAction;
use serde::Serialize;
use speakmore_core::error::AppError;

/// Stable presentation codes. Native readiness remains the sole start gate;
/// changing language or an unknown frontend code cannot enable capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessCode {
    Ready,
    ShortcutEditing,
    ShuttingDown,
    SystemSuspended,
    RecoveringInterruption,
    DictationBusy,
    StorageUnavailable,
    UpdatingConfiguration,
    PreparingModel,
    ReadingSettings,
    ModelFilesBusy,
    ModelServiceUnavailable,
    EngineUnavailable,
    EnginePreparing,
    ModelRequired,
    ModelConfigurationMismatch,
    MicrophonePromptPending,
    MicrophoneNotGranted,
    MicrophoneDenied,
    MicrophoneRestricted,
    MicrophoneStatusUnavailable,
    InputDeviceUnavailable,
    CheckingInputDevice,
}

impl ReadinessCode {
    pub(super) fn action(self) -> ReadinessAction {
        use ReadinessAction as A;
        use ReadinessCode::*;
        match self {
            Ready => A::None,
            StorageUnavailable => A::Storage,
            ModelServiceUnavailable
            | EngineUnavailable
            | ModelRequired
            | ModelConfigurationMismatch => A::Models,
            MicrophonePromptPending
            | MicrophoneNotGranted
            | MicrophoneDenied
            | MicrophoneRestricted
            | MicrophoneStatusUnavailable => A::Microphone,
            InputDeviceUnavailable | CheckingInputDevice => A::Device,
            ShortcutEditing
            | ShuttingDown
            | SystemSuspended
            | RecoveringInterruption
            | DictationBusy
            | UpdatingConfiguration
            | PreparingModel
            | ReadingSettings
            | ModelFilesBusy
            | EnginePreparing => A::Wait,
        }
    }
}

pub(super) struct Blocker {
    pub code: ReadinessCode,
    pub message: String,
    pub detail_code: Option<String>,
}
impl Blocker {
    pub fn new(code: ReadinessCode, message: &str) -> Self {
        Self {
            code,
            message: message.into(),
            detail_code: None,
        }
    }
    pub fn error(code: ReadinessCode, error: &AppError) -> Self {
        Self {
            code,
            message: error.user_message.clone(),
            detail_code: Some(error.code.clone()),
        }
    }
}
