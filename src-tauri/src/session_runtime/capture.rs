use speakmore_audio::{
    capture::{AudioService, CaptureEvent, CaptureHandle, Selection},
    error::Result,
};
use std::sync::mpsc::TryRecvError;
use uuid::Uuid;

pub(super) trait Capture: Send {
    fn stop(&self);
    fn cancel(&self);
    fn level(&self) -> f32;
    fn try_recv(&self) -> std::result::Result<CaptureEvent, TryRecvError>;
}
impl Capture for CaptureHandle {
    fn stop(&self) {
        self.stop()
    }
    fn cancel(&self) {
        self.cancel()
    }
    fn level(&self) -> f32 {
        self.level()
    }
    fn try_recv(&self) -> std::result::Result<CaptureEvent, TryRecvError> {
        self.try_recv()
    }
}
pub(super) trait Source: Send {
    fn permissions(&self) -> crate::permissions::Status;
    fn device_probe(&self, selected: Option<String>) -> Box<dyn FnOnce() -> Result<()> + Send>;
    fn busy(&self) -> bool;
    fn start(&self, id: Uuid, selection: Selection) -> Result<Box<dyn Capture>>;
}
impl Source for AudioService {
    fn permissions(&self) -> crate::permissions::Status {
        crate::permissions::read()
    }
    fn device_probe(&self, selected: Option<String>) -> Box<dyn FnOnce() -> Result<()> + Send> {
        Box::new(move || speakmore_audio::device::probe(selected.as_deref()).map(|_| ()))
    }
    fn busy(&self) -> bool {
        self.is_busy()
    }
    fn start(&self, id: Uuid, selection: Selection) -> Result<Box<dyn Capture>> {
        // A click, hotkey, or stale readiness snapshot must never implicitly
        // request microphone access. Check again immediately before capture.
        if self.permissions().microphone != crate::permissions::Microphone::Authorized {
            return Err(speakmore_audio::error::AudioError::PermissionDenied);
        }
        Ok(Box::new(self.start(id, selection)?))
    }
}
