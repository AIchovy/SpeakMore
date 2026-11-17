use crate::{cancellation::Cancellation, model_file::verify_file};
use speakmore_protocol::{
    is_sensevoice_profile, model_artifact_matches_profile, supports_model_profile, Control,
    QualityFlag,
};
use speakmore_sensevoice::{Recognizer, Vad};
use std::{path::Path, time::Instant};

pub const ENGINE_ID: &str = "sherpa-onnx";
type Result<T> = std::result::Result<T, &'static str>;
pub struct Engine {
    recognizer: Recognizer,
    vad: Vad,
    profile_id: String,
}
impl Engine {
    pub fn load(spec: &Control, gpu: bool, cancellation: &Cancellation) -> Result<Self> {
        let Control::LoadModel {
            model_id,
            engine_id,
            profile_id,
            path,
            sha256,
            size_bytes,
            tokens,
            auxiliary,
            vad_path,
            vad_sha256,
            vad_size_bytes,
            ..
        } = spec
        else {
            return Err("InvalidModelProfile");
        };
        if gpu
            || engine_id != ENGINE_ID
            || !supports_model_profile(model_id, engine_id, profile_id)
            || !model_artifact_matches_profile(profile_id, sha256, *size_bytes)
        {
            return Err("InvalidModelProfile");
        }
        verify_file(path, *size_bytes, sha256, cancellation)?;
        if let Some(tokens) = tokens {
            verify_file(
                &tokens.path,
                tokens.size_bytes,
                &tokens.sha256,
                cancellation,
            )?;
        }
        for a in auxiliary {
            verify_file(
                &a.file.path,
                a.file.size_bytes,
                &a.file.sha256,
                cancellation,
            )?;
        }
        verify_file(vad_path, *vad_size_bytes, vad_sha256, cancellation)?;
        // Fixed profile, measured separately from Whisper's thread policy.
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(4));
        let assets: Vec<_> = auxiliary
            .iter()
            .map(|a| (a.role.as_str(), Path::new(&a.file.path)))
            .collect();
        let recognizer = Recognizer::load_profile(
            Path::new(path),
            tokens.as_ref().map(|t| Path::new(&t.path)),
            &assets,
            threads,
            profile_id,
        )?;
        cancellation.check()?;
        let vad = Vad::load(Path::new(vad_path))?;
        cancellation.check()?;
        Ok(Self {
            recognizer,
            vad,
            profile_id: profile_id.clone(),
        })
    }
    pub fn transcribe(
        &mut self,
        request: &Control,
        samples: &[f32],
        started: Instant,
        cancellation: &Cancellation,
        mut stage: impl FnMut(&'static str),
    ) -> Result<Control> {
        let Control::Transcribe {
            session_id,
            profile_id,
            language,
            dictionary_revision,
            dictionary_terms,
            ..
        } = request
        else {
            return Err("InvalidTranscriptionProfile");
        };
        if profile_id != &self.profile_id {
            return Err("InvalidTranscriptionProfile");
        }
        cancellation.check()?;
        stage("detecting_speech");
        let (begin, end) = self
            .vad
            .speech_bounds(samples, || cancellation.check())?
            .ok_or("NoSpeech")?;
        if end - begin < 4000 {
            return Err("NoSpeech");
        }
        stage("decoding");
        // The pinned C API has no cooperative ONNX cancellation. Control and
        // watchdog threads remain live; a cancelled call is reaped within 1 s.
        let output = if is_sensevoice_profile(profile_id) {
            self.recognizer.decode(&samples[begin..end], language)?
        } else {
            let mut combined = speakmore_sensevoice::Transcript {
                text: String::new(),
                language: "auto".into(),
                event: String::new(),
                tokens: Vec::new(),
            };
            // Keep attention and decoder KV bounded for the entire five-minute recording.
            // Prefer a quiet boundary near 20 s; preserve every input sample once.
            for (start, stop) in chunk_ranges(&samples[begin..end]) {
                cancellation.check()?;
                let part = self
                    .recognizer
                    .decode(&samples[begin + start..begin + stop], "auto")?;
                let part = part.text.trim();
                if !part.is_empty() {
                    if !combined.text.is_empty() {
                        combined.text.push(' ');
                    }
                    combined.text.push_str(part);
                }
                if combined.text.len() > 16_384 {
                    return Err("ResultTooLong");
                }
            }
            combined
        };
        cancellation.check()?;
        let text = output.text.trim().to_string();
        if text.is_empty() {
            return Err("NoSpeech");
        }
        if text.contains('\0') || text.contains("<|") || text.contains("|>") {
            return Err("InvalidResultText");
        }
        if is_sensevoice_profile(profile_id)
            && !["zh", "en", "yue", "ja", "ko"].contains(&output.language.as_str())
        {
            return Err("InvalidResultLanguage");
        }
        Ok(Control::Completed {
            session_id: *session_id,
            text,
            language: output.language,
            asr_ms: started.elapsed().as_millis() as u64,
            dictionary_revision: *dictionary_revision,
            prompt_terms_used: 0,
            count_omitted: dictionary_terms.len() as u32,
            quality_flags: if is_sensevoice_profile(profile_id) {
                vec![QualityFlag::NeedsReview]
            } else {
                Vec::new()
            },
        })
    }
}

fn chunk_ranges(samples: &[f32]) -> Vec<(usize, usize)> {
    const LIMIT: usize = 20 * 16_000;
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < samples.len() {
        let mut stop = (start + LIMIT).min(samples.len());
        if stop < samples.len() {
            // Search backwards for 100 ms of quiet in the last three seconds.
            for candidate in ((stop - 3 * 16_000)..=stop).rev().step_by(1600) {
                if samples[candidate - 1600..candidate]
                    .iter()
                    .map(|x| x * x)
                    .sum::<f32>()
                    / 1600.0
                    < 0.0001
                {
                    stop = candidate;
                    break;
                }
            }
        }
        if samples.len() - stop < 4000 {
            stop = samples.len();
        }
        ranges.push((start, stop));
        start = stop;
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunking_preserves_all_samples_and_prefers_silence() {
        let mut audio = vec![0.3; 65 * 16_000];
        audio[18 * 16_000..18 * 16_000 + 1600].fill(0.0);
        let ranges = chunk_ranges(&audio);
        assert_eq!(ranges[0].1, 18 * 16_000 + 1600);
        assert_eq!(ranges.first().unwrap().0, 0);
        assert_eq!(ranges.last().unwrap().1, audio.len());
        assert!(ranges.windows(2).all(|p| p[0].1 == p[1].0));
        assert!(ranges
            .iter()
            .all(|(a, b)| b - a <= 20 * 16_000 + 4000 && b - a >= 4000));
    }
}
