use std::{
    ffi::{CStr, CString},
    marker::PhantomData,
    path::Path,
    ptr::NonNull,
    rc::Rc,
};
#[allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    dead_code,
    clippy::all
)]
mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

type Result<T> = std::result::Result<T, &'static str>;
#[derive(Debug)]
pub struct Token {
    pub text: String,
    pub seconds: f32,
}
#[derive(Debug)]
pub struct Transcript {
    pub text: String,
    pub language: String,
    pub event: String,
    pub tokens: Vec<Token>,
}
/// Deliberately !Send and !Sync: create/use/drop on the same worker decoding thread.
pub struct Recognizer {
    ptr: NonNull<ffi::SherpaOnnxOfflineRecognizer>,
    config: ffi::SherpaOnnxOfflineRecognizerConfig,
    _strings: Vec<CString>,
    profile: String,
    _owner: PhantomData<Rc<()>>,
}
impl Drop for Recognizer {
    fn drop(&mut self) {
        unsafe { ffi::SherpaOnnxDestroyOfflineRecognizer(self.ptr.as_ptr()) }
    }
}
struct Stream(NonNull<ffi::SherpaOnnxOfflineStream>);
impl Drop for Stream {
    fn drop(&mut self) {
        unsafe { ffi::SherpaOnnxDestroyOfflineStream(self.0.as_ptr()) }
    }
}
struct NativeResult(NonNull<ffi::SherpaOnnxOfflineRecognizerResult>);
impl Drop for NativeResult {
    fn drop(&mut self) {
        unsafe { ffi::SherpaOnnxDestroyOfflineRecognizerResult(self.0.as_ptr()) }
    }
}
// Pointers originate only from the matching verified C library and live RAII result.
unsafe fn string(ptr: *const std::ffi::c_char, max: usize) -> Result<String> {
    if ptr.is_null() {
        return Ok(String::new());
    }
    let value = unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| "InvalidResultText")?;
    if value.len() > max {
        return Err("ResultTooLong");
    }
    Ok(value.to_owned())
}
pub fn versions() -> Result<(String, String)> {
    unsafe {
        Ok((
            string(ffi::SherpaOnnxGetVersionStr(), 80)?,
            string(ffi::SherpaOnnxGetOnnxruntimeVersionStr(), 80)?,
        ))
    }
}
impl Recognizer {
    /// Call only after the worker verifies both model and tokens against its trusted bundle.
    pub fn load(model: &Path, tokens: &Path, threads: usize) -> Result<Self> {
        Self::load_profile(model, Some(tokens), &[], threads, "sensevoice-v1")
    }
    pub fn load_profile(
        model: &Path,
        tokens: Option<&Path>,
        auxiliary: &[(&str, &Path)],
        threads: usize,
        profile: &str,
    ) -> Result<Self> {
        let (sherpa, ort) = versions()?;
        if sherpa != env!("SPEAKMORE_SHERPA_VERSION") || ort != env!("SPEAKMORE_ORT_VERSION") {
            return Err("NativeVersionMismatch");
        }
        if !model.is_absolute()
            || tokens.is_some_and(|p| !p.is_absolute())
            || auxiliary.iter().any(|(_, p)| !p.is_absolute())
            || !(1..=8).contains(&threads)
        {
            return Err("InvalidModelProfile");
        }
        let model_path = model;
        let model = CString::new(model.to_str().ok_or("InvalidModelPath")?)
            .map_err(|_| "InvalidModelPath")?;
        let tokens = CString::new(tokens.map_or(Ok(""), |p| p.to_str().ok_or("InvalidModelPath"))?)
            .map_err(|_| "InvalidModelPath")?;
        let mut owned = vec![model, tokens];
        let mut extra = Vec::new();
        for (role, path) in auxiliary {
            owned.push(
                CString::new(path.to_str().ok_or("InvalidModelPath")?)
                    .map_err(|_| "InvalidModelPath")?,
            );
            extra.push((*role, owned.last().unwrap().as_ptr()));
        }
        let required = |role: &str| {
            extra
                .iter()
                .find(|(r, _)| *r == role)
                .map(|(_, p)| *p)
                .ok_or("InvalidModelProfile")
        };
        let mut config = ffi::SherpaOnnxOfflineRecognizerConfig::default();
        config.feat_config.sample_rate = 16_000;
        config.feat_config.feature_dim = 80;
        config.model_config.num_threads = threads as i32;
        config.model_config.provider = c"cpu".as_ptr();
        config.model_config.tokens = owned[1].as_ptr();
        match profile {
            "sensevoice-v1" | "sensevoice-tiled256-v1" => {
                config.model_config.model_type = c"sense_voice".as_ptr();
                config.model_config.sense_voice.model = owned[0].as_ptr();
                config.model_config.sense_voice.language = c"auto".as_ptr();
                config.model_config.sense_voice.use_itn = 1;
            }
            "parakeet-tdt-v3-int8-v1" => {
                config.model_config.model_type = c"nemo_transducer".as_ptr();
                config.model_config.transducer.encoder = owned[0].as_ptr();
                config.model_config.transducer.decoder = required("decoder")?;
                config.model_config.transducer.joiner = required("joiner")?;
            }
            "qwen3-asr-0.6b-int8-v1" => {
                owned.push(
                    CString::new(
                        model_path
                            .parent()
                            .ok_or("InvalidModelPath")?
                            .to_str()
                            .ok_or("InvalidModelPath")?,
                    )
                    .map_err(|_| "InvalidModelPath")?,
                );
                config.model_config.tokens = std::ptr::null();
                config.model_config.qwen3_asr.encoder = owned[0].as_ptr();
                config.model_config.qwen3_asr.decoder = required("decoder")?;
                config.model_config.qwen3_asr.conv_frontend = required("frontend")?;
                config.model_config.qwen3_asr.tokenizer = owned.last().unwrap().as_ptr();
                config.model_config.qwen3_asr.max_total_len = 1024;
                config.model_config.qwen3_asr.max_new_tokens = 512;
                config.model_config.qwen3_asr.temperature = 0.0;
                config.model_config.qwen3_asr.top_p = 1.0;
                config.model_config.qwen3_asr.seed = 42;
            }
            _ => return Err("InvalidModelProfile"),
        }
        config.decoding_method = c"greedy_search".as_ptr();
        let ptr =
            NonNull::new(unsafe { ffi::SherpaOnnxCreateOfflineRecognizer(&config) }.cast_mut())
                .ok_or("ModelLoadFailed")?;
        Ok(Self {
            ptr,
            config,
            _strings: owned,
            profile: profile.to_owned(),
            _owner: PhantomData,
        })
    }
    /// One bounded invocation. The worker owns speech detection and cancellation.
    pub fn decode(&mut self, samples: &[f32], language: &str) -> Result<Transcript> {
        if samples.len() < 4000
            || samples.len() > 300 * 16_000
            || samples.iter().any(|s| !s.is_finite())
        {
            return Err("InvalidAudio");
        }
        let language = match language {
            "auto" => c"auto",
            "zh" => c"zh",
            "en" => c"en",
            "yue" => c"yue",
            "ja" => c"ja",
            "ko" => c"ko",
            _ => return Err("InvalidLanguage"),
        };
        if self.profile.starts_with("sensevoice-") {
            self.config.model_config.sense_voice.language = language.as_ptr();
            unsafe { ffi::SherpaOnnxOfflineRecognizerSetConfig(self.ptr.as_ptr(), &self.config) };
        }
        let stream = Stream(
            NonNull::new(
                unsafe { ffi::SherpaOnnxCreateOfflineStream(self.ptr.as_ptr()) }.cast_mut(),
            )
            .ok_or("DecodeFailed")?,
        );
        unsafe {
            ffi::SherpaOnnxAcceptWaveformOffline(
                stream.0.as_ptr(),
                16_000,
                samples.as_ptr(),
                samples.len() as i32,
            );
            ffi::SherpaOnnxDecodeOfflineStream(self.ptr.as_ptr(), stream.0.as_ptr());
        }
        let result = NativeResult(
            NonNull::new(
                unsafe { ffi::SherpaOnnxGetOfflineStreamResult(stream.0.as_ptr()) }.cast_mut(),
            )
            .ok_or("DecodeFailed")?,
        );
        let raw = unsafe { result.0.as_ref() };
        if !self.profile.starts_with("sensevoice-") {
            // These adapters do not provide the SenseVoice language/event contract.
            // Do not fabricate detected languages or word timestamps.
            return Ok(Transcript {
                text: unsafe { string(raw.text, 16_384)? },
                language: "auto".into(),
                event: String::new(),
                tokens: Vec::new(),
            });
        }
        if !(0..=8192).contains(&raw.count)
            || (raw.count != 0 && (raw.tokens_arr.is_null() || raw.timestamps.is_null()))
        {
            return Err("InvalidResultText");
        }
        let mut tokens = Vec::with_capacity(raw.count as usize);
        for index in 0..raw.count as usize {
            let token = unsafe { string(*raw.tokens_arr.add(index), 1024)? };
            let seconds = unsafe { *raw.timestamps.add(index) };
            if !seconds.is_finite()
                || seconds < 0.0
                || seconds > samples.len() as f32 / 16_000.0 + 0.5
                || tokens.last().is_some_and(|t: &Token| t.seconds > seconds)
            {
                return Err("InvalidResultTimestamps");
            }
            tokens.push(Token {
                text: token,
                seconds,
            });
        }
        // These are structured special tokens in the pinned upstream API, not
        // ISO language codes. Match exact known values; never strip arbitrary text.
        let language = match unsafe { string(raw.lang, 32)? }.as_str() {
            "<|zh|>" => "zh",
            "<|en|>" => "en",
            "<|yue|>" => "yue",
            "<|ja|>" => "ja",
            "<|ko|>" => "ko",
            _ => return Err("InvalidResultLanguage"),
        }
        .to_owned();
        Ok(Transcript {
            text: unsafe { string(raw.text, 16_384)? },
            language,
            event: unsafe { string(raw.event, 80)? },
            tokens,
        })
    }
}

/// Separate CPU VAD. Only first/last speech bounds leave this owner; detected
/// segments never splice the ASR input. Native buffering is bounded to 301 s.
pub struct Vad {
    ptr: NonNull<ffi::SherpaOnnxVoiceActivityDetector>,
    _model: CString,
    _owner: PhantomData<Rc<()>>,
}
impl Drop for Vad {
    fn drop(&mut self) {
        unsafe { ffi::SherpaOnnxDestroyVoiceActivityDetector(self.ptr.as_ptr()) }
    }
}
struct ResetVad(*const ffi::SherpaOnnxVoiceActivityDetector);
impl Drop for ResetVad {
    fn drop(&mut self) {
        unsafe { ffi::SherpaOnnxVoiceActivityDetectorReset(self.0) }
    }
}
struct Speech(NonNull<ffi::SherpaOnnxSpeechSegment>);
impl Drop for Speech {
    fn drop(&mut self) {
        unsafe { ffi::SherpaOnnxDestroySpeechSegment(self.0.as_ptr()) }
    }
}
impl Vad {
    pub fn load(model: &Path) -> Result<Self> {
        if !model.is_absolute() {
            return Err("InvalidModelPath");
        }
        let model = CString::new(model.to_str().ok_or("InvalidModelPath")?)
            .map_err(|_| "InvalidModelPath")?;
        let mut config = ffi::SherpaOnnxVadModelConfig::default();
        config.silero_vad.model = model.as_ptr();
        config.silero_vad.threshold = 0.5;
        config.silero_vad.min_speech_duration = 0.15;
        config.silero_vad.min_silence_duration = 0.3;
        config.silero_vad.window_size = 512;
        // Prevent the upstream adaptive threshold from changing this profile.
        config.silero_vad.max_speech_duration = 301.0;
        config.sample_rate = 16_000;
        config.num_threads = 1;
        config.provider = c"cpu".as_ptr();
        let ptr = NonNull::new(unsafe {
            ffi::SherpaOnnxCreateVoiceActivityDetector(&config, 301.0).cast_mut()
        })
        .ok_or("VadLoadFailed")?;
        Ok(Self {
            ptr,
            _model: model,
            _owner: PhantomData,
        })
    }
    pub fn speech_bounds(
        &mut self,
        samples: &[f32],
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Option<(usize, usize)>> {
        if !(4000..=4_800_000).contains(&samples.len()) || samples.iter().any(|s| !s.is_finite()) {
            return Err("InvalidAudio");
        }
        let ptr = self.ptr.as_ptr();
        unsafe { ffi::SherpaOnnxVoiceActivityDetectorReset(ptr) };
        let _reset = ResetVad(ptr); // Also clears PCM after cancellation or error.
        let mut first = usize::MAX;
        let mut last = 0;
        for chunk in samples.chunks(512) {
            check()?;
            // The upstream Flush does not evaluate a partial window. Pad only
            // VAD's last window; ASR always sees the original, unpadded samples.
            let mut padded = [0.0; 512];
            padded[..chunk.len()].copy_from_slice(chunk);
            unsafe {
                ffi::SherpaOnnxVoiceActivityDetectorAcceptWaveform(ptr, padded.as_ptr(), 512);
            }
            self.drain_bounds(samples.len(), &mut first, &mut last)?;
        }
        unsafe { ffi::SherpaOnnxVoiceActivityDetectorFlush(ptr) };
        self.drain_bounds(samples.len(), &mut first, &mut last)?;
        check()?;
        if first >= last {
            return Ok(None);
        }
        Ok(Some((
            first.saturating_sub(3200),
            last.saturating_add(3200).min(samples.len()),
        )))
    }
    fn drain_bounds(&self, count: usize, first: &mut usize, last: &mut usize) -> Result<()> {
        let ptr = self.ptr.as_ptr();
        while unsafe { ffi::SherpaOnnxVoiceActivityDetectorEmpty(ptr) } == 0 {
            let segment = Speech(
                NonNull::new(unsafe { ffi::SherpaOnnxVoiceActivityDetectorFront(ptr).cast_mut() })
                    .ok_or("VadFailed")?,
            );
            let raw = unsafe { segment.0.as_ref() };
            if raw.start < 0 || raw.n <= 0 || raw.start as usize + raw.n as usize > count + 512 {
                return Err("VadFailed");
            }
            *first = (*first).min(raw.start as usize).min(count);
            *last = (*last).max(raw.start as usize + raw.n as usize).min(count);
            unsafe { ffi::SherpaOnnxVoiceActivityDetectorPop(ptr) };
        }
        Ok(())
    }
}
