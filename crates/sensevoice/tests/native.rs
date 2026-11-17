#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use sha2::{Digest, Sha256};
use speakmore_sensevoice::{versions, Recognizer, Vad};
use std::{path::Path, time::Instant};

#[test]
#[ignore = "requires public SenseVoice fixture files, never opens a microphone"]
fn public_speech_real_native_decoder_and_owned_results() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.cache/sensevoice/model")
        .canonicalize()
        .unwrap();
    for (file, sha) in [
        (
            "model.int8.onnx",
            "c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51",
        ),
        (
            "tokens.txt",
            "f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc",
        ),
    ] {
        assert_eq!(
            format!(
                "{:x}",
                Sha256::digest(std::fs::read(root.join(file)).unwrap())
            ),
            sha
        );
    }
    let started = Instant::now();
    let mut decoder =
        Recognizer::load(&root.join("model.int8.onnx"), &root.join("tokens.txt"), 4).unwrap();
    eprintln!(
        "SenseVoice native versions {:?}; loaded {} ms",
        versions().unwrap(),
        started.elapsed().as_millis()
    );
    let mut retained = Vec::new();
    for (file, language, expected) in [
        ("zh.wav", "zh", "9点"),
        ("en.wav", "en", "tribal"),
        ("yue.wav", "auto", ""),
        ("ja.wav", "ja", ""),
        ("ko.wav", "ko", ""),
    ] {
        let mut wav = hound::WavReader::open(root.join("test_wavs").join(file)).unwrap();
        assert_eq!(wav.spec().sample_rate, 16_000);
        assert_eq!(wav.spec().channels, 1);
        let samples: Vec<f32> = wav
            .samples::<i16>()
            .map(|s| f32::from(s.unwrap()) / 32768.0)
            .collect();
        let started = Instant::now();
        let result = decoder.decode(&samples, language).unwrap();
        eprintln!(
            "Public fixture {file}: {} ms, lang={}, {} tokens, {} bytes",
            started.elapsed().as_millis(),
            result.language,
            result.tokens.len(),
            result.text.len()
        );
        assert!(!result.text.is_empty());
        assert!(!result.text.contains("<|"));
        assert!(
            result.text.to_lowercase().contains(expected),
            "unexpected public fixture text: {}",
            result.text
        );
        assert!(!result.tokens.is_empty());
        assert_eq!(
            result.language,
            if language == "auto" { "yue" } else { language }
        );
        retained.push(result);
    }
    assert!(decoder.decode(&[0.0; 3000], "auto").is_err());
    assert!(decoder.decode(&vec![f32::NAN; 4000], "auto").is_err());
    drop(decoder);
    assert!(retained[1].text.to_lowercase().contains("tribal"));
}

#[test]
#[ignore = "profiles a real 300-second public-speech fixture; never opens the microphone"]
fn public_five_minute_recording_retains_repeated_phrases() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.cache/sensevoice/model")
        .canonicalize()
        .unwrap();
    let mut wav = hound::WavReader::open(root.join("test_wavs/en.wav")).unwrap();
    let mut phrase: Vec<f32> = wav
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect();
    phrase.extend(vec![0.0; 16_000]);
    let expected = 4_800_000 / phrase.len();
    let samples: Vec<f32> = phrase.iter().copied().cycle().take(4_800_000).collect();
    let mut decoder =
        Recognizer::load(&root.join("model.int8.onnx"), &root.join("tokens.txt"), 4).unwrap();
    let started = Instant::now();
    let result = decoder.decode(&samples, "en").unwrap();
    let count = result.text.to_lowercase().matches("tribal").count();
    eprintln!("Full 300-second SenseVoice call: {} ms, {} bytes, {count} repeated phrases / {expected} complete input phrases", started.elapsed().as_millis(), result.text.len());
    assert!((expected..=expected + 1).contains(&count));
    assert!(!result.text.contains("<|"));
}

#[test]
#[ignore = "uses pinned public VAD and speech files; no microphone"]
fn vad_silence_full_five_minutes_cancel_and_clean_reuse() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.cache/sensevoice/model")
        .canonicalize()
        .unwrap();
    let mut vad = Vad::load(&root.join("silero_vad_v5.onnx")).unwrap();
    assert_eq!(
        vad.speech_bounds(&vec![0.0; 4_800_000], || Ok(())).unwrap(),
        None
    );
    let mut wav = hound::WavReader::open(root.join("test_wavs/en.wav")).unwrap();
    let speech: Vec<f32> = wav
        .samples::<i16>()
        .map(|s| f32::from(s.unwrap()) / 32768.0)
        .collect();
    let baseline = vad.speech_bounds(&speech, || Ok(())).unwrap().unwrap();
    let mut ticks = 0;
    assert_eq!(
        vad.speech_bounds(&speech, || {
            ticks += 1;
            if ticks > 100 {
                Err("Cancelled")
            } else {
                Ok(())
            }
        }),
        Err("Cancelled")
    );
    assert_eq!(
        vad.speech_bounds(&speech, || Ok(())).unwrap().unwrap(),
        baseline
    );
    // All five minutes remain within native buffering; flush only at session end.
    let full: Vec<f32> = speech.iter().copied().cycle().take(4_800_000).collect();
    let bounds = vad.speech_bounds(&full, || Ok(())).unwrap().unwrap();
    assert!(bounds.0 < 16_000 && bounds.1 > 4_784_000);
    assert_eq!(vad.speech_bounds(&[0.0; 4000], || Ok(())).unwrap(), None);
}
