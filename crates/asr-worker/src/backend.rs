//! Fail closed if whisper.cpp silently chooses CPU while GPU was requested.
//! The locked 1.8.3 engine exposes selection through these diagnostics but no
//! public context backend getter. Keep only three atomic bits, never log text.
use std::{
    ffi::{c_char, c_uint, c_void, CStr},
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};
use whisper_rs::{WhisperContext, WhisperState};

static ARMED: AtomicBool = AtomicBool::new(false);
static OBSERVED: AtomicU8 = AtomicU8::new(0);
const SELECTED: u8 = 1;
const FAILED: u8 = 2;
fn diagnostic(text: &[u8]) -> u8 {
    if text.starts_with(b"whisper_backend_init_gpu: using ") {
        SELECTED
    } else if text.starts_with(b"whisper_backend_init_gpu: failed to initialize ")
        || text.starts_with(b"whisper_backend_init_gpu: no GPU found")
    {
        FAILED
    } else {
        0
    }
}
unsafe extern "C" fn observe(_: c_uint, text: *const c_char, _: *mut c_void) {
    if text.is_null() || !ARMED.load(Ordering::Acquire) {
        return;
    }
    // whisper.cpp owns a valid NUL-terminated string throughout this callback.
    // No conversion, allocation, file output or retained model/device/path text.
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    OBSERVED.fetch_or(diagnostic(bytes), Ordering::Relaxed);
}
pub fn install() {
    whisper_rs::install_logging_hooks();
    // Process-lifetime function pointer with no borrowed user data, and no panic.
    unsafe {
        whisper_rs::set_log_callback(Some(observe), std::ptr::null_mut());
    }
}
struct Probe;
impl Drop for Probe {
    fn drop(&mut self) {
        ARMED.store(false, Ordering::Release);
    }
}
pub fn create_state(context: &WhisperContext, gpu: bool) -> Result<WhisperState, &'static str> {
    if !gpu {
        return context.create_state().map_err(|_| "StateCreationFailed");
    }
    OBSERVED.store(0, Ordering::Release);
    ARMED.store(true, Ordering::Release);
    let probe = Probe;
    let state = context.create_state().map_err(|_| "GpuUnavailable");
    drop(probe);
    if OBSERVED.load(Ordering::Acquire) != SELECTED {
        return Err("GpuUnavailable");
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_fixed_backend_markers_count_and_missing_markers_fail_closed() {
        assert_eq!(
            diagnostic(b"whisper_backend_init_gpu: using Metal backend\n"),
            SELECTED
        );
        assert_eq!(
            diagnostic(b"whisper_backend_init_gpu: failed to initialize Metal backend\n"),
            FAILED
        );
        assert_eq!(
            diagnostic(b"whisper_backend_init_gpu: no GPU found\n"),
            FAILED
        );
        assert_eq!(diagnostic(b"whisper_model_load: private/path/model.bin"), 0);
        assert_eq!(diagnostic(b"transcript contents"), 0);
        assert_eq!(diagnostic(b"whisper_backend_init: using CPU backend"), 0);
        assert_ne!(SELECTED | FAILED, SELECTED);
    }
}
