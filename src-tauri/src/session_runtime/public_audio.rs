//! Explicit development build only. Fixed public 300-second PCM fixture;
//! simulated capture/permissions, real actor, IPC, VAD and ASR. No microphone.
use super::capture;
use sha2::{Digest, Sha256};
use speakmore_audio::{
    capture::{CaptureEvent, Diagnostics, Selection},
    device::{Configuration, Format},
    dsp::ProcessedAudio,
    error::{AudioError, Result},
};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc::TryRecvError,
        Arc,
    },
    time::Instant,
};
use uuid::Uuid;

const WAV: &[u8] = include_bytes!("../../../.cache/sensevoice/model/test_wavs/en.wav");
const SHA: &str = "eb1eb008904465b74c304aad8342e8c7d3c6e61ffe9f66adcaca9cf0f76a93f4";
const SAMPLES: usize = 4_800_000;

fn speech() -> Result<Vec<f32>> {
    if format!("{:x}", Sha256::digest(WAV)) != SHA {
        return Err(AudioError::InvalidSamples);
    }
    let mut reader = hound::WavReader::new(WAV).map_err(|_| AudioError::InvalidSamples)?;
    let spec = reader.spec();
    if (spec.sample_rate, spec.channels, spec.bits_per_sample) != (16_000, 1, 16)
        || spec.sample_format != hound::SampleFormat::Int
    {
        return Err(AudioError::InvalidSamples);
    }
    let mut phrase = reader
        .samples::<i16>()
        .map(|s| s.map(|v| f32::from(v) / 32768.0))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| AudioError::InvalidSamples)?;
    if phrase.is_empty() {
        return Err(AudioError::InvalidSamples);
    }
    phrase.extend([0.0; 16_000]);
    let mut samples = speakmore_audio::dsp::allocated_zeros(SAMPLES)?;
    for (target, value) in samples.iter_mut().zip(phrase.iter().copied().cycle()) {
        *target = value;
    }
    Ok(samples)
}

#[derive(Default)]
pub(super) struct Source(Arc<AtomicBool>);
struct Capture {
    busy: Arc<AtomicBool>,
    finished: AtomicBool,
    id: Uuid,
    intent: AtomicU8,
    ready: AtomicBool,
}
impl capture::Source for Source {
    fn permissions(&self) -> crate::permissions::Status {
        // Explicitly simulated, never a claim about macOS TCC. The build's
        // window title and persistent banner identify this substitution.
        crate::permissions::Status {
            microphone: crate::permissions::Microphone::Authorized,
            accessibility: false,
            requesting_microphone: false,
        }
    }
    fn device_probe(&self, _: Option<String>) -> Box<dyn FnOnce() -> Result<()> + Send> {
        Box::new(|| Ok(()))
    }
    fn busy(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    fn start(&self, id: Uuid, _: Selection) -> Result<Box<dyn capture::Capture>> {
        if self.0.swap(true, Ordering::AcqRel) {
            return Err(AudioError::Busy);
        }
        // Opening runs under the NativeInput lock; decode only on finish.
        Ok(Box::new(Capture {
            busy: self.0.clone(),
            finished: AtomicBool::new(false),
            id,
            intent: AtomicU8::new(0),
            ready: AtomicBool::new(false),
        }))
    }
}
impl capture::Capture for Capture {
    fn stop(&self) {
        let _ = self
            .intent
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire);
    }
    fn cancel(&self) {
        self.intent.store(2, Ordering::Release);
    }
    fn level(&self) -> f32 {
        0.0
    }
    fn try_recv(&self) -> std::result::Result<CaptureEvent, TryRecvError> {
        let intent = self.intent.load(Ordering::Acquire);
        if intent != 0 && !self.finished.swap(true, Ordering::AcqRel) {
            let result = if intent == 2 {
                Err(AudioError::Cancelled)
            } else {
                speech().map(|samples| ProcessedAudio {
                    samples,
                    input_frames: SAMPLES as u64,
                    input_rate: 16_000,
                    duration_ms: 300_000,
                    reached_limit: true,
                })
            };
            self.busy.store(false, Ordering::Release);
            return Ok(CaptureEvent::Finished {
                session_id: self.id,
                result,
                diagnostics: Diagnostics {
                    accepted_frames: SAMPLES as u64,
                    clipped_samples: 0,
                    scheduling_fallback: false,
                },
            });
        }
        if !self.ready.swap(true, Ordering::AcqRel) {
            return Ok(CaptureEvent::Ready {
                session_id: self.id,
                device_id: "public-audio-fixture-no-microphone".into(),
                configuration: Configuration {
                    sample_rate: 16_000,
                    channels: 1,
                    sample_format: Format::F32,
                },
                started_at: Instant::now(),
            });
        }
        Err(TryRecvError::Empty)
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.busy.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::capture::Source as _;
    use super::*;

    #[test]
    fn public_pcm_keeps_internal_gaps_and_exact_duration() {
        let samples = speech().unwrap();
        assert_eq!(samples.len(), SAMPLES);
        assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        let reader = hound::WavReader::new(WAV).unwrap();
        let end = reader.duration() as usize;
        assert!(samples[end..end + 16_000].iter().all(|s| *s == 0.0));
        assert_eq!(&samples[..end], &samples[end + 16_000..end * 2 + 16_000]);
    }

    #[test]
    fn cancelling_fixture_does_not_emit_audio_and_releases_source() {
        let source = Source::default();
        let capture = source.start(Uuid::new_v4(), Selection::default()).unwrap();
        assert!(source.busy());
        assert!(matches!(capture.try_recv(), Ok(CaptureEvent::Ready { .. })));
        capture.cancel();
        assert!(matches!(
            capture.try_recv(),
            Ok(CaptureEvent::Finished {
                result: Err(AudioError::Cancelled),
                ..
            })
        ));
        assert!(!source.busy());
        assert!(matches!(capture.try_recv(), Err(TryRecvError::Empty)));
        drop(capture);
        let capture = source.start(Uuid::new_v4(), Selection::default()).unwrap();
        assert!(matches!(capture.try_recv(), Ok(CaptureEvent::Ready { .. })));
        assert!(matches!(
            source.start(Uuid::new_v4(), Selection::default()),
            Err(AudioError::Busy)
        ));
        capture.stop();
        let Ok(CaptureEvent::Finished {
            result: Ok(audio), ..
        }) = capture.try_recv()
        else {
            panic!("expected one complete public fixture");
        };
        assert_eq!(audio.samples.len(), SAMPLES);
        assert_eq!(audio.duration_ms, 300_000);
        assert!(audio.reached_limit);
        assert!(matches!(capture.try_recv(), Err(TryRecvError::Empty)));
    }
}
