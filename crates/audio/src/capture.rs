use crate::{
    device::{self, Configuration, Format},
    dsp::{Dsp, Mix, ProcessedAudio},
    error::{AudioError, Result},
    realtime::{
        CallbackWriter, InputSample, Signals, CANCEL, DEVICE_LOST, INCOMPLETE, LIMIT, PERMISSION,
        RUN, STOP,
    },
};
use cpal::traits::{DeviceTrait, StreamTrait};
use ringbuf::{
    traits::{Consumer, Observer},
    HeapCons,
};
use serde::Serialize;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub device_id: Option<String>,
    pub mix: Mix,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub accepted_frames: u64,
    pub clipped_samples: u64,
    pub scheduling_fallback: bool,
}
#[derive(Debug)]
pub enum CaptureEvent {
    Ready {
        session_id: Uuid,
        device_id: String,
        configuration: Configuration,
        started_at: Instant,
    },
    Finished {
        session_id: Uuid,
        result: Result<ProcessedAudio>,
        diagnostics: Diagnostics,
    },
}

#[derive(Default, Clone)]
pub struct AudioService {
    busy: Arc<AtomicBool>,
}
struct Lease(Arc<AtomicBool>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub struct CaptureHandle {
    signals: Arc<Signals>,
    events: Receiver<CaptureEvent>,
}
impl CaptureHandle {
    pub fn pause(&self, paused: bool) {
        if !paused {
            self.signals.last_data_ns.store(
                self.signals.origin.elapsed().as_nanos() as u64,
                Ordering::Release,
            );
        }
        self.signals
            .paused
            .store(u8::from(paused), Ordering::Release);
    }
    pub fn stop(&self) {
        let _ =
            self.signals
                .intent
                .compare_exchange(RUN, STOP, Ordering::AcqRel, Ordering::Acquire);
    }
    pub fn cancel(&self) {
        self.signals.intent.store(CANCEL, Ordering::Release);
    }
    pub fn captured_ms(&self, sample_rate: u32) -> u64 {
        self.signals.accepted_frames.load(Ordering::Acquire) * 1000 / u64::from(sample_rate.max(1))
    }
    pub fn elapsed_ms(&self) -> u64 {
        self.signals.elapsed_ms()
    }
    pub fn level(&self) -> f32 {
        f32::from_bits(self.signals.rms_bits.load(Ordering::Relaxed))
    }
    pub fn try_recv(&self) -> std::result::Result<CaptureEvent, TryRecvError> {
        self.events.try_recv()
    }
}
impl Drop for CaptureHandle {
    fn drop(&mut self) {
        // Never block the UI waiting for a native stream destructor. The owner
        // observes cancellation and drops its stream, retaining its busy lease
        // until cleanup actually completes.
        self.cancel();
    }
}

impl AudioService {
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }
    pub fn start(&self, session_id: Uuid, selection: Selection) -> Result<CaptureHandle> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| AudioError::Busy)?;
        let lease = Lease(self.busy.clone());
        let signals = Arc::new(Signals::new());
        let control = signals.clone();
        let (sender, events) = mpsc::sync_channel(4);
        thread::Builder::new()
            .name("speakmore-audio".into())
            .spawn(move || {
                let result = capture(session_id, selection, &control, &sender);
                let result = if control.intent.load(Ordering::Acquire) == CANCEL {
                    Err(AudioError::Cancelled)
                } else {
                    result
                };
                let diagnostics = Diagnostics {
                    accepted_frames: control.accepted_frames.load(Ordering::Acquire),
                    clipped_samples: control.clipped_samples.load(Ordering::Relaxed),
                    scheduling_fallback: control.scheduling_fallback.load(Ordering::Relaxed) != 0,
                };
                // Stream has already been dropped. Release its lease before
                // publishing completion so a receiver can start the next session
                // without spuriously observing Busy. A closed receiver discards PCM.
                drop(lease);
                let _ = sender.try_send(CaptureEvent::Finished {
                    session_id,
                    result,
                    diagnostics,
                });
            })
            .map_err(|_| AudioError::ThreadUnavailable)?;
        Ok(CaptureHandle { signals, events })
    }
}

fn check_preparing(signals: &Signals) -> Result<()> {
    if signals.intent.load(Ordering::Acquire) != RUN {
        return Err(AudioError::Cancelled);
    }
    if signals.origin.elapsed() >= Duration::from_secs(3) {
        return Err(AudioError::PrepareTimeout);
    }
    signals.error()
}

fn make_stream<T: InputSample>(
    device: &cpal::Device,
    config: Configuration,
    mut writer: CallbackWriter,
    signals: Arc<Signals>,
) -> Result<cpal::Stream> {
    device
        .build_input_stream::<T, _, _>(
            config.stream_config(),
            move |data, _| writer.write(data),
            move |error| {
                // DeviceChanged is fatal even if the backend says it rerouted itself.
                // RealtimeDenied is a scheduling diagnostic; it does not report lost PCM.
                match error.kind() {
                    cpal::ErrorKind::RealtimeDenied => {
                        signals.scheduling_fallback.store(1, Ordering::Relaxed)
                    }
                    cpal::ErrorKind::PermissionDenied => signals.fail(PERMISSION),
                    cpal::ErrorKind::DeviceChanged
                    | cpal::ErrorKind::DeviceNotAvailable
                    | cpal::ErrorKind::StreamInvalidated => signals.fail(DEVICE_LOST),
                    _ => signals.fail(INCOMPLETE),
                }
            },
            Some(Duration::from_secs(3)),
        )
        .map_err(device::map_error)
}

fn pump(
    consumer: &mut HeapCons<f32>,
    block: &mut [f32],
    channels: usize,
    dsp: &mut Dsp,
    signals: &Signals,
) -> Result<bool> {
    let count = (consumer.occupied_len().min(block.len()) / channels) * channels;
    if count == 0 {
        return Ok(false);
    }
    let received = consumer.pop_slice(&mut block[..count]);
    if received != count {
        return Err(AudioError::Incomplete);
    }
    dsp.push_interleaved(&block[..count])?;
    signals
        .rms_bits
        .store(dsp.last_rms().to_bits(), Ordering::Relaxed);
    Ok(true)
}

fn capture(
    session_id: Uuid,
    selection: Selection,
    signals: &Arc<Signals>,
    events: &SyncSender<CaptureEvent>,
) -> Result<ProcessedAudio> {
    check_preparing(signals)?;
    let (device, device_id) = device::resolve(selection.device_id.as_deref())?;
    let config = device::negotiate(&device)?;
    let mut dsp = Dsp::new(config.sample_rate, config.channels, selection.mix)?;
    let (writer, mut consumer) = CallbackWriter::new(config, signals.clone())?;
    let mut block = crate::dsp::allocated_zeros(1024 * usize::from(config.channels))?;
    check_preparing(signals)?;
    let stream = match config.sample_format {
        Format::F32 => make_stream::<f32>(&device, config, writer, signals.clone())?,
        Format::I16 => make_stream::<i16>(&device, config, writer, signals.clone())?,
        Format::U16 => make_stream::<u16>(&device, config, writer, signals.clone())?,
    };
    // A stop during a slow native build must never start the microphone later.
    check_preparing(signals)?;
    stream.play().map_err(device::map_error)?;
    let mut announced = false;
    loop {
        signals.error()?;
        let intent = signals.intent.load(Ordering::Acquire);
        if intent != RUN {
            break;
        }
        let ready_ns = signals.ready_ns.load(Ordering::Acquire);
        if ready_ns == 0 {
            check_preparing(signals)?;
        } else {
            if !announced {
                events
                    .try_send(CaptureEvent::Ready {
                        session_id,
                        device_id: device_id.clone(),
                        configuration: config,
                        started_at: signals.origin + Duration::from_nanos(ready_ns),
                    })
                    .map_err(|_| AudioError::Cancelled)?;
                announced = true;
            }
            if signals.elapsed_ms() >= 300000 {
                let _ = signals.intent.compare_exchange(
                    RUN,
                    LIMIT,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                break;
            }
            let last = signals.last_data_ns.load(Ordering::Acquire);
            if (signals.origin.elapsed().as_nanos() as u64).saturating_sub(last) >= 2_000_000_000 {
                return Err(AudioError::Incomplete);
            }
        }
        if !pump(
            &mut consumer,
            &mut block,
            usize::from(config.channels),
            &mut dsp,
            signals,
        )? {
            thread::sleep(Duration::from_millis(2));
        }
    }
    let drain_started = Instant::now();
    drop(stream);
    if !announced {
        return Err(AudioError::Cancelled);
    }
    if signals.intent.load(Ordering::Acquire) == CANCEL {
        return Err(AudioError::Cancelled);
    }
    signals.error()?;
    while pump(
        &mut consumer,
        &mut block,
        usize::from(config.channels),
        &mut dsp,
        signals,
    )? {
        if drain_started.elapsed() >= Duration::from_secs(1) {
            return Err(AudioError::DrainTimeout);
        }
        if signals.intent.load(Ordering::Acquire) == CANCEL {
            return Err(AudioError::Cancelled);
        }
    }
    if consumer.occupied_len() != 0
        || dsp.input_frames() != signals.accepted_frames.load(Ordering::Acquire)
    {
        return Err(AudioError::Incomplete);
    }
    let mut audio = dsp.finish()?;
    // The monotonic clock may reach five minutes before the device sample
    // counter does. Both are normal limit stops and must be reported that way.
    audio.reached_limit |= signals.intent.load(Ordering::Acquire) == LIMIT;
    if drain_started.elapsed() >= Duration::from_secs(1) {
        return Err(AudioError::DrainTimeout);
    }
    signals.error()?;
    Ok(audio)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preparation_cancellation_and_deadline_are_checked_before_opening_audio() {
        let mut signals = Signals::new();
        signals.origin = Instant::now() - Duration::from_secs(4);
        assert_eq!(check_preparing(&signals), Err(AudioError::PrepareTimeout));
        signals.intent.store(CANCEL, Ordering::Release);
        assert_eq!(check_preparing(&signals), Err(AudioError::Cancelled));
    }
    #[test]
    fn one_audio_lease_blocks_reentry_without_touching_the_device() {
        let service = AudioService::default();
        service.busy.store(true, Ordering::Release);
        assert!(matches!(
            service.start(Uuid::new_v4(), Selection::default()),
            Err(AudioError::Busy)
        ));
    }
}

impl AudioService {
    /// Long recordings stream bounded PCM blocks to durable storage outside the realtime callback.
    pub fn start_meeting(
        &self,
        session_id: Uuid,
        selection: Selection,
        sink: impl FnMut(&[f32]) -> Result<()> + Send + 'static,
    ) -> Result<CaptureHandle> {
        self.start_continuous(session_id, selection, 80_000, sink)
    }
    pub fn start_stream(
        &self,
        session_id: Uuid,
        selection: Selection,
        seconds: u32,
        block_samples: usize,
        sink: impl FnMut(&[f32]) -> Result<()> + Send + 'static,
    ) -> Result<CaptureHandle> {
        if !(1..=86400).contains(&seconds) {
            return Err(AudioError::Incomplete);
        }
        self.start_stream_with_limit(session_id, selection, Some(seconds), block_samples, sink)
    }

    /// Continuous capture keeps the device and resampler open until explicitly stopped.
    pub fn start_continuous(
        &self,
        session_id: Uuid,
        selection: Selection,
        block_samples: usize,
        sink: impl FnMut(&[f32]) -> Result<()> + Send + 'static,
    ) -> Result<CaptureHandle> {
        self.start_stream_with_limit(session_id, selection, None, block_samples, sink)
    }

    fn start_stream_with_limit(
        &self,
        session_id: Uuid,
        selection: Selection,
        seconds: Option<u32>,
        block_samples: usize,
        mut sink: impl FnMut(&[f32]) -> Result<()> + Send + 'static,
    ) -> Result<CaptureHandle> {
        if !(1600..=80_000).contains(&block_samples) {
            return Err(AudioError::Incomplete);
        }
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| AudioError::Busy)?;
        let lease = Lease(self.busy.clone());
        let signals = Arc::new(Signals::new());
        let control = signals.clone();
        let (sender, events) = mpsc::sync_channel(4);
        thread::Builder::new()
            .name("speakmore-meeting-audio".into())
            .spawn(move || {
                let result = (|| {
                    check_preparing(&control)?;
                    let (device, device_id) = device::resolve(selection.device_id.as_deref())?;
                    let config = device::negotiate(&device)?;
                    let mut dsp = match seconds {
                        Some(seconds) => Dsp::with_limit(
                            config.sample_rate,
                            config.channels,
                            selection.mix,
                            seconds,
                        )?,
                        None => {
                            Dsp::continuous(config.sample_rate, config.channels, selection.mix)?
                        }
                    };
                    let (writer, mut consumer) = match seconds {
                        Some(seconds) => {
                            CallbackWriter::with_limit(config, control.clone(), seconds)?
                        }
                        None => CallbackWriter::continuous(config, control.clone())?,
                    };
                    let mut block =
                        crate::dsp::allocated_zeros(1024 * usize::from(config.channels))?;
                    let stream = match config.sample_format {
                        Format::F32 => {
                            make_stream::<f32>(&device, config, writer, control.clone())?
                        }
                        Format::I16 => {
                            make_stream::<i16>(&device, config, writer, control.clone())?
                        }
                        Format::U16 => {
                            make_stream::<u16>(&device, config, writer, control.clone())?
                        }
                    };
                    check_preparing(&control)?;
                    stream.play().map_err(device::map_error)?;
                    let mut announced = false;
                    while control.intent.load(Ordering::Acquire) == RUN {
                        control.error()?;
                        let ready_ns = control.ready_ns.load(Ordering::Acquire);
                        if !announced && ready_ns != 0 {
                            let _ = sender.try_send(CaptureEvent::Ready {
                                session_id,
                                device_id: device_id.clone(),
                                configuration: config,
                                started_at: control.origin + Duration::from_nanos(ready_ns),
                            });
                            announced = true;
                        }
                        if !announced {
                            check_preparing(&control)?;
                        }
                        if control.paused.load(Ordering::Acquire) == 0 && announced {
                            let last = control.last_data_ns.load(Ordering::Acquire);
                            if (control.origin.elapsed().as_nanos() as u64).saturating_sub(last)
                                > 5_000_000_000
                            {
                                return Err(AudioError::Incomplete);
                            }
                        }
                        if !pump(
                            &mut consumer,
                            &mut block,
                            usize::from(config.channels),
                            &mut dsp,
                            &control,
                        )? {
                            thread::sleep(Duration::from_millis(2));
                        }
                        if dsp.buffered_samples() >= block_samples {
                            dsp.drain_into(&mut sink)?;
                        }
                    }
                    drop(stream);
                    while pump(
                        &mut consumer,
                        &mut block,
                        usize::from(config.channels),
                        &mut dsp,
                        &control,
                    )? {
                        if dsp.buffered_samples() >= block_samples {
                            dsp.drain_into(&mut sink)?;
                        }
                    }
                    let mut result = dsp.finish()?;
                    if !result.samples.is_empty() {
                        sink(&result.samples)?;
                        result.samples.clear();
                    }
                    control.error()?;
                    Ok(result)
                })();
                drop(lease);
                let _ = sender.try_send(CaptureEvent::Finished {
                    session_id,
                    result,
                    diagnostics: Diagnostics {
                        accepted_frames: control.accepted_frames.load(Ordering::Acquire),
                        clipped_samples: control.clipped_samples.load(Ordering::Relaxed),
                        scheduling_fallback: control.scheduling_fallback.load(Ordering::Relaxed)
                            != 0,
                    },
                });
            })
            .map_err(|_| AudioError::ThreadUnavailable)?;
        Ok(CaptureHandle { signals, events })
    }
}
