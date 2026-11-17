use crate::cancellation::Cancellation;
use crate::model_file::verify_file;
#[cfg(test)]
use sha2::{Digest, Sha256};
pub const ENGINE_ID: &str = "whisper.cpp";
use speakmore_protocol::{Control, QualityFlag};
use std::time::Instant;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperVadContext,
    WhisperVadContextParams, WhisperVadParams,
};

pub type EngineResult<T> = Result<T, &'static str>;

pub struct Engine {
    context: WhisperContext,
    vad: WhisperVadContext,
    threads: i32,
    gpu: bool,
}

impl Engine {
    pub fn load(spec: &Control, gpu: bool, cancellation: &Cancellation) -> EngineResult<Self> {
        let Control::LoadModel {
            engine_id,
            path,
            sha256,
            size_bytes,
            vad_path,
            vad_sha256,
            vad_size_bytes,
            ..
        } = spec
        else {
            return Err("InvalidModelProfile");
        };
        if engine_id != ENGINE_ID {
            return Err("InvalidModelProfile");
        }
        verify_file(path, *size_bytes, sha256, cancellation)?;
        verify_file(vad_path, *vad_size_bytes, vad_sha256, cancellation)?;
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8)) as i32;
        let mut params = WhisperContextParameters::default();
        params.use_gpu(gpu);
        let context =
            WhisperContext::new_with_params(path, params).map_err(|_| "ModelLoadFailed")?;
        cancellation.check()?;
        if !context.is_multilingual() {
            return Err("InvalidModelProfile");
        }
        if gpu {
            // Verify real backend initialization before claiming ModelReady.
            // The temporary state is dropped; sessions always get a fresh state.
            drop(crate::backend::create_state(&context, true)?);
            cancellation.check()?;
        }
        let mut params = WhisperVadContextParams::default();
        params.set_n_threads(threads);
        // VAD remains CPU on every build; this small model does not need a GPU.
        params.set_use_gpu(false);
        let vad = WhisperVadContext::new(vad_path, params).map_err(|_| "VadLoadFailed")?;
        Ok(Self {
            context,
            vad,
            threads,
            gpu,
        })
    }

    pub fn transcribe(
        &mut self,
        request: &Control,
        samples: &[f32],
        started: Instant,
        cancellation: &Cancellation,
        mut stage: impl FnMut(&'static str),
    ) -> EngineResult<Control> {
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
        if profile_id != speakmore_protocol::ENGINE_PROFILE {
            return Err("InvalidTranscriptionProfile");
        }
        cancellation.check()?;
        stage("detecting_speech");
        let mut vad_params = WhisperVadParams::new();
        vad_params.set_threshold(0.5);
        vad_params.set_min_speech_duration(150);
        vad_params.set_min_silence_duration(300);
        // Add padding once to the outside bounds, never concatenate speech.
        vad_params.set_speech_pad(0);
        let segments = self
            .vad
            .segments_from_samples(vad_params, samples)
            .map_err(|_| "VadFailed")?;
        cancellation.check()?;
        let mut first = f32::INFINITY;
        let mut last = 0f32;
        for segment in segments {
            if !segment.start.is_finite()
                || !segment.end.is_finite()
                || segment.start < 0.0
                || segment.end < segment.start
            {
                return Err("VadFailed");
            }
            first = first.min(segment.start);
            last = last.max(segment.end);
        }
        let (begin, end) = speech_bounds(first, last, samples.len()).ok_or("NoSpeech")?;
        let (tokens, used) = build_prompt(dictionary_terms, |text| {
            self.context
                .tokenize(text, 1024)
                .map_err(|_| "TokenizerFailed")
        })?;
        let mut state = crate::backend::create_state(&self.context, self.gpu)?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_translate(false);
        params.set_temperature(0.0);
        params.set_temperature_inc(0.0);
        // Each session has its own state. Keep intra-recording context for the
        // engine's long-audio windows and dictionary; never share past sessions.
        params.set_no_context(false);
        params.set_language(Some(language));
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_debug_mode(false);
        params.set_vad_model_path(None);
        if !tokens.is_empty() {
            params.set_tokens(&tokens);
        }
        cancellation.check()?;
        stage("decoding");
        let decoded = crate::whisper_cancellation::decode(
            &mut state,
            params,
            &samples[begin..end],
            cancellation,
        );
        cancellation.check()?;
        decoded.map_err(|_| "DecodeFailed")?;
        let mut text = String::new();
        let mut needs_review = false;
        for segment in state.as_iter() {
            let value = segment.to_str().map_err(|_| "InvalidResultText")?;
            if text.len() + value.len() > 16384 {
                return Err("ResultTooLong");
            }
            text.push_str(value);
            // Initial conservative quality signal, subject to the corpus gate.
            let probability = segment.no_speech_probability();
            needs_review |= !probability.is_finite() || probability >= 0.6;
            let mut text_tokens = 0;
            let mut log_probability = 0.0f64;
            for index in 0..segment.n_tokens() {
                let token = segment.get_token(index).ok_or("InvalidResultText")?;
                if token.token_id() >= self.context.token_eot() {
                    continue;
                }
                let probability = token.token_probability();
                if !probability.is_finite() || probability <= 0.0 || probability > 1.0 {
                    needs_review = true;
                } else {
                    text_tokens += 1;
                    log_probability += f64::from(probability).ln();
                }
            }
            needs_review |= text_tokens == 0 || log_probability / f64::from(text_tokens) < -1.0;
        }
        let text = text.trim().to_owned();
        if text.is_empty() || text.contains('\0') {
            return Err("NoSpeech");
        }
        let language = whisper_rs::get_lang_str(state.full_lang_id_from_state())
            .ok_or("InvalidResultLanguage")?
            .to_owned();
        Ok(Control::Completed {
            session_id: *session_id,
            text,
            language,
            asr_ms: started.elapsed().as_millis() as u64,
            dictionary_revision: *dictionary_revision,
            prompt_terms_used: used as u32,
            count_omitted: (dictionary_terms.len() - used) as u32,
            quality_flags: if needs_review {
                vec![QualityFlag::NeedsReview]
            } else {
                vec![]
            },
        })
    }
}

fn speech_bounds(first_cs: f32, last_cs: f32, count: usize) -> Option<(usize, usize)> {
    if !first_cs.is_finite() || !last_cs.is_finite() || last_cs <= first_cs {
        return None;
    }
    let begin = ((first_cs * 160.0).floor() as usize)
        .saturating_sub(3200)
        .min(count);
    let end = ((last_cs * 160.0).ceil() as usize)
        .saturating_add(3200)
        .min(count);
    (begin < end).then_some((begin, end))
}

fn build_prompt(
    terms: &[String],
    mut tokenize: impl FnMut(&str) -> EngineResult<Vec<i32>>,
) -> EngineResult<(Vec<i32>, usize)> {
    let mut text = String::new();
    let mut tokens = Vec::new();
    let mut used = 0;
    for term in terms {
        let candidate = if text.is_empty() {
            term.clone()
        } else {
            format!("{text}, {term}")
        };
        let candidate_tokens = tokenize(&candidate)?;
        if candidate_tokens.len() <= 128 {
            text = candidate;
            tokens = candidate_tokens;
            used += 1;
        }
    }
    Ok((tokens, used))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn padding_keeps_internal_pauses_and_clamps_to_audio() {
        assert_eq!(speech_bounds(10.0, 900.0, 160000), Some((0, 147200)));
        assert_eq!(speech_bounds(0.0, 30000.0, 4800000), Some((0, 4800000)));
        assert_eq!(speech_bounds(f32::INFINITY, 0.0, 16000), None);
    }
    #[test]
    fn dictionary_skips_whole_terms_then_tries_later_terms() {
        let terms = vec!["x".repeat(124), "large".into(), "ok".into()];
        let (tokens, used) =
            build_prompt(&terms, |s| Ok(s.bytes().map(i32::from).collect())).unwrap();
        assert_eq!(used, 2);
        assert_eq!(tokens.len(), 128);
        assert!(tokens.ends_with(&[111, 107]));
    }
    #[test]
    fn model_verification_rejects_changed_bytes_and_wrong_size() {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"abc").unwrap();
        let path = file.path().to_str().unwrap();
        let hash = format!("{:x}", Sha256::digest(b"abc"));
        let cancel = Cancellation::default();
        assert!(verify_file(path, 3, &hash, &cancel).is_ok());
        assert_eq!(
            verify_file(path, 4, &hash, &cancel),
            Err("ModelSizeMismatch")
        );
        assert_eq!(
            verify_file(path, 3, &"0".repeat(64), &cancel),
            Err("ModelHashMismatch")
        );
        cancel.cancel(crate::cancellation::USER);
        assert_eq!(verify_file(path, 3, &hash, &cancel), Err("Cancelled"));
    }
}
