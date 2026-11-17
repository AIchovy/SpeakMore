use super::*;
use std::{
    io::{BufRead, BufReader, Read},
    process::{Command, Stdio},
    sync::mpsc,
};
pub fn transcribe(s: &Service, id: Uuid, cancel: &AtomicBool, live: bool) -> Result<()> {
    let python = moss_runtime::python(s)?;
    catalog::verify(s, "moss", cancel)?;
    catalog::verify(s, "speaker", cancel)?;
    let mut command = Command::new(python);
    command
        .env_clear()
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env(
            "NUMBA_CACHE_DIR",
            s.0.root.parent().unwrap().join("moss-cache"),
        )
        .arg("-I")
        .arg(s.0.workers.join("moss-worker.py"))
        .arg(catalog::root(s, "moss"))
        .arg(s.0.root.join(id.to_string()))
        .arg(catalog::root(s, "speaker").join("speaker.onnx"))
        .arg(s.0.workers.join("libspeakmore-streaming.dylib"))
        // The worker exits when this pipe closes, so it cannot outlive the app.
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if live {
        command.arg("--live");
    }
    let mut child = command.spawn().map_err(|_| {
        err(
            "MossRuntime",
            "MOSS 本地运行环境不可用，请在设置 → 语音模型中重新安装运行环境。",
        )
    })?;
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::sync_channel(2);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut bytes = vec![];
            if reader
                .by_ref()
                .take(2097153)
                .read_until(b'\n', &mut bytes)
                .is_err()
                || bytes.is_empty()
                || bytes.len() > 2097152
            {
                break;
            }
            if tx.send(bytes).is_err() {
                break;
            }
        }
    });
    let result = (|| {
        let mut deadline = Instant::now() + Duration::from_secs(300);
        loop {
            check_cancel(cancel)?;
            if Instant::now() > deadline {
                return Err(err(
                    "MossTimeout",
                    "MOSS 处理超时，已保存的进度可继续；可改用轻量转写模型。",
                ));
            }
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(raw) => {
                    let value: serde_json::Value =
                        serde_json::from_slice(&raw).map_err(store::io)?;
                    if value.get("error").is_some() {
                        return Err(err(
                            "MossFailed",
                            "MOSS 本地转写失败，音频和已完成片段已保留。",
                        ));
                    }
                    deadline = Instant::now() + Duration::from_secs(900);
                    if let Some(processed) = value["processed"].as_u64() {
                        let previous = store::read(&s.0.root, id)?.processed_samples;
                        let profiles: Vec<store::VoiceProfile> =
                            serde_json::from_value(value["profiles"].clone()).map_err(store::io)?;
                        if profiles.len() > 1000
                            || profiles.iter().any(|p| {
                                p.id.len() > 80
                                    || p.embedding.len() > 512
                                    || p.embedding.is_empty()
                                    || p.embedding.iter().any(|v| !v.is_finite())
                            })
                        {
                            return Err(store::io("speaker profiles"));
                        }
                        let mut segments:Vec<store::Segment>=value["segments"].as_array().ok_or_else(||store::io("segments"))?.iter().map(|v|serde_json::from_value(serde_json::json!({"id":"", "startMs":v["startMs"],"endMs":v["endMs"],"text":v["text"],"speaker":v["speaker"],"speakerId":v["speakerId"]})).map_err(store::io)).collect::<Result<_>>()?;
                        s.edit(id, |m| {
                            check_cancel(cancel)?;
                            if processed as usize > m.samples()
                                || processed as usize <= m.processed_samples
                            {
                                return Err(store::io("MOSS output"));
                            }
                            // Deterministic decoding repeats the same output; skip a bad segment, keep the batch.
                            let total = segments.len();
                            segments.retain_mut(|seg| {
                                seg.end_ms = seg.end_ms.min(m.duration_ms);
                                seg.start_ms < seg.end_ms && seg.text.len() <= 32000
                            });
                            if segments.len() < total {
                                eprintln!(
                                    "SpeakMore MOSS: skipped {} invalid segments",
                                    total - segments.len()
                                );
                            }
                            for seg in &mut segments {
                                check_cancel(cancel)?;
                                if let Some(name) = seg
                                    .speaker_id
                                    .as_ref()
                                    .and_then(|id| m.speaker_names.get(id))
                                {
                                    seg.speaker = Some(name.clone());
                                }
                                seg.text = workspace::apply_terms(
                                    &seg.text,
                                    &workspace::preferences(s)?.terms,
                                );
                                seg.id = format!("s{}", m.segments.len() + 1);
                                m.segments.push(seg.clone());
                            }
                            m.processed_samples = processed as usize;
                            m.speaker_profiles = profiles;
                            Ok(())
                        })?;
                        if live {
                            live_notes::summarize(s, id, previous, cancel)?;
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let m = store::read(&s.0.root, id)?;
                    if m.processed_samples == m.samples() {
                        return Ok(());
                    }
                    return Err(err("MossFailed", "MOSS 进程中断，可继续已保存的进度。"));
                }
                _ => {}
            }
        }
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}
