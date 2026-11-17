use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use uuid::Uuid;

pub const VERSION: u32 = 2;
pub const BUILD_ID: &str = match option_env!("SPEAKMORE_BUILD_ID") {
    Some(value) => value,
    None => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};
pub const MAX_SAMPLES: usize = 4_800_000;
pub const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_CONTROL_BYTES: usize = 64 * 1024;
const AUDIO_KIND: u8 = 4;
pub const ENGINE_PROFILE: &str = "whisper-v1";
pub const PARAKEET_PROFILE: &str = "parakeet-tdt-v3-int8-v1";
pub const QWEN_ASR_PROFILE: &str = "qwen3-asr-0.6b-int8-v1";
pub const SENSEVOICE_PROFILE: &str = "sensevoice-v1";
// Distinct, immutable execution profile for the reproducible 256-query-row
// graph. Support in code does not publish a model in the signed catalog.
pub const SENSEVOICE_TILED_PROFILE: &str = "sensevoice-tiled256-v1";
pub const SENSEVOICE_TILED_SHA256: &str =
    "46346d31c9314b4873aa16882f20955774cda4048a849c80db2a27117569a3d8";
pub const SENSEVOICE_TILED_BYTES: u64 = 239_537_160;
/// Session `Failed` code for the worker's own inference deadline.
pub const INFERENCE_TIMEOUT: &str = "InferenceTimeout";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelFile {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
}
impl ModelFile {
    pub fn valid(&self, max_size: u64) -> bool {
        std::path::Path::new(&self.path).is_absolute()
            && self.path.len() <= 4096
            && !self.path.contains('\0')
            && valid_sha256(&self.sha256)
            && (1..=max_size).contains(&self.size_bytes)
    }
}

pub fn is_sensevoice_profile(profile: &str) -> bool {
    matches!(profile, SENSEVOICE_PROFILE | SENSEVOICE_TILED_PROFILE)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelAsset {
    pub role: String,
    pub file: ModelFile,
}

pub fn auxiliary_names(profile: &str) -> &'static [(&'static str, &'static str)] {
    match profile {
        PARAKEET_PROFILE => &[
            ("decoder", "decoder.int8.onnx"),
            ("joiner", "joiner.int8.onnx"),
        ],
        QWEN_ASR_PROFILE => &[
            ("frontend", "conv_frontend.onnx"),
            ("decoder", "decoder.int8.onnx"),
            ("vocab", "vocab.json"),
            ("merges", "merges.txt"),
            ("tokenizer-config", "tokenizer_config.json"),
        ],
        _ => &[],
    }
}

pub fn supports_model_profile(model_id: &str, engine_id: &str, profile: &str) -> bool {
    match (model_id, engine_id) {
        ("whisper-small-q5_1" | "whisper-large-v3-turbo-q5_0", "whisper.cpp") => {
            profile == ENGINE_PROFILE
        }
        ("sensevoice-small-int8", "sherpa-onnx") => is_sensevoice_profile(profile),
        ("parakeet-tdt-v3-int8", "sherpa-onnx") => profile == PARAKEET_PROFILE,
        ("qwen3-asr-0.6b-int8", "sherpa-onnx") => profile == QWEN_ASR_PROFILE,
        _ => false,
    }
}

pub fn model_artifact_matches_profile(profile: &str, sha256: &str, size: u64) -> bool {
    match profile {
        SENSEVOICE_TILED_PROFILE => {
            sha256 == SENSEVOICE_TILED_SHA256 && size == SENSEVOICE_TILED_BYTES
        }
        ENGINE_PROFILE | SENSEVOICE_PROFILE | PARAKEET_PROFILE | QWEN_ASR_PROFILE => true,
        _ => false,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum QualityFlag {
    NeedsReview,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    Hello {
        protocol_version: u32,
        build_id: String,
        backend: String,
        engine_id: String,
    },
    LoadModel {
        request_id: Uuid,
        model_id: String,
        engine_id: String,
        profile_id: String,
        bundle_sha256: String,
        tokens: Option<Box<ModelFile>>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        auxiliary: Vec<ModelAsset>,
        path: String,
        sha256: String,
        size_bytes: u64,
        vad_path: String,
        vad_sha256: String,
        vad_size_bytes: u64,
    },
    ModelReady {
        request_id: Uuid,
        model_id: String,
        backend: String,
        engine_id: String,
        profile_id: String,
        bundle_sha256: String,
    },
    Transcribe {
        session_id: Uuid,
        profile_id: String,
        language: String,
        audio_samples: u32,
        dictionary_revision: i64,
        dictionary_terms: Vec<String>,
    },
    Cancel {
        session_id: Uuid,
    },
    InputAccepted {
        session_id: Uuid,
        sample_count: u32,
    },
    Progress {
        session_id: Uuid,
        stage: String,
        progress: Option<f32>,
    },
    Completed {
        session_id: Uuid,
        text: String,
        language: String,
        asr_ms: u64,
        dictionary_revision: i64,
        prompt_terms_used: u32,
        count_omitted: u32,
        quality_flags: Vec<QualityFlag>,
    },
    Failed {
        session_id: Option<Uuid>,
        request_id: Option<Uuid>,
        code: String,
    },
    Cancelled {
        session_id: Uuid,
    },
    Shutdown {
        request_id: Uuid,
    },
}

impl Control {
    fn kind(&self) -> u8 {
        match self {
            Self::Hello { .. } => 0,
            Self::LoadModel { .. } => 1,
            Self::ModelReady { .. } => 2,
            Self::Transcribe { .. } => 3,
            Self::Cancel { .. } => 5,
            Self::Progress { .. } => 6,
            Self::Completed { .. } => 7,
            Self::Failed { .. } => 8,
            Self::Cancelled { .. } => 9,
            Self::Shutdown { .. } => 10,
            Self::InputAccepted { .. } => 11,
        }
    }
    fn validate(&self) -> io::Result<()> {
        match self {
            Self::LoadModel {
                model_id,
                engine_id,
                profile_id,
                bundle_sha256,
                tokens,
                auxiliary,
                path,
                sha256,
                size_bytes,
                vad_path,
                vad_sha256,
                vad_size_bytes,
                ..
            } => {
                if !supports_model_profile(model_id, engine_id, profile_id)
                    || !model_artifact_matches_profile(profile_id, sha256, *size_bytes)
                    || !valid_sha256(bundle_sha256)
                    || if engine_id == "sherpa-onnx" && profile_id != QWEN_ASR_PROFILE {
                        tokens.as_ref().is_none_or(|t| !t.valid(4 * 1024 * 1024))
                    } else {
                        tokens.is_some()
                    }
                    || !std::path::Path::new(path).is_absolute()
                    || !std::path::Path::new(vad_path).is_absolute()
                    || path.len() > 4096
                    || vad_path.len() > 4096
                    || path.contains('\0')
                    || vad_path.contains('\0')
                    || !valid_sha256(sha256)
                    || !valid_sha256(vad_sha256)
                    || !(1..=2 * 1024 * 1024 * 1024).contains(size_bytes)
                    || !(1..=20 * 1024 * 1024).contains(vad_size_bytes)
                {
                    return Err(invalid("Invalid model profile"));
                }
                let names = auxiliary_names(profile_id);
                let parent = std::path::Path::new(path).parent();
                if auxiliary.len() != names.len()
                    || names.iter().any(|(role, name)| {
                        let found: Vec<_> = auxiliary.iter().filter(|a| a.role == *role).collect();
                        found.len() != 1
                            || !found[0].file.valid(2 * 1024 * 1024 * 1024)
                            || std::path::Path::new(&found[0].file.path).parent() != parent
                            || std::path::Path::new(&found[0].file.path)
                                .file_name()
                                .and_then(|n| n.to_str())
                                != Some(*name)
                            || (!name.ends_with(".onnx")
                                && found[0].file.size_bytes > 4 * 1024 * 1024)
                    })
                {
                    return Err(invalid("Invalid auxiliary model files"));
                }
            }
            Self::Transcribe {
                profile_id,
                audio_samples,
                dictionary_terms,
                dictionary_revision,
                language,
                ..
            } => {
                if !(profile_id == ENGINE_PROFILE
                    || is_sensevoice_profile(profile_id)
                    || profile_id == PARAKEET_PROFILE
                    || profile_id == QWEN_ASR_PROFILE)
                    || *audio_samples < 4000
                    || *audio_samples as usize > MAX_SAMPLES
                    || *dictionary_revision < 1
                    || !(if is_sensevoice_profile(profile_id) {
                        &["auto", "zh", "en", "yue", "ja", "ko"][..]
                    } else {
                        &["auto", "zh", "en"][..]
                    })
                    .contains(&language.as_str())
                    || dictionary_terms.len() > 200
                    || dictionary_terms.iter().map(String::len).sum::<usize>() > 32 * 1024
                    || dictionary_terms.iter().any(|s| {
                        s.trim() != s
                            || s.is_empty()
                            || s.chars().count() > 40
                            || s.chars().any(char::is_control)
                            || s.len() > 160
                    })
                {
                    return Err(invalid("Invalid transcription profile"));
                }
            }
            Self::Completed { text, .. }
                if text.is_empty() || text.len() > 16_384 || text.contains('\0') =>
            {
                return Err(invalid("Invalid result text"))
            }
            Self::Progress {
                progress: Some(p), ..
            } if !p.is_finite() || !(0.0..=1.0).contains(p) => {
                return Err(invalid("Invalid progress"))
            }
            _ => {}
        }
        Ok(())
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Debug)]
pub enum Frame {
    Control(Control),
    Audio { session_id: Uuid, samples: Vec<f32> },
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn write_control(writer: &mut impl Write, message: &Control) -> io::Result<()> {
    message.validate()?;
    let bytes = serde_json::to_vec(message).map_err(|_| invalid("Invalid control message"))?;
    if bytes.len() > MAX_CONTROL_BYTES {
        return Err(invalid("Control frame too large"));
    }
    writer.write_all(&((bytes.len() + 1) as u32).to_le_bytes())?;
    writer.write_all(&[message.kind()])?;
    writer.write_all(&bytes)?;
    writer.flush()
}

pub fn write_audio(writer: &mut impl Write, session_id: Uuid, samples: &[f32]) -> io::Result<()> {
    if samples.is_empty() || samples.len() > MAX_SAMPLES || samples.iter().any(|s| !s.is_finite()) {
        return Err(invalid("Invalid audio samples"));
    }
    writer.write_all(&((21 + samples.len() * 4) as u32).to_le_bytes())?;
    writer.write_all(&[AUDIO_KIND])?;
    writer.write_all(session_id.as_bytes())?;
    writer.write_all(&(samples.len() as u32).to_le_bytes())?;
    let mut block = [0u8; 16_384];
    for chunk in samples.chunks(4096) {
        for (sample, bytes) in chunk.iter().zip(block.chunks_exact_mut(4)) {
            bytes.copy_from_slice(&sample.to_le_bytes());
        }
        writer.write_all(&block[..chunk.len() * 4])?;
    }
    writer.flush()
}

pub fn read_frame(reader: &mut impl Read) -> io::Result<Frame> {
    read_frame_checked(reader, |_, _| Ok(()))
}

/// Check declaration/order before allocating the PCM payload. The worker uses
/// this hook to reject unsolicited audio, not merely validate it after reading.
pub fn read_frame_checked(
    reader: &mut impl Read,
    check_audio: impl FnOnce(Uuid, usize) -> io::Result<()>,
) -> io::Result<Frame> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let length = u32::from_le_bytes(header) as usize;
    if !(1..=MAX_FRAME_BYTES).contains(&length) {
        return Err(invalid("Frame exceeds envelope"));
    }
    let mut kind = [0];
    reader.read_exact(&mut kind)?;
    if kind[0] != AUDIO_KIND {
        if kind[0] > 11 || length - 1 > MAX_CONTROL_BYTES {
            return Err(invalid("Invalid control header"));
        }
        let mut bytes = vec![0; length - 1];
        reader.read_exact(&mut bytes)?;
        let control: Control =
            serde_json::from_slice(&bytes).map_err(|_| invalid("Malformed control message"))?;
        if control.kind() != kind[0] {
            return Err(invalid("Message kind mismatch"));
        }
        control.validate()?;
        return Ok(Frame::Control(control));
    }
    if !(25..=21 + MAX_SAMPLES * 4).contains(&length) {
        return Err(invalid("Invalid audio frame size"));
    }
    let mut id = [0; 16];
    reader.read_exact(&mut id)?;
    reader.read_exact(&mut header)?;
    let count = u32::from_le_bytes(header) as usize;
    if count == 0
        || count > MAX_SAMPLES
        || count.checked_mul(4).and_then(|n| n.checked_add(21)) != Some(length)
    {
        return Err(invalid("Sample count mismatch"));
    }
    check_audio(Uuid::from_bytes(id), count)?;
    let mut samples = Vec::with_capacity(count);
    let mut block = [0u8; 16_384];
    while samples.len() < count {
        let n = (count - samples.len()).min(4096);
        reader.read_exact(&mut block[..n * 4])?;
        for bytes in block[..n * 4].chunks_exact(4) {
            let sample = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            if !sample.is_finite() {
                return Err(invalid("Non-finite audio"));
            }
            samples.push(sample);
        }
    }
    Ok(Frame::Audio {
        session_id: Uuid::from_bytes(id),
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn additional_models_require_their_complete_same_directory_assets() {
        for (id, profile) in [
            ("parakeet-tdt-v3-int8", PARAKEET_PROFILE),
            ("qwen3-asr-0.6b-int8", QWEN_ASR_PROFILE),
        ] {
            let file = |name: &str| ModelFile {
                path: format!("/tmp/bundle/{name}"),
                sha256: "a".repeat(64),
                size_bytes: 100,
            };
            let model = file("encoder.int8.onnx");
            let vad = file("vad.onnx");
            let load = Control::LoadModel {
                request_id: Uuid::new_v4(),
                model_id: id.into(),
                engine_id: "sherpa-onnx".into(),
                profile_id: profile.into(),
                bundle_sha256: "b".repeat(64),
                tokens: (profile == PARAKEET_PROFILE).then(|| Box::new(file("tokens.txt"))),
                auxiliary: auxiliary_names(profile)
                    .iter()
                    .map(|(role, name)| ModelAsset {
                        role: (*role).into(),
                        file: file(name),
                    })
                    .collect(),
                path: model.path,
                sha256: model.sha256,
                size_bytes: model.size_bytes,
                vad_path: vad.path,
                vad_sha256: vad.sha256,
                vad_size_bytes: vad.size_bytes,
            };
            write_control(&mut std::io::sink(), &load).unwrap();
            for variant in 0..4 {
                let mut broken = load.clone();
                if let Control::LoadModel { auxiliary, .. } = &mut broken {
                    match variant {
                        0 => {
                            auxiliary.pop();
                        }
                        1 => {
                            auxiliary[0].file.path = "/tmp/outside/decoder.int8.onnx".into();
                        }
                        2 => {
                            auxiliary[0].role = "arbitrary".into();
                        }
                        _ => {
                            auxiliary.push(auxiliary[0].clone());
                        }
                    }
                }
                assert!(write_control(&mut std::io::sink(), &broken).is_err());
            }
        }
    }
    #[test]
    fn tiled_profile_requires_its_exact_graph_and_sensevoice_adapter() {
        let path = |name| std::env::temp_dir().join(name).to_str().unwrap().to_owned();
        let load = Control::LoadModel {
            request_id: Uuid::new_v4(),
            model_id: "sensevoice-small-int8".into(),
            engine_id: "sherpa-onnx".into(),
            profile_id: SENSEVOICE_TILED_PROFILE.into(),
            bundle_sha256: "1".repeat(64),
            tokens: Some(Box::new(ModelFile {
                path: path("tokens.txt"),
                sha256: "2".repeat(64),
                size_bytes: 100,
            })),
            auxiliary: Vec::new(),
            path: path("model.tiled.onnx"),
            sha256: SENSEVOICE_TILED_SHA256.into(),
            size_bytes: SENSEVOICE_TILED_BYTES,
            vad_path: path("vad.onnx"),
            vad_sha256: "3".repeat(64),
            vad_size_bytes: 100,
        };
        let mut encoded = Vec::new();
        write_control(&mut encoded, &load).unwrap();
        assert!(
            matches!(read_frame(&mut encoded.as_slice()).unwrap(), Frame::Control(c) if c == load)
        );
        for which in 0..6 {
            let mut invalid = load.clone();
            if let Control::LoadModel {
                model_id,
                engine_id,
                profile_id,
                sha256,
                size_bytes,
                tokens,
                ..
            } = &mut invalid
            {
                match which {
                    0 => *model_id = "whisper-small-q5_1".into(),
                    1 => *engine_id = "whisper.cpp".into(),
                    2 => *profile_id = "sensevoice-tiled128-v1".into(),
                    3 => *sha256 = "4".repeat(64),
                    4 => *size_bytes -= 1,
                    _ => *tokens = None,
                }
            }
            assert!(
                write_control(&mut Vec::new(), &invalid).is_err(),
                "case {which}"
            );
        }
    }

    #[test]
    fn both_sensevoice_profiles_keep_five_minute_and_language_bounds() {
        for profile in [SENSEVOICE_PROFILE, SENSEVOICE_TILED_PROFILE] {
            for language in ["auto", "zh", "en", "yue", "ja", "ko", "fr"] {
                for count in [3999, 4000, MAX_SAMPLES, MAX_SAMPLES + 1] {
                    let request = Control::Transcribe {
                        session_id: Uuid::new_v4(),
                        profile_id: profile.into(),
                        language: language.into(),
                        audio_samples: count as u32,
                        dictionary_revision: 1,
                        dictionary_terms: vec![],
                    };
                    assert_eq!(
                        write_control(&mut Vec::new(), &request).is_ok(),
                        language != "fr" && (4000..=MAX_SAMPLES).contains(&count)
                    );
                }
            }
        }
    }

    #[test]
    fn five_minutes_round_trip_without_truncation() {
        let id = Uuid::new_v4();
        let mut input = vec![0.25; MAX_SAMPLES];
        input[0] = -0.5;
        input[MAX_SAMPLES - 1] = 0.75;
        let mut bytes = Vec::new();
        write_audio(&mut bytes, id, &input).unwrap();
        assert_eq!(bytes.len(), 19_200_025);
        match read_frame(&mut bytes.as_slice()).unwrap() {
            Frame::Audio {
                session_id,
                samples,
            } => {
                assert_eq!(session_id, id);
                assert_eq!(samples, input);
            }
            _ => panic!("wrong frame"),
        }
    }
    #[test]
    fn bad_envelope_is_rejected_before_reading_payload() {
        assert_eq!(
            read_frame(&mut (u32::MAX.to_le_bytes().as_slice()))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
    #[test]
    fn mismatched_sample_count_is_rejected() {
        let mut bytes = Vec::new();
        write_audio(&mut bytes, Uuid::new_v4(), &[0.1, 0.2]).unwrap();
        bytes[21..25].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(
            read_frame(&mut bytes.as_slice()).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
    #[test]
    fn rejects_nan_and_oversized_control() {
        assert!(write_audio(&mut Vec::new(), Uuid::new_v4(), &[f32::NAN]).is_err());
        let c = Control::Completed {
            session_id: Uuid::new_v4(),
            text: "a".repeat(16385),
            language: "en".into(),
            asr_ms: 1,
            dictionary_revision: 1,
            prompt_terms_used: 0,
            count_omitted: 0,
            quality_flags: vec![],
        };
        assert!(write_control(&mut Vec::new(), &c).is_err());
    }
    #[test]
    fn control_round_trip_and_kind_spoofing() {
        let c = Control::Cancel {
            session_id: Uuid::new_v4(),
        };
        let mut bytes = Vec::new();
        write_control(&mut bytes, &c).unwrap();
        match read_frame(&mut bytes.as_slice()).unwrap() {
            Frame::Control(v) => assert_eq!(v, c),
            _ => panic!(),
        };
        bytes[4] = 0;
        assert!(read_frame(&mut bytes.as_slice()).is_err());
    }
}
