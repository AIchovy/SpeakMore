//! SenseVoice's native boundary. One owner, no shared decoder or borrowed C results.
//! Model trust and whole-session VAD/cancellation remain the worker's responsibility.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod native;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub use native::*;
