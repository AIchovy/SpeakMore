use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};

pub const USER: u8 = 1;
pub const TIMEOUT: u8 = 2;
pub const SHUTDOWN: u8 = 3;

#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicU8>);
impl Cancellation {
    pub fn cancel(&self, reason: u8) {
        let _ = self
            .0
            .compare_exchange(0, reason, Ordering::AcqRel, Ordering::Acquire);
    }
    #[cfg(all(test, feature = "whisper"))]
    #[allow(
        dead_code,
        reason = "Shared module; only Whisper binary tests inspect callback ownership"
    )]
    pub fn owner_count(&self) -> usize {
        Arc::strong_count(&self.0)
    }
    pub fn reason(&self) -> u8 {
        self.0.load(Ordering::Acquire)
    }
    pub fn check(&self) -> Result<(), &'static str> {
        match self.reason() {
            0 => Ok(()),
            TIMEOUT => Err(speakmore_protocol::INFERENCE_TIMEOUT),
            _ => Err("Cancelled"),
        }
    }
}
