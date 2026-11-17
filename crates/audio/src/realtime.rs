//! The application-controlled portion of the CPAL callback: only normalization,
//! ring writes, monotonic timestamps and atomics. No locks, allocation or I/O.
use crate::{
    device::Configuration,
    error::{AudioError, Result},
    MAX_SECONDS,
};
use ringbuf::{
    traits::{Consumer, Producer, Split},
    HeapCons, HeapProd, HeapRb,
};
use std::{
    sync::{
        atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering},
        Arc,
    },
    time::Instant,
};

pub(crate) const RUN: u8 = 0;
pub(crate) const STOP: u8 = 1;
pub(crate) const CANCEL: u8 = 2;
pub(crate) const LIMIT: u8 = 3;
pub(crate) const BAD_SAMPLE: u8 = 1;
pub(crate) const INCOMPLETE: u8 = 2;
pub(crate) const DEVICE_LOST: u8 = 3;
pub(crate) const PERMISSION: u8 = 4;

pub(crate) struct Signals {
    pub origin: Instant,
    pub intent: AtomicU8,
    pub fault: AtomicU8,
    pub paused: AtomicU8,
    pub ready_ns: AtomicU64,
    pub last_data_ns: AtomicU64,
    pub accepted_frames: AtomicU64,
    pub clipped_samples: AtomicU64,
    pub rms_bits: AtomicU32,
    pub scheduling_fallback: AtomicU8,
}
impl Signals {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            intent: AtomicU8::new(RUN),
            fault: AtomicU8::new(0),
            paused: AtomicU8::new(0),
            ready_ns: AtomicU64::new(0),
            last_data_ns: AtomicU64::new(0),
            accepted_frames: AtomicU64::new(0),
            clipped_samples: AtomicU64::new(0),
            rms_bits: AtomicU32::new(0),
            scheduling_fallback: AtomicU8::new(0),
        }
    }
    pub fn fail(&self, code: u8) {
        let _ = self
            .fault
            .compare_exchange(0, code, Ordering::AcqRel, Ordering::Acquire);
    }
    pub fn error(&self) -> Result<()> {
        match self.fault.load(Ordering::Acquire) {
            0 => Ok(()),
            BAD_SAMPLE => Err(AudioError::InvalidSamples),
            DEVICE_LOST => Err(AudioError::DeviceUnavailable),
            PERMISSION => Err(AudioError::PermissionDenied),
            _ => Err(AudioError::Incomplete),
        }
    }
    pub fn elapsed_ms(&self) -> u64 {
        let ready = self.ready_ns.load(Ordering::Acquire);
        if ready == 0 {
            0
        } else {
            ((self.origin.elapsed().as_nanos() as u64).saturating_sub(ready) / 1_000_000)
                .min(300000)
        }
    }
}

pub(crate) trait InputSample: Copy + cpal::SizedSample {
    fn normalized(self) -> f32;
}
impl InputSample for f32 {
    fn normalized(self) -> f32 {
        self
    }
}
impl InputSample for i16 {
    fn normalized(self) -> f32 {
        f32::from(self) / 32768.0
    }
}
impl InputSample for u16 {
    fn normalized(self) -> f32 {
        (f32::from(self) - 32768.0) / 32768.0
    }
}

pub(crate) struct CallbackWriter {
    producer: HeapProd<f32>,
    signals: Arc<Signals>,
    channels: usize,
    frames: u64,
    max_frames: Option<u64>,
    wall_limit_ns: Option<u64>,
}
impl CallbackWriter {
    pub fn new(config: Configuration, signals: Arc<Signals>) -> Result<(Self, HeapCons<f32>)> {
        Self::with_limit(config, signals, MAX_SECONDS)
    }
    pub fn with_limit(
        config: Configuration,
        signals: Arc<Signals>,
        seconds: u32,
    ) -> Result<(Self, HeapCons<f32>)> {
        Self::configure(config, signals, Some(seconds))
    }
    pub fn continuous(
        config: Configuration,
        signals: Arc<Signals>,
    ) -> Result<(Self, HeapCons<f32>)> {
        Self::configure(config, signals, None)
    }
    fn configure(
        config: Configuration,
        signals: Arc<Signals>,
        seconds: Option<u32>,
    ) -> Result<(Self, HeapCons<f32>)> {
        let capacity = config.sample_rate as usize * usize::from(config.channels) * 2;
        // Fallible allocation and page touching happen before opening the stream.
        let ring = HeapRb::<f32>::from(crate::dsp::allocated_zeros(capacity)?);
        let (producer, mut consumer) = ring.split();
        consumer.clear();
        Ok((
            Self {
                producer,
                signals,
                channels: usize::from(config.channels),
                frames: 0,
                max_frames: seconds
                    .map(|seconds| u64::from(config.sample_rate) * u64::from(seconds)),
                wall_limit_ns: seconds
                    .filter(|seconds| *seconds == MAX_SECONDS)
                    .map(|seconds| u64::from(seconds) * 1_000_000_000),
            },
            consumer,
        ))
    }
    pub fn write<T: InputSample>(&mut self, input: &[T]) {
        if self.signals.paused.load(Ordering::Acquire) != 0 {
            return;
        }
        if self.signals.intent.load(Ordering::Acquire) != RUN
            || self.signals.fault.load(Ordering::Acquire) != 0
            || input.is_empty()
        {
            return;
        }
        if !input.len().is_multiple_of(self.channels) {
            self.signals.fail(INCOMPLETE);
            return;
        }
        let now_ns = (self.signals.origin.elapsed().as_nanos() as u64).max(1);
        let ready = self.signals.ready_ns.load(Ordering::Acquire);
        if ready != 0
            && self
                .wall_limit_ns
                .is_some_and(|limit| now_ns.saturating_sub(ready) >= limit)
        {
            let _ = self.signals.intent.compare_exchange(
                RUN,
                LIMIT,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            return;
        }
        let input_frames = input.len() / self.channels;
        let frames = self.max_frames.map_or(input_frames, |limit| {
            input_frames.min((limit - self.frames) as usize)
        });
        let mut clipped = 0;
        for frame in input.chunks_exact(self.channels).take(frames) {
            // Observe cancel between frames, even for an unusually large callback.
            if self.signals.intent.load(Ordering::Acquire) != RUN {
                break;
            }
            for &raw in frame {
                let value = raw.normalized();
                if !value.is_finite() {
                    self.signals.fail(BAD_SAMPLE);
                    return;
                }
                if value.abs() > 1.0 {
                    clipped += 1;
                }
                if self.producer.try_push(value.clamp(-1.0, 1.0)).is_err() {
                    self.signals.fail(INCOMPLETE);
                    return;
                }
            }
            self.frames += 1;
        }
        if self.frames != 0 {
            self.signals
                .accepted_frames
                .store(self.frames, Ordering::Release);
            self.signals
                .clipped_samples
                .fetch_add(clipped, Ordering::Relaxed);
            self.signals.last_data_ns.store(now_ns, Ordering::Release);
            // Ready means real, complete frames have arrived, not merely play().
            let _ = self.signals.ready_ns.compare_exchange(
                0,
                now_ns,
                Ordering::Release,
                Ordering::Relaxed,
            );
        }
        if self.max_frames == Some(self.frames) {
            let _ = self.signals.intent.compare_exchange(
                RUN,
                LIMIT,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::traits::Observer;
    fn config(channels: u16) -> Configuration {
        Configuration {
            sample_rate: 16000,
            channels,
            sample_format: crate::device::Format::F32,
        }
    }
    #[test]
    fn native_sample_formats_are_normalized_and_clipped() {
        let signals = Arc::new(Signals::new());
        let (mut writer, mut consumer) = CallbackWriter::new(config(1), signals.clone()).unwrap();
        writer.write(&[i16::MIN, 0i16, i16::MAX]);
        writer.write(&[0u16, 32768u16, u16::MAX]);
        writer.write(&[-2.0f32, 2.0]);
        let mut output = [0.0; 8];
        assert_eq!(consumer.pop_slice(&mut output), 8);
        assert_eq!(
            output,
            [
                -1.0,
                0.0,
                32767.0 / 32768.0,
                -1.0,
                0.0,
                32767.0 / 32768.0,
                -1.0,
                1.0
            ]
        );
        assert_eq!(signals.clipped_samples.load(Ordering::Relaxed), 2);
    }
    #[test]
    fn overflow_nonfinite_and_incomplete_frames_poison_recording() {
        for invalid in [vec![f32::NAN, 0.0], vec![0.0], vec![0.0; 64002]] {
            let signals = Arc::new(Signals::new());
            let (mut writer, consumer) = CallbackWriter::new(config(2), signals.clone()).unwrap();
            writer.write(&invalid);
            assert!(signals.error().is_err());
            let before = consumer.occupied_len();
            writer.write(&[0.2, 0.3]);
            assert_eq!(consumer.occupied_len(), before);
        }
    }
    #[test]
    fn cancellation_before_first_frame_never_becomes_ready() {
        let signals = Arc::new(Signals::new());
        signals.intent.store(CANCEL, Ordering::Release);
        let (mut writer, consumer) = CallbackWriter::new(config(1), signals.clone()).unwrap();
        writer.write(&[1.0]);
        assert_eq!(consumer.occupied_len(), 0);
        assert_eq!(signals.ready_ns.load(Ordering::Acquire), 0);
    }
    #[test]
    fn callback_does_not_allocate_for_any_supported_sample_type() {
        let signals = Arc::new(Signals::new());
        let (mut writer, consumer) = CallbackWriter::new(config(2), signals).unwrap();
        let floats = [0.2f32; 2048];
        let signed = [42i16; 2048];
        let unsigned = [32768u16; 2048];
        let allocations = crate::allocation_test::allocations_during(|| {
            writer.write(&floats);
            writer.write(&signed);
            writer.write(&unsigned);
        });
        assert_eq!(allocations, 0);
        assert_eq!(consumer.occupied_len(), 6144);
    }
    #[test]
    fn final_callback_trims_to_sample_limit_and_requires_new_capture() {
        let signals = Arc::new(Signals::new());
        let (mut writer, mut consumer) = CallbackWriter::new(config(2), signals.clone()).unwrap();
        writer.frames = writer.max_frames.unwrap() - 1;
        writer.write(&[0.1, 0.2, 0.3, 0.4]);
        assert_eq!(consumer.occupied_len(), 2);
        assert_eq!(signals.intent.load(Ordering::Acquire), LIMIT);
        writer.write(&[0.5, 0.6]);
        assert_eq!(consumer.try_pop(), Some(0.1));
        assert_eq!(consumer.try_pop(), Some(0.2));
        assert_eq!(consumer.try_pop(), None);
    }
    #[test]
    fn monotonic_limit_stops_even_when_sample_counter_is_short() {
        let mut state = Signals::new();
        state.origin = Instant::now() - std::time::Duration::from_secs(301);
        state.ready_ns.store(1, Ordering::Release);
        let signals = Arc::new(state);
        let (mut writer, consumer) = CallbackWriter::new(config(1), signals.clone()).unwrap();
        writer.write(&[0.5]);
        assert_eq!(signals.intent.load(Ordering::Acquire), LIMIT);
        assert_eq!(consumer.occupied_len(), 0);
        assert_eq!(signals.elapsed_ms(), 300000);
    }
    #[test]
    fn meeting_clock_does_not_inherit_five_minute_limit_and_pause_drops_frames() {
        let mut state = Signals::new();
        state.origin = Instant::now() - std::time::Duration::from_secs(601);
        state.ready_ns.store(1, Ordering::Release);
        let signals = Arc::new(state);
        let (mut writer, mut consumer) =
            CallbackWriter::with_limit(config(1), signals.clone(), 3600).unwrap();
        writer.write(&[0.5]);
        assert_eq!(signals.intent.load(Ordering::Acquire), RUN);
        assert_eq!(consumer.try_pop(), Some(0.5));
        signals.paused.store(1, Ordering::Release);
        writer.write(&[0.2]);
        assert_eq!(consumer.try_pop(), None);
        signals.paused.store(0, Ordering::Release);
        writer.frames = writer.max_frames.unwrap() - 1;
        writer.write(&[0.3, 0.4]);
        assert_eq!(consumer.try_pop(), Some(0.3));
        assert_eq!(consumer.try_pop(), None);
        assert_eq!(signals.intent.load(Ordering::Acquire), LIMIT);
    }
    #[test]
    fn continuous_capture_crosses_hour_and_day_boundaries_until_explicit_stop() {
        for seconds in [3600u64, 3 * 3600, 86400] {
            let mut state = Signals::new();
            state.origin = Instant::now() - std::time::Duration::from_secs(seconds + 1);
            state.ready_ns.store(1, Ordering::Release);
            let signals = Arc::new(state);
            let (mut writer, mut consumer) =
                CallbackWriter::continuous(config(1), signals.clone()).unwrap();
            writer.frames = seconds * 16000 - 1;
            let allocations = crate::allocation_test::allocations_during(|| {
                writer.write(&[0.25, 0.5]);
            });
            assert_eq!(allocations, 0);
            assert_eq!(consumer.try_pop(), Some(0.25));
            assert_eq!(consumer.try_pop(), Some(0.5));
            assert_eq!(signals.intent.load(Ordering::Acquire), RUN);
            assert_eq!(
                signals.accepted_frames.load(Ordering::Acquire),
                seconds * 16000 + 1
            );
            signals.paused.store(1, Ordering::Release);
            writer.write(&[0.75]);
            assert_eq!(consumer.try_pop(), None);
            signals.paused.store(0, Ordering::Release);
            writer.write(&[0.75]);
            assert_eq!(consumer.try_pop(), Some(0.75));
            signals.intent.store(STOP, Ordering::Release);
            writer.write(&[1.0]);
            assert_eq!(consumer.try_pop(), None);
        }
    }
}
