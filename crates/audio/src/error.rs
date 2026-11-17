use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error, Serialize)]
pub enum AudioError {
    #[error("AudioBusy")]
    Busy,
    #[error("NoInputDevice")]
    NoInputDevice,
    #[error("DeviceUnavailable")]
    DeviceUnavailable,
    #[error("UnsupportedAudioFormat")]
    UnsupportedAudioFormat,
    #[error("InvalidChannel")]
    InvalidChannel,
    #[error("MicrophonePermissionDenied")]
    PermissionDenied,
    #[error("DeviceBusy")]
    DeviceBusy,
    #[error("MicrophonePrepareTimeout")]
    PrepareTimeout,
    #[error("AudioIncomplete")]
    Incomplete,
    #[error("InvalidAudioSamples")]
    InvalidSamples,
    #[error("AudioTooShort")]
    TooShort,
    #[error("AudioDrainTimeout")]
    DrainTimeout,
    #[error("AudioResamplingFailed")]
    ResamplingFailed,
    #[error("AudioMemoryUnavailable")]
    MemoryUnavailable,
    #[error("AudioThreadUnavailable")]
    ThreadUnavailable,
    #[error("Cancelled")]
    Cancelled,
}
pub type Result<T> = std::result::Result<T, AudioError>;
