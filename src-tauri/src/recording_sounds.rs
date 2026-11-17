//! Local, short earcons follow actual capture state, not repeated button events.
use objc2::{rc::Retained, AllocAnyThread};
use objc2_app_kit::NSSound;
use objc2_foundation::NSData;
use speakmore_core::session::Phase;
use std::cell::RefCell;
use uuid::Uuid;

#[derive(Debug, PartialEq, Eq)]
enum Cue {
    Start,
    Stop,
}
#[derive(Default)]
struct Transition {
    recording: Option<Uuid>,
}
impl Transition {
    fn update(&mut self, id: Option<Uuid>, phase: Phase) -> Option<Cue> {
        let next = id.filter(|_| phase == Phase::Recording);
        if next == self.recording {
            return None;
        }
        let previous = std::mem::replace(&mut self.recording, next);
        if next.is_some() {
            Some(Cue::Start)
        } else if previous.is_some() {
            Some(Cue::Stop)
        } else {
            None
        }
    }
}
struct Sounds {
    transition: Transition,
    start: Option<Retained<NSSound>>,
    stop: Option<Retained<NSSound>>,
}
fn sound(bytes: &[u8]) -> Option<Retained<NSSound>> {
    let sound = NSSound::initWithData(NSSound::alloc(), &NSData::with_bytes(bytes))?;
    sound.setVolume(0.55);
    Some(sound)
}
impl Default for Sounds {
    fn default() -> Self {
        Self {
            transition: Transition::default(),
            start: sound(include_bytes!("../sounds/start.wav")),
            stop: sound(include_bytes!("../sounds/stop.wav")),
        }
    }
}
thread_local! { static SOUNDS: RefCell<Sounds> = RefCell::new(Sounds::default()); }
pub(crate) fn sync(id: Option<Uuid>, phase: Phase) {
    SOUNDS.with(|sounds| {
        let mut sounds = sounds.borrow_mut();
        let cue = sounds.transition.update(id, phase);
        let audio = match cue {
            Some(Cue::Start) => &sounds.start,
            Some(Cue::Stop) => &sounds.stop,
            None => return,
        };
        if let Some(audio) = audio {
            audio.stop();
            audio.play();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cues_once_for_capture_start_stop_and_cancel_without_sounding_on_preparation() {
        let mut state = Transition::default();
        let id = Some(Uuid::new_v4());
        assert_eq!(state.update(id, Phase::Preparing), None);
        assert_eq!(state.update(id, Phase::Recording), Some(Cue::Start));
        assert_eq!(state.update(id, Phase::Recording), None);
        assert_eq!(state.update(id, Phase::Transcribing), Some(Cue::Stop));
        assert_eq!(state.update(None, Phase::Idle), None);
        assert_eq!(state.update(id, Phase::Recording), Some(Cue::Start));
        assert_eq!(state.update(id, Phase::Cancelling), Some(Cue::Stop));
        assert_eq!(state.update(None, Phase::Idle), None);
    }
}
