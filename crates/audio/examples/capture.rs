//! Explicit, manual native microphone probe. Audio stays in memory and is
//! dropped after printing sample counts; no ASR, file export or networking.
use speakmore_audio::capture::{AudioService, CaptureEvent, Selection};
use std::{
    thread,
    time::{Duration, Instant},
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 || args[0] != "--record-for" {
        eprintln!(
            "Usage: capture --record-for <1..300 seconds>. This opens your default microphone."
        );
        std::process::exit(64);
    }
    let seconds: u64 = args[1]
        .parse()
        .ok()
        .filter(|seconds| (1..=300).contains(seconds))
        .expect("duration must be 1..300 seconds");
    let service = AudioService::default();
    let handle = service
        .start(uuid::Uuid::new_v4(), Selection::default())
        .expect("unable to start audio service");
    let deadline = Instant::now() + Duration::from_secs(seconds + 5);
    let mut requested_stop = false;
    loop {
        if !requested_stop && handle.elapsed_ms() >= seconds * 1000 {
            handle.stop();
            requested_stop = true;
        }
        match handle.try_recv() {
            Ok(CaptureEvent::Ready { configuration, .. }) => println!(
                "Microphone ready: {} Hz, {} channel(s)",
                configuration.sample_rate, configuration.channels
            ),
            Ok(CaptureEvent::Finished {
                result,
                diagnostics,
                ..
            }) => {
                match result {
                    Ok(audio) => println!("Capture complete: {} ms, {} mono 16 kHz samples, {} clipped device samples", audio.duration_ms, audio.samples.len(), diagnostics.clipped_samples),
                    Err(error) => { eprintln!("{error}"); std::process::exit(1); }
                }
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                eprintln!("Audio service exited without a result");
                std::process::exit(1);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        if Instant::now() >= deadline {
            handle.cancel();
            eprintln!("Capture deadline exceeded");
            std::process::exit(1);
        }
        thread::sleep(Duration::from_millis(5));
    }
}
