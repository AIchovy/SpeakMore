use super::*;
use speakmore_audio::capture::{AudioService, CaptureHandle, Selection};
use std::{
    ffi::{c_void, CStr},
    sync::mpsc::{self, Receiver, SyncSender},
};

#[derive(Clone)]
pub struct Frame {
    pub track: usize,
    pub samples: Arc<[f32]>,
    pub at: Instant,
}
struct Context {
    tx: SyncSender<Frame>,
    overflow: Arc<AtomicBool>,
}
unsafe extern "C" fn callback(context: *mut c_void, pcm: *const f32, count: i32, _: f64) {
    let ctx = unsafe { &*(context as *const Context) };
    if count <= 0 || count > 16000 || pcm.is_null() {
        ctx.overflow.store(true, Ordering::Release);
        return;
    }
    let samples = Arc::from(unsafe { std::slice::from_raw_parts(pcm, count as usize) });
    if ctx
        .tx
        .try_send(Frame {
            track: 1,
            samples,
            at: Instant::now(),
        })
        .is_err()
    {
        ctx.overflow.store(true, Ordering::Release);
    }
}
struct SystemCapture {
    raw: *mut c_void,
    context: *mut Context,
    library: Arc<libloading::Library>,
}
fn system_audio_error(error: impl std::fmt::Debug) -> AppError {
    eprintln!("SpeakMore system audio component: {error:?}");
    err(
        "SystemAudioUnavailable",
        "系统音频组件加载失败，请重新打开新版 SpeakMore。",
    )
}
// ScreenCaptureKit may retain delegate cleanup work after stopCapture returns.
// Keep the Swift code image resident while those framework-owned callbacks drain.
fn system_library(s: &Service) -> Result<Arc<libloading::Library>> {
    static LIBRARY: std::sync::OnceLock<std::result::Result<Arc<libloading::Library>, String>> =
        std::sync::OnceLock::new();
    LIBRARY
        .get_or_init(|| unsafe {
            libloading::Library::new(s.0.workers.join("libspeakmore-audio.dylib"))
                .map(Arc::new)
                .map_err(|e| e.to_string())
        })
        .clone()
        .map_err(system_audio_error)
}
// Ownership stays on a single capture thread. Swift drains its serial callback queue before Drop releases context.
unsafe impl Send for SystemCapture {}
impl Drop for SystemCapture {
    fn drop(&mut self) {
        unsafe {
            if let Ok(stop) = self
                .library
                .get::<unsafe extern "C" fn(*mut c_void)>(b"sm_audio_stop")
            {
                stop(self.raw);
            }
            drop(Box::from_raw(self.context));
        }
    }
}
struct RawCapture {
    source: String,
    pub frames: Receiver<Frame>,
    pub overflow: Arc<AtomicBool>,
    mic: Option<CaptureHandle>,
    _system: Option<SystemCapture>,
}
impl RawCapture {
    pub fn start(s: &Service, source: &str, pid: Option<i32>) -> Result<Self> {
        if !["microphone", "system", "mixed"].contains(&source) {
            return Err(AppError::invalid("请选择录音来源。"));
        }
        if source != "system"
            && crate::permissions::read().microphone != crate::permissions::Microphone::Authorized
        {
            return Err(err("MicrophonePermission", "请在设置中授权麦克风。"));
        }
        let (tx, frames) = mpsc::sync_channel(1500); // <=30s of typical 20ms system audio; overflow is surfaced.
        let overflow = Arc::new(AtomicBool::new(false));
        let system = if source != "microphone" {
            unsafe {
                let library = system_library(s)?;
                let start = library
                    .get::<unsafe extern "C" fn(
                        i32,
                        unsafe extern "C" fn(*mut c_void, *const f32, i32, f64),
                        *mut c_void,
                    ) -> *mut c_void>(b"sm_audio_start")
                    .map_err(system_audio_error)?;
                let context = Box::into_raw(Box::new(Context {
                    tx: tx.clone(),
                    overflow: overflow.clone(),
                }));
                let raw = start(pid.unwrap_or(0), callback, context.cast());
                if raw.is_null() {
                    drop(Box::from_raw(context));
                    return Err(err("SystemAudioPermission", "无法采集系统声音。请在系统设置的「屏幕与系统音频录制」中允许 SpeakMore；修改后重启应用。"));
                }
                Some(SystemCapture {
                    raw,
                    context,
                    library,
                })
            }
        } else {
            None
        };
        let mic = if source != "system" {
            let overflow = overflow.clone();
            Some(
                AudioService::default()
                    .start_continuous(
                        Uuid::new_v4(),
                        Selection {
                            device_id: s.0.runtime.snapshot()?.settings.input_device_id,
                            ..Default::default()
                        },
                        1600,
                        move |samples| {
                            if tx
                                .try_send(Frame {
                                    track: 0,
                                    samples: Arc::from(samples),
                                    at: Instant::now(),
                                })
                                .is_err()
                            {
                                overflow.store(true, Ordering::Release);
                            }
                            Ok(())
                        },
                    )
                    .map_err(|_| err("MicrophoneCapture", "无法启动麦克风。"))?,
            )
        } else {
            None
        };
        if let Some(mic) = &mic {
            wait_for_microphone_ready(|| mic.try_recv(), Duration::from_secs(4))?;
        }
        Ok(Self {
            source: source.into(),
            frames,
            overflow,
            mic,
            _system: system,
        })
    }
    pub fn check(&self) -> Result<()> {
        if let Some(m) = &self.mic {
            match m.try_recv() {
                Ok(speakmore_audio::capture::CaptureEvent::Finished { result: Err(_), .. }) => {
                    return Err(err(
                        "CaptureInterrupted",
                        "麦克风录音中断，已保存的音频保留。",
                    ))
                }
                _ => {}
            }
        }
        if self.overflow.swap(false, Ordering::AcqRel) {
            return Err(err(
                "AudioGap",
                "音频处理跟不上或音源已中断，已停止以避免生成缺失的记录。",
            ));
        }
        Ok(())
    }
}

fn wait_for_microphone_ready(
    mut poll: impl FnMut() -> std::result::Result<
        speakmore_audio::capture::CaptureEvent,
        mpsc::TryRecvError,
    >,
    timeout: Duration,
) -> Result<()> {
    use speakmore_audio::{capture::CaptureEvent, error::AudioError};
    let deadline = Instant::now() + timeout;
    loop {
        match poll() {
            Ok(CaptureEvent::Ready { .. }) => return Ok(()),
            Ok(CaptureEvent::Finished {
                result: Err(AudioError::PermissionDenied),
                ..
            }) => {
                return Err(err("MicrophonePermission", "请在设置中授权麦克风。"));
            }
            Ok(CaptureEvent::Finished { .. }) | Err(mpsc::TryRecvError::Disconnected) => {
                return Err(err(
                    "MicrophoneCapture",
                    "无法启动麦克风，请检查设备连接和系统声音输入设置。",
                ));
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if Instant::now() >= deadline {
            return Err(err(
                "MicrophoneCapture",
                "麦克风启动超时，请检查设备连接后重试。",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
impl Drop for RawCapture {
    fn drop(&mut self) {
        if let Some(m) = &self.mic {
            m.stop();
        }
    }
}

pub struct Hub {
    captures: Mutex<Vec<RawCapture>>,
    sources: Mutex<(bool, bool, Option<i32>)>,
    subscribers: Mutex<Vec<(Uuid, SyncSender<Frame>, Arc<AtomicBool>, String)>>,
}
pub struct Capture {
    pub frames: Receiver<Frame>,
    hub: Arc<Hub>,
    id: Uuid,
    overflow: Arc<AtomicBool>,
}
impl Capture {
    pub fn start(s: &Service, source: &str, pid: Option<i32>) -> Result<Self> {
        validate_recording_permission(source, crate::permissions::read().microphone)?;
        let mut slot = s.0.audio_hub.lock().map_err(store::io)?;
        let hub = if let Some(h) = slot.upgrade() {
            h
        } else {
            let h = Arc::new(Hub {
                captures: Mutex::new(vec![]),
                sources: Mutex::new((false, false, None)),
                subscribers: Mutex::new(vec![]),
            });
            *slot = Arc::downgrade(&h);
            let weak = Arc::downgrade(&h);
            std::thread::spawn(move || {
                while let Some(h) = weak.upgrade() {
                    let captures = h.captures.lock().unwrap();
                    let clients = h.subscribers.lock().unwrap();
                    for capture in captures.iter() {
                        if capture.check().is_err() {
                            let track = usize::from(capture.source == "system");
                            for (_, _, err, source) in clients.iter() {
                                if receives(source, track) {
                                    err.store(true, Ordering::Release);
                                }
                            }
                        }
                        while let Ok(frame) = capture.frames.try_recv() {
                            for (_, tx, error, source) in clients.iter() {
                                if !receives(source, frame.track) {
                                    continue;
                                }
                                if tx.try_send(frame.clone()).is_err() {
                                    error.store(true, Ordering::Release);
                                }
                            }
                        }
                    }
                    drop(clients);
                    drop(captures);
                    drop(h);
                    std::thread::sleep(Duration::from_millis(10));
                }
            });
            h
        };
        let mic = source != "system";
        let system = source != "microphone";
        let (tx, frames) = mpsc::sync_channel(1000);
        let overflow = Arc::new(AtomicBool::new(false));
        let id = Uuid::new_v4();
        {
            let mut sources = hub.sources.lock().unwrap();
            if system && sources.1 && sources.2 != pid {
                return Err(err(
                    "AudioSourceBusy",
                    "会议和字幕共用音源，请选择相同的应用或全部系统声音。",
                ));
            }
            // Start devices outside the captures lock; the hub thread keeps delivering other sources.
            let mut started = vec![];
            if mic && !sources.0 {
                started.push(RawCapture::start(s, "microphone", None)?);
            }
            if system && !sources.1 {
                started.push(RawCapture::start(s, "system", pid)?);
                sources.1 = true;
                sources.2 = pid;
            }
            sources.0 |= mic;
            hub.captures.lock().unwrap().extend(started);
            hub.subscribers
                .lock()
                .unwrap()
                .push((id, tx, overflow.clone(), source.into()));
        }

        Ok(Self {
            frames,
            hub,
            id,
            overflow,
        })
    }
    pub fn pause(&self, _paused: bool) {} // Each subscriber independently discards paused audio; other sessions continue.
    pub fn check(&self) -> Result<()> {
        if self.overflow.swap(false, Ordering::AcqRel) {
            Err(err("AudioGap", "音源中断或处理跟不上，已保存的内容保留。"))
        } else {
            Ok(())
        }
    }
}

pub(super) fn validate_recording_permission(
    source: &str,
    microphone: crate::permissions::Microphone,
) -> Result<()> {
    if !["microphone", "system", "mixed"].contains(&source) {
        return Err(AppError::invalid("请选择录音来源。"));
    }
    if source != "system" && microphone != crate::permissions::Microphone::Authorized {
        return Err(err("MicrophonePermission", "请在设置中授权麦克风。"));
    }
    Ok(())
}
impl Drop for Capture {
    fn drop(&mut self) {
        let mut sources = self.hub.sources.lock().unwrap();
        let (mic, system) = {
            let mut subscribers = self.hub.subscribers.lock().unwrap();
            subscribers.retain(|(id, _, _, _)| *id != self.id);
            (
                subscribers.iter().any(|(_, _, _, s)| s != "system"),
                subscribers.iter().any(|(_, _, _, s)| s != "microphone"),
            )
        };
        let removed: Vec<RawCapture> = {
            let mut captures = self.hub.captures.lock().unwrap();
            let (keep, removed) = std::mem::take(&mut *captures).into_iter().partition(|c| {
                if c.source == "system" {
                    system
                } else {
                    mic
                }
            });
            *captures = keep;
            removed
        };
        sources.0 = mic;
        sources.1 = system;
        if !system {
            sources.2 = None;
        }
        // Stopping a device can block; never hold the captures lock while it drains.
        drop(removed);
    }
}
fn receives(source: &str, track: usize) -> bool {
    match source {
        "system" => track == 1,
        "microphone" => track == 0,
        _ => true,
    }
}

#[tauri::command]
pub async fn meeting_audio_apps(
    window: WebviewWindow,
    state: State<'_, Managed>,
) -> Result<serde_json::Value> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    tauri::async_runtime::spawn_blocking(move || unsafe {
        let library = system_library(&s)?;
        let apps = library
            .get::<unsafe extern "C" fn() -> *mut i8>(b"sm_audio_apps")
            .map_err(system_audio_error)?;
        let free = library
            .get::<unsafe extern "C" fn(*mut c_void)>(b"sm_audio_free")
            .map_err(system_audio_error)?;
        let p = apps();
        if p.is_null() {
            return Err(audio_apps_error());
        }
        let value = parse_audio_apps(CStr::from_ptr(p).to_bytes());
        free(p.cast());
        value
    })
    .await
    .map_err(|_| audio_apps_error())?
}

pub(super) fn system_apps(s: &Service) -> Result<serde_json::Value> {
    unsafe {
        let library = system_library(s)?;
        let apps = library
            .get::<unsafe extern "C" fn() -> *mut i8>(b"sm_audio_apps")
            .map_err(system_audio_error)?;
        let free = library
            .get::<unsafe extern "C" fn(*mut c_void)>(b"sm_audio_free")
            .map_err(system_audio_error)?;
        let p = apps();
        if p.is_null() {
            return Err(audio_apps_error());
        }
        let value = parse_audio_apps(CStr::from_ptr(p).to_bytes());
        free(p.cast());
        value
    }
}
pub(super) fn scope_for(s: &Service, pid: Option<i32>) -> Result<Option<store::SystemApp>> {
    let Some(pid) = pid else {
        return Ok(None);
    };
    let apps = system_apps(s)?;
    let app = apps
        .as_array()
        .and_then(|apps| {
            apps.iter()
                .find(|app| app["pid"].as_i64() == Some(i64::from(pid)))
        })
        .ok_or_else(|| err("AudioAppMissing", "所选应用已退出，请调整音源后续录。"))?;
    let bundle_id = app["bundleId"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| err("AudioAppMissing", "无法确认所选应用，请重新选择音源。"))?;
    Ok(Some(store::SystemApp {
        bundle_id: bundle_id.into(),
        name: app["name"].as_str().unwrap_or("应用").into(),
    }))
}
pub(super) fn resume_scope(s: &Service, scope: &store::SystemApp) -> Result<i32> {
    let apps = system_apps(s)?;
    let pid = apps
        .as_array()
        .and_then(|apps| {
            apps.iter()
                .find(|app| app["bundleId"].as_str() == Some(&scope.bundle_id))
        })
        .and_then(|app| app["pid"].as_i64());
    pid.and_then(|pid| i32::try_from(pid).ok()).ok_or_else(|| {
        err(
            "AudioAppMissing",
            "原会议应用已退出，请重新打开它，或调整音源后续录。",
        )
    })
}

fn audio_apps_error() -> AppError {
    err(
        "AudioAppsUnavailable",
        "无法读取可采集声音的应用列表，请稍后重试；也可选择「全部系统声音」。",
    )
}
fn parse_audio_apps(bytes: &[u8]) -> Result<serde_json::Value> {
    #[derive(Deserialize, Serialize)]
    struct AudioApp {
        pid: i32,
        name: String,
        #[serde(default, rename = "bundleId")]
        bundle_id: Option<String>,
    }
    #[derive(Deserialize)]
    struct Response {
        apps: Option<Vec<AudioApp>>,
        error: Option<String>,
    }
    let response: Response = serde_json::from_slice(bytes).map_err(|_| audio_apps_error())?;
    match response.error.as_deref() {
        Some("SystemAudioPermission") => return Err(err("SystemAudioPermission", "请在系统设置 → 隐私与安全性 → 屏幕与系统音频录制中允许 SpeakMore。授权后如仍无法读取，请退出并重新打开应用。")),
        Some(_) => return Err(audio_apps_error()),
        None => {},
    }
    let apps = response.apps.ok_or_else(audio_apps_error)?;
    if apps.is_empty() {
        return Err(err(
            "AudioAppsEmpty",
            "暂未发现可采集声音的应用，请先打开会议或播放器应用，再重试。",
        ));
    }
    serde_json::to_value(apps).map_err(|_| audio_apps_error())
}

#[cfg(test)]
mod app_list_tests {
    use super::*;
    #[test]
    fn asynchronous_microphone_start_failure_is_not_reported_as_ready() {
        use speakmore_audio::{
            capture::{CaptureEvent, Diagnostics},
            error::AudioError,
        };
        for (cause, expected) in [
            (AudioError::PermissionDenied, "MicrophonePermission"),
            (AudioError::NoInputDevice, "MicrophoneCapture"),
        ] {
            let error = wait_for_microphone_ready(
                || {
                    Ok(CaptureEvent::Finished {
                        session_id: Uuid::new_v4(),
                        result: Err(cause),
                        diagnostics: Diagnostics {
                            accepted_frames: 0,
                            clipped_samples: 0,
                            scheduling_fallback: false,
                        },
                    })
                },
                Duration::ZERO,
            )
            .unwrap_err();
            assert_eq!(error.code, expected);
        }
        for state in [mpsc::TryRecvError::Empty, mpsc::TryRecvError::Disconnected] {
            assert_eq!(
                wait_for_microphone_ready(|| Err(state), Duration::ZERO)
                    .unwrap_err()
                    .code,
                "MicrophoneCapture"
            );
        }
    }
    #[test]
    fn microphone_sources_require_permission_but_system_only_does_not() {
        use crate::permissions::Microphone::*;
        for permission in [NotDetermined, Restricted, Denied, Unavailable] {
            for source in ["microphone", "mixed"] {
                assert_eq!(
                    validate_recording_permission(source, permission)
                        .unwrap_err()
                        .code,
                    "MicrophonePermission"
                );
            }
            assert!(validate_recording_permission("system", permission).is_ok());
        }
        for source in ["microphone", "mixed", "system"] {
            assert!(validate_recording_permission(source, Authorized).is_ok());
        }
        assert!(validate_recording_permission("invalid", Authorized).is_err());
    }
    #[test]
    fn distinguishes_permission_component_and_app_list_failures() {
        let permission = parse_audio_apps(br#"{"error":"SystemAudioPermission"}"#).unwrap_err();
        assert_eq!(permission.code, "SystemAudioPermission");
        assert!(permission.user_message.contains("屏幕与系统音频录制"));
        for data in [
            br#"{"error":"AudioAppsUnavailable"}"#.as_slice(),
            b"bad",
            b"{}",
        ] {
            let error = parse_audio_apps(data).unwrap_err();
            assert_eq!(error.code, "AudioAppsUnavailable");
            assert!(!error.user_message.contains("文件"));
        }
        assert_eq!(
            parse_audio_apps(br#"{"apps":[]}"#).unwrap_err().code,
            "AudioAppsEmpty"
        );
        assert_eq!(
            system_audio_error("missing export").code,
            "SystemAudioUnavailable"
        );
    }
    #[test]
    fn returns_real_app_names_and_process_ids() {
        let apps = parse_audio_apps(br#"{"apps":[{"pid":123,"name":"Zoom"}]}"#).unwrap();
        assert_eq!(apps[0]["pid"], 123);
        assert_eq!(apps[0]["name"], "Zoom");
    }
}

// Arrival jitter up to this bound keeps a track contiguous; larger drift re-aligns to the wall clock.
const DRIFT: usize = 1600;
struct Timeline {
    offset: usize,
    cursor: [Option<usize>; 2],
    tracks: [Vec<f32>; 2],
}
impl Timeline {
    fn new(offset: usize) -> Self {
        Self {
            offset,
            cursor: [None, None],
            tracks: [vec![0f32; 96000], vec![0f32; 96000]],
        }
    }
    /// `end` is the wall-clock sample position where the frame finished arriving.
    fn place(&mut self, track: usize, end: usize, samples: &[f32]) {
        let wall = end.saturating_sub(samples.len());
        let at = self.cursor[track]
            .filter(|cursor| cursor.abs_diff(wall) <= DRIFT)
            .unwrap_or(wall);
        let buffer = &mut self.tracks[track];
        for (i, v) in samples.iter().enumerate() {
            if let Some(p) = (at + i)
                .checked_sub(self.offset)
                .filter(|p| *p < buffer.len())
            {
                buffer[p] = *v;
            }
        }
        self.cursor[track] = Some(at + samples.len());
    }
    /// End of the audio actually received on any track; later positions have not arrived yet.
    fn received(&self) -> usize {
        self.cursor
            .iter()
            .flatten()
            .copied()
            .max()
            .unwrap_or(self.offset)
    }
    fn flush(&mut self, s: &Service, id: Uuid, source: &str, count: usize) -> Result<()> {
        let count = count.min(self.tracks[0].len());
        if count == 0 {
            return Ok(());
        }
        let [microphone, system] = &self.tracks;
        let mix: Vec<f32> = (0..count)
            .map(|i| match source {
                "microphone" => microphone[i],
                "system" => system[i],
                _ => (microphone[i] + system[i]) * 0.5,
            })
            .collect();
        let _lock = s.0.lock.lock().map_err(store::io)?;
        let mut m = store::read(&s.0.root, id)?;
        if source == "mixed" {
            store::save_track(&s.0.root, &m, "microphone", &microphone[..count])?;
            store::save_track(&s.0.root, &m, "system", &system[..count])?;
        }
        store::append(&s.0.root, &mut m, &mix)?;
        self.offset += count;
        for t in &mut self.tracks {
            t.copy_within(count.., 0);
            let n = t.len();
            t[n - count..].fill(0.0);
        }
        s.changed();
        Ok(())
    }
}

pub(super) fn record(s: &Service, guard: &JobGuard, source: &str, capture: Capture) -> Result<()> {
    let id = guard.active.meeting.unwrap();
    let input = s.0.runtime.native_input();
    let epoch = input.lifecycle().epoch;
    let meeting = store::read(&s.0.root, id)?;
    *guard.active.recording.lock().unwrap() = Some(RecordingMeta {
        title: meeting.title,
        source: source.into(),
    });
    s.edit(id, |m| {
        m.state = "recording".into();
        if let Some(gap) = m.gaps.last_mut().filter(|gap| gap.ended_at.is_none()) {
            gap.ended_at = Some(crate::now());
        }
        Ok(())
    })?;
    let _live_notes = live_notes::start(s, id);
    let start = Instant::now();
    let base_samples = store::read(&s.0.root, id)?.samples();
    let base_ms = base_samples as u64 / 16;
    let mut paused_at = None;
    let mut paused_time = Duration::ZERO;
    let mut stopped_at = None::<Instant>;
    let mut stopped_elapsed = None;
    let mut timeline = Timeline::new(base_samples);
    let mut ready = base_samples;
    let result = (|| loop {
        let lifecycle = input.lifecycle();
        if lifecycle.blocked() || lifecycle.epoch != epoch {
            return Err(err(
                "RecordingInterrupted",
                "系统睡眠或会话切换，录音已中断；已保存的音频可继续整理。",
            ));
        }
        let paused = guard.active.paused.load(Ordering::Acquire);
        if paused && paused_at.is_none() {
            paused_at = Some(Instant::now());
            capture.pause(true);
            s.edit(id, |m| {
                m.state = "paused".into();
                Ok(())
            })?;
        }
        if !paused {
            if let Some(at) = paused_at.take() {
                paused_time += at.elapsed();
                capture.pause(false);
                s.edit(id, |m| {
                    m.state = "recording".into();
                    Ok(())
                })?;
            }
        }
        let elapsed = paused_at
            .unwrap_or_else(Instant::now)
            .duration_since(start)
            .saturating_sub(paused_time);
        guard
            .active
            .elapsed
            .store(base_ms + elapsed.as_millis() as u64, Ordering::Release);
        let requested = guard.active.stop.load(Ordering::Acquire)
            || guard.active.cancel.load(Ordering::Acquire);
        if requested && stopped_at.is_none() {
            stopped_at = Some(Instant::now());
            stopped_elapsed = Some(elapsed);
        }
        let stop = stopped_at.is_some_and(|at| at.elapsed() >= Duration::from_millis(180));
        while let Ok(frame) = capture.frames.try_recv() {
            // Audio queued before this recording started belongs to no position in it.
            if paused || frame.at < start {
                continue;
            }
            let rms = (frame.samples.iter().map(|v| v * v).sum::<f32>()
                / frame.samples.len().max(1) as f32)
                .sqrt();
            guard.active.levels[frame.track]
                .store((rms.min(1.0) * 1000.0) as u64, Ordering::Release);
            let end = base_samples
                + (frame
                    .at
                    .saturating_duration_since(start)
                    .saturating_sub(paused_time)
                    .as_secs_f64()
                    * 16000.0) as usize;
            timeline.place(frame.track, end, &frame.samples);
        }
        // Freeze the active duration on stop, including when stopped while paused.
        let end = stopped_elapsed.unwrap_or(elapsed);
        ready = base_samples + (end.as_secs_f64() * f64::from(store::RATE)) as usize;
        capture.check()?;
        // Allow 250ms for callback scheduling plus the drift bound, then persist five-second blocks.
        let count = if stop {
            ready.saturating_sub(timeline.offset).min(80000)
        } else if ready >= timeline.offset + 84000 + DRIFT {
            80000
        } else {
            0
        };
        timeline.flush(s, id, source, count)?;
        if stop && timeline.offset >= ready {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    })();
    if let Err(error) = result {
        // Keep the audio already received before the interruption, without padding with
        // silence for frames that never arrived.
        let end = ready.min(timeline.received());
        if let Err(e) = timeline.flush(s, id, source, end.saturating_sub(timeline.offset)) {
            eprintln!("SpeakMore meeting audio flush: {}", e.code);
        }
        return Err(error);
    }
    s.edit(id, |m| {
        m.state = "saved".into();
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod timeline_tests {
    use super::*;
    #[test]
    fn arrival_jitter_keeps_samples_contiguous_and_large_drift_realigns() {
        let mut t = Timeline::new(0);
        t.place(0, 1600, &[1.0; 1600]);
        // Arrives 30ms late: still contiguous instead of leaving a silent gap.
        t.place(0, 3200 + 480, &[2.0; 1600]);
        // Arrives 20ms early: does not overwrite the previous block.
        t.place(0, 4800 - 320, &[3.0; 1600]);
        assert!(t.tracks[0][..1600].iter().all(|v| *v == 1.0));
        assert!(t.tracks[0][1600..3200].iter().all(|v| *v == 2.0));
        assert!(t.tracks[0][3200..4800].iter().all(|v| *v == 3.0));
        assert_eq!(t.cursor[0], Some(4800));
        // A real interruption (500ms) is padded explicitly with silence.
        t.place(0, 4800 + 8000 + 1600, &[4.0; 1600]);
        assert!(t.tracks[0][4800..12800].iter().all(|v| *v == 0.0));
        assert!(t.tracks[0][12800..14400].iter().all(|v| *v == 4.0));
        // Tracks are independent.
        assert_eq!(t.cursor[1], None);
        t.place(1, 1600, &[5.0; 1600]);
        assert!(t.tracks[1][..1600].iter().all(|v| *v == 5.0));
    }
    #[test]
    fn received_end_follows_the_furthest_track() {
        let mut t = Timeline::new(16000);
        assert_eq!(t.received(), 16000);
        t.place(1, 16000 + 3200, &[1.0; 1600]);
        t.place(0, 16000 + 1600, &[1.0; 1600]);
        assert_eq!(t.received(), 16000 + 3200);
    }
    #[test]
    fn samples_before_the_flushed_offset_are_dropped() {
        let mut t = Timeline::new(16000);
        t.place(0, 16000 + 800, &[1.0; 1600]);
        assert!(t.tracks[0][..800].iter().all(|v| *v == 1.0));
        assert_eq!(t.cursor[0], Some(16800));
    }
}
