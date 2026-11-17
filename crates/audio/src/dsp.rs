use crate::{
    error::{AudioError, Result},
    MAX_SAMPLES, MAX_SECONDS, SAMPLE_RATE,
};
use rubato::{
    calculate_cutoff, Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
use serde::{Deserialize, Serialize};

const BLOCK_FRAMES: usize = 1024;
const MAX_CHANNELS: usize = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "channel", rename_all = "snake_case")]
pub enum Mix {
    /// Average of the live channels. An input far below the loudest one (an
    /// unused interface input) is faded out instead of diluting speech by 6-12 dB.
    #[default]
    Average,
    Channel(u16),
}

/// Fixed-size downmix gate, updated every 20 ms. Levels attack at once and
/// release over seconds, so pauses in speech do not toggle membership.
/// Membership changes are crossfaded across the following window.
struct Gate {
    window: usize,
    position: usize,
    energy: [f64; MAX_CHANNELS],
    level: [f64; MAX_CHANNELS],
    from: [bool; MAX_CHANNELS],
    to: [bool; MAX_CHANNELS],
}
impl Gate {
    const RELEASE: f64 = 0.98;
    const ENTER: f64 = 0.01;
    const LEAVE: f64 = 0.0025;
    const FLOOR: f64 = 1e-9;

    fn new(rate: u32) -> Self {
        Self {
            window: (rate / 50) as usize,
            position: 0,
            energy: [0.0; MAX_CHANNELS],
            level: [0.0; MAX_CHANNELS],
            from: [true; MAX_CHANNELS],
            to: [true; MAX_CHANNELS],
        }
    }

    fn mix(&mut self, frame: &[f32]) -> f32 {
        let t = self.position as f64 / self.window as f64;
        let (mut sum, mut weight) = (0.0f64, 0.0f64);
        for (channel, &sample) in frame.iter().enumerate() {
            let sample = f64::from(sample.clamp(-1.0, 1.0));
            self.energy[channel] += sample * sample;
            // Both endpoints keep the loudest channel, so weight is always >= 1.
            let w = match (self.from[channel], self.to[channel]) {
                (true, true) => 1.0,
                (true, false) => 1.0 - t,
                (false, true) => t,
                (false, false) => 0.0,
            };
            sum += w * sample;
            weight += w;
        }
        self.position += 1;
        if self.position == self.window {
            self.update(frame.len());
        }
        (sum / weight) as f32
    }

    fn update(&mut self, channels: usize) {
        self.from = self.to;
        self.position = 0;
        let mut loudest = 0.0f64;
        for channel in 0..channels {
            let energy = std::mem::take(&mut self.energy[channel]) / self.window as f64;
            self.level[channel] = energy.max(self.level[channel] * Self::RELEASE);
            loudest = loudest.max(self.level[channel]);
        }
        if loudest < Self::FLOOR {
            return;
        }
        for channel in 0..channels {
            let ratio = if self.from[channel] {
                Self::LEAVE
            } else {
                Self::ENTER
            };
            self.to[channel] = self.level[channel] >= loudest * ratio;
        }
    }
}

#[derive(Debug)]
pub struct ProcessedAudio {
    pub samples: Vec<f32>,
    pub input_frames: u64,
    pub input_rate: u32,
    pub duration_ms: u64,
    pub reached_limit: bool,
}

/// Runs exclusively outside the device callback. The output buffer is reserved
/// before opening the microphone. Continuous streams drain PCM between blocks.
pub struct Dsp {
    rate: u32,
    channels: usize,
    mix: Mix,
    gate: Gate,
    resampler: Option<SincFixedIn<f32>>,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    filled: usize,
    frames: u64,
    pcm: Vec<f32>,
    last_rms: f32,
    fault: Option<AudioError>,
    max_seconds: Option<u32>,
    drained: usize,
}

pub fn allocated_zeros(count: usize) -> Result<Vec<f32>> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| AudioError::MemoryUnavailable)?;
    values.resize(count, 0.0);
    Ok(values)
}

impl Dsp {
    pub fn new(rate: u32, channels: u16, mix: Mix) -> Result<Self> {
        Self::with_limit(rate, channels, mix, MAX_SECONDS)
    }

    pub fn with_limit(rate: u32, channels: u16, mix: Mix, max_seconds: u32) -> Result<Self> {
        if !(1..=86400).contains(&max_seconds) {
            return Err(AudioError::UnsupportedAudioFormat);
        }
        Self::configure(rate, channels, mix, Some(max_seconds))
    }

    pub(crate) fn continuous(rate: u32, channels: u16, mix: Mix) -> Result<Self> {
        Self::configure(rate, channels, mix, None)
    }

    fn configure(rate: u32, channels: u16, mix: Mix, max_seconds: Option<u32>) -> Result<Self> {
        if !(8000..=192000).contains(&rate) || !(1..=8).contains(&channels) {
            return Err(AudioError::UnsupportedAudioFormat);
        }
        if matches!(mix, Mix::Channel(channel) if channel >= channels) {
            return Err(AudioError::InvalidChannel);
        }
        let mut pcm = Vec::new();
        pcm.try_reserve_exact(if max_seconds.is_some() {
            MAX_SAMPLES
        } else {
            BLOCK_FRAMES
        })
        .map_err(|_| AudioError::MemoryUnavailable)?;
        let resampler = if rate == SAMPLE_RATE {
            None
        } else {
            let window = WindowFunction::Blackman2;
            let params = SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: calculate_cutoff(256, window),
                interpolation: SincInterpolationType::Cubic,
                oversampling_factor: 256,
                window,
            };
            Some(
                SincFixedIn::new(
                    f64::from(SAMPLE_RATE) / f64::from(rate),
                    1.0,
                    params,
                    BLOCK_FRAMES,
                    1,
                )
                .map_err(|_| AudioError::ResamplingFailed)?,
            )
        };
        let output_len = resampler
            .as_ref()
            .map_or(BLOCK_FRAMES, Resampler::output_frames_max);
        Ok(Self {
            rate,
            channels: usize::from(channels),
            mix,
            gate: Gate::new(rate),
            resampler,
            input: vec![allocated_zeros(BLOCK_FRAMES)?],
            output: vec![allocated_zeros(output_len)?],
            filled: 0,
            frames: 0,
            pcm,
            last_rms: 0.0,
            fault: None,
            max_seconds,
            drained: 0,
        })
    }

    pub fn input_frames(&self) -> u64 {
        self.frames
    }
    pub fn last_rms(&self) -> f32 {
        self.last_rms
    }
    pub fn reached_limit(&self) -> bool {
        self.max_seconds
            .is_some_and(|seconds| self.frames >= u64::from(self.rate) * u64::from(seconds))
    }

    /// Input consists of complete interleaved frames. Samples outside the normal
    /// range are clamped, non-finite values fail the entire recording.
    pub fn push_interleaved(&mut self, samples: &[f32]) -> Result<usize> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let result = self.push_inner(samples);
        if let Err(error) = result {
            self.fault = Some(error);
        }
        result
    }

    fn push_inner(&mut self, samples: &[f32]) -> Result<usize> {
        if !samples.len().is_multiple_of(self.channels) {
            return Err(AudioError::Incomplete);
        }
        let input_frames = samples.len() / self.channels;
        let count = self.max_seconds.map_or(input_frames, |seconds| {
            input_frames.min((u64::from(self.rate) * u64::from(seconds) - self.frames) as usize)
        });
        let mut energy = 0.0f64;
        for frame in samples.chunks_exact(self.channels).take(count) {
            if frame.iter().any(|s| !s.is_finite()) {
                return Err(AudioError::InvalidSamples);
            }
            let sample = match self.mix {
                Mix::Average if self.channels > 1 => self.gate.mix(frame),
                Mix::Average => frame[0].clamp(-1.0, 1.0),
                Mix::Channel(index) => frame[usize::from(index)].clamp(-1.0, 1.0),
            };
            energy += f64::from(sample).powi(2);
            self.input[0][self.filled] = sample;
            self.filled += 1;
            self.frames += 1;
            if self.filled == BLOCK_FRAMES {
                self.process_block()?;
                self.filled = 0;
            }
        }
        self.last_rms = if count == 0 {
            0.0
        } else {
            (energy / count as f64).sqrt() as f32
        };
        Ok(count)
    }

    fn process_block(&mut self) -> Result<()> {
        let produced = if let Some(resampler) = &mut self.resampler {
            resampler
                .process_into_buffer(&self.input, &mut self.output, None)
                .map_err(|_| AudioError::ResamplingFailed)?
                .1
        } else {
            self.output[0].copy_from_slice(&self.input[0]);
            BLOCK_FRAMES
        };
        // In pinned SincFixedIn 0.16.2 the first output is already aligned to
        // the input beginning (the initial index is -sinc_len/2). output_delay()
        // describes lookahead latency, not extra leading samples to discard.
        // Removing it again loses the first word and pads silence at the end.
        let keep = self.max_seconds.map_or(produced, |seconds| {
            produced.min(seconds as usize * SAMPLE_RATE as usize - self.drained - self.pcm.len())
        });
        let output = &self.output[0][..keep];
        if output.iter().any(|s| !s.is_finite()) {
            return Err(AudioError::InvalidSamples);
        }
        // Sinc interpolation may slightly overshoot even for bounded inputs.
        self.pcm
            .extend(output.iter().map(|sample| sample.clamp(-1.0, 1.0)));
        Ok(())
    }

    /// Drain completed samples without resetting filter state or the total duration budget.
    pub fn take_samples(&mut self) -> Vec<f32> {
        self.drained += self.pcm.len();
        let samples = self.pcm.clone();
        self.pcm.clear();
        samples
    }
    /// As `take_samples`, but lends the samples and keeps the buffer's capacity.
    pub(crate) fn drain_into(&mut self, sink: &mut impl FnMut(&[f32]) -> Result<()>) -> Result<()> {
        self.drained += self.pcm.len();
        let result = sink(&self.pcm);
        self.pcm.clear();
        result
    }
    pub fn buffered_samples(&self) -> usize {
        self.pcm.len()
    }

    pub fn finish(mut self) -> Result<ProcessedAudio> {
        if let Some(error) = self.fault {
            return Err(error);
        }
        let target = ((self.frames * u64::from(SAMPLE_RATE)) / u64::from(self.rate)) as usize;
        if target < 4000 {
            return Err(AudioError::TooShort);
        }
        let target = target.saturating_sub(self.drained);
        if self.filled > 0 {
            self.input[0][self.filled..].fill(0.0);
            self.process_block()?;
        }
        self.input[0].fill(0.0);
        // The selected filter has at most 256 input frames of delay. Four
        // 1024-frame zero blocks are a conservative bound on full tail draining.
        for _ in 0..4 {
            if self.pcm.len() >= target {
                break;
            }
            self.process_block()?;
        }
        if self.pcm.len() < target {
            return Err(AudioError::ResamplingFailed);
        }
        self.pcm.truncate(target);
        let reached_limit = self.reached_limit();
        Ok(ProcessedAudio {
            samples: self.pcm,
            input_frames: self.frames,
            input_rate: self.rate,
            duration_ms: self.frames * 1000 / u64::from(self.rate),
            reached_limit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;
    fn tone(rate: u32, hz: f32, seconds: f32, channels: usize) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .flat_map(|n| {
                std::iter::repeat_n(0.5 * (TAU * hz * n as f32 / rate as f32).sin(), channels)
            })
            .collect()
    }
    fn process(rate: u32, channels: u16, samples: &[f32]) -> Vec<f32> {
        let mut dsp = Dsp::new(rate, channels, Mix::Average).unwrap();
        for chunk in samples.chunks(137 * usize::from(channels)) {
            dsp.push_interleaved(chunk).unwrap();
        }
        dsp.finish().unwrap().samples
    }
    #[test]
    fn common_and_irregular_rates_preserve_duration_and_pitch() {
        for rate in [8000, 16000, 22050, 44100, 44101, 48000, 96000, 192000] {
            let samples = tone(rate, 1000.0, 0.507, 2);
            let output = process(rate, 2, &samples);
            assert_eq!(
                output.len(),
                samples.len() / 2 * 16000 / rate as usize,
                "rate {rate}"
            );
            let core = &output[1000..output.len() - 1000];
            let crossings = core
                .windows(2)
                .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
                .count();
            let hz = crossings as f32 * 16000.0 / core.len() as f32;
            assert!((hz - 1000.0).abs() < 4.0, "rate {rate}: {hz} Hz");
            let rms = (core.iter().map(|s| s * s).sum::<f32>() / core.len() as f32).sqrt();
            assert!((rms - 0.3535).abs() < 0.015, "rate {rate}: {rms} rms");
        }
    }
    #[test]
    fn antialias_filter_rejects_out_of_band_audio() {
        let output = process(48000, 1, &tone(48000, 12000.0, 1.0, 1));
        let core = &output[1000..15000];
        let rms = (core.iter().map(|s| s * s).sum::<f32>() / core.len() as f32).sqrt();
        assert!(rms < 0.002, "aliased RMS: {rms}");
    }
    #[test]
    fn downmix_and_selected_channel_are_explicit() {
        let frames: Vec<f32> = (0..4000).flat_map(|_| [0.5, -0.5]).collect();
        assert!(process(16000, 2, &frames).iter().all(|s| *s == 0.0));
        let mut dsp = Dsp::new(16000, 2, Mix::Channel(1)).unwrap();
        dsp.push_interleaved(&frames).unwrap();
        assert!(dsp.finish().unwrap().samples.iter().all(|s| *s == -0.5));
        assert!(matches!(
            Dsp::new(48000, 2, Mix::Channel(2)),
            Err(AudioError::InvalidChannel)
        ));
    }
    #[test]
    fn downmix_excludes_unused_inputs_and_readmits_a_channel_that_becomes_live() {
        let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
        let voice = tone(48000, 1000.0, 1.0, 1);
        for (channels, live) in [(2usize, 0usize), (2, 1), (4, 2)] {
            let frames: Vec<f32> = voice
                .iter()
                .enumerate()
                .flat_map(|(n, &v)| {
                    let hiss = if n % 2 == 0 { 1e-4 } else { -1e-4 };
                    (0..channels).map(move |c| if c == live { v } else { hiss })
                })
                .collect();
            let output = process(48000, channels as u16, &frames);
            let level = rms(&output[1600..output.len() - 1000]);
            assert!((level - 0.3535).abs() < 0.015, "{channels}ch: {level} rms");
        }
        // Channel 0 joins in anti-phase after 0.5 s: once admitted, the mix cancels.
        let half = voice.len() / 2;
        let frames: Vec<f32> = voice
            .iter()
            .enumerate()
            .flat_map(|(n, &v)| [if n < half { 0.0 } else { -v }, v])
            .collect();
        let output = process(48000, 2, &frames);
        assert!((rms(&output[1600..7600]) - 0.3535).abs() < 0.015);
        assert!(rms(&output[8000 + 960..output.len() - 1000]) < 0.002);
    }
    #[test]
    fn continuous_drain_does_not_allocate_once_warm() {
        let mut dsp = Dsp::continuous(48000, 2, Mix::Average).unwrap();
        let block: Vec<f32> = tone(48000, 440.0, 0.1, 2);
        let mut received = 0;
        let mut sink = |samples: &[f32]| {
            received += samples.len();
            Ok(())
        };
        for _ in 0..4 {
            dsp.push_interleaved(&block).unwrap();
            dsp.drain_into(&mut sink).unwrap();
        }
        let allocations = crate::allocation_test::allocations_during(|| {
            for _ in 0..20 {
                dsp.push_interleaved(&block).unwrap();
                dsp.drain_into(&mut sink).unwrap();
            }
        });
        assert_eq!(allocations, 0);
        assert!(received > 0);
    }
    #[test]
    fn short_malformed_or_nonfinite_audio_never_returns_partial_text_input() {
        for (input, error) in [
            (vec![0.0], AudioError::Incomplete),
            (vec![f32::NAN, 0.0], AudioError::InvalidSamples),
        ] {
            let mut dsp = Dsp::new(16000, 2, Mix::Average).unwrap();
            dsp.push_interleaved(&vec![0.25; 8000]).unwrap();
            assert_eq!(dsp.push_interleaved(&input), Err(error));
            assert_eq!(dsp.push_interleaved(&[0.0, 0.0]), Err(error));
            assert!(matches!(dsp.finish(), Err(value) if value == error));
        }
        assert!(matches!(
            Dsp::new(16000, 1, Mix::Average).unwrap().finish(),
            Err(AudioError::TooShort)
        ));
    }
    #[test]
    fn filter_tail_keeps_the_first_and_last_impulses() {
        for rate in [8000, 44100, 48000, 96000, 192000] {
            let mut samples = vec![0.0; rate as usize / 2 + 13];
            samples[8] = 0.8;
            let tail = samples.len() - 9;
            samples[tail] = 0.8;
            let output = process(rate, 1, &samples);
            let position = |range: &[f32]| {
                range
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                    .unwrap()
                    .0
            };
            let first_peak = position(&output[..100]);
            let last_peak = output.len() - 100 + position(&output[output.len() - 100..]);
            assert!(
                first_peak.abs_diff(8 * 16000 / rate as usize) <= 3,
                "start shifted at {rate}"
            );
            assert!(
                last_peak.abs_diff(tail * 16000 / rate as usize) <= 3,
                "tail shifted at {rate}"
            );
            assert!(
                output[..100].iter().any(|v| v.abs() > 0.005),
                "lost start at {rate}"
            );
            assert!(
                output[output.len() - 100..].iter().any(|v| v.abs() > 0.005),
                "lost tail at {rate}"
            );
        }
    }
    #[test]
    fn full_five_minutes_is_exact_even_when_the_last_block_overruns() {
        for rate in [8000, 16000, 44100, 48000, 192000] {
            let mut dsp = Dsp::new(rate, 1, Mix::Average).unwrap();
            let mut remaining = rate as usize * 300;
            let block = [0.125; 997];
            while remaining > 0 {
                let accepted = dsp.push_interleaved(&block).unwrap();
                remaining -= accepted;
            }
            assert!(dsp.reached_limit());
            assert_eq!(dsp.push_interleaved(&block).unwrap(), 0);
            let result = dsp.finish().unwrap();
            assert_eq!(result.samples.len(), MAX_SAMPLES);
            assert_eq!(result.duration_ms, 300000);
            assert!(result.reached_limit);
            assert!(result.samples[result.samples.len() - 100..]
                .iter()
                .any(|s| *s > 0.05));
        }
    }
    #[test]
    fn one_hour_stream_preserves_every_sample_with_bounded_buffers() {
        let mut dsp = Dsp::with_limit(16000, 1, Mix::Average, 3600).unwrap();
        let block = vec![0.25; 16000];
        let mut count = 0;
        for _ in 0..3601 {
            dsp.push_interleaved(&block).unwrap();
            assert!(dsp.buffered_samples() <= 16384);
            count += dsp.take_samples().len();
        }
        let tail = dsp.finish().unwrap();
        count += tail.samples.len();
        assert_eq!(count, 3600 * 16000);
        assert_eq!(tail.duration_ms, 3600000);
        assert!(tail.reached_limit);
    }
    #[test]
    fn three_hour_continuous_stream_preserves_all_audio_with_bounded_buffers() {
        let mut dsp = Dsp::continuous(16000, 1, Mix::Average).unwrap();
        let block = [0.25; 16000];
        let mut count = 0;
        for _ in 0..3 * 3600 {
            assert_eq!(dsp.push_interleaved(&block).unwrap(), block.len());
            assert!(!dsp.reached_limit());
            assert!(dsp.buffered_samples() <= 16384);
            let samples = dsp.take_samples();
            assert!(samples.iter().all(|sample| *sample == 0.25));
            count += samples.len();
        }
        // Ending partway through a DSP block must also retain the last spoken samples.
        dsp.push_interleaved(&[0.75; 4001]).unwrap();
        let tail = dsp.finish().unwrap();
        assert!(tail.samples.iter().all(|sample| *sample == 0.75));
        count += tail.samples.len();
        assert_eq!(count, 3 * 3600 * 16000 + 4001);
        assert_eq!(tail.duration_ms, 3 * 3600000 + 250);
        assert!(!tail.reached_limit);
    }
}
