use super::*;

const KEPT_VERSIONS: usize = 50;

pub(super) fn archive(s: &Service, m: &Meeting) -> Result<()> {
    let dir = s.0.root.join(m.id.to_string());
    let mut f = tempfile::NamedTempFile::new_in(&dir).map_err(store::io)?;
    use std::io::Write;
    f.write_all(&serde_json::to_vec(m).map_err(store::io)?)
        .map_err(store::io)?;
    f.as_file().sync_all().map_err(store::io)?;
    f.persist(dir.join(format!("summary-{}.json", m.revision)))
        .map_err(store::io)?;
    // Only the newest versions are offered for restore; older snapshots are full copies.
    for (_, path) in version_files(&dir).into_iter().skip(KEPT_VERSIONS) {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}
/// Snapshots, newest first, ordered by the revision in the file name.
fn version_files(dir: &Path) -> Vec<(u64, PathBuf)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let revision = name
                .to_str()?
                .strip_prefix("summary-")?
                .strip_suffix(".json")?
                .parse::<u64>()
                .ok()?;
            Some((revision, e.path()))
        })
        .collect();
    files.sort_by_key(|(revision, _)| std::cmp::Reverse(*revision));
    files
}
fn read_version(path: &Path) -> Result<Meeting> {
    let (bytes, _) = store::read_bounded(path, 16 * 1024 * 1024)?;
    serde_json::from_slice(&bytes).map_err(store::io)
}
#[tauri::command]
pub fn meeting_transcript_update(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    revision: u64,
    mut segments: Vec<store::Segment>,
) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    if s.meeting_busy(id) {
        return Err(AppError::busy());
    }
    s.edit(id, |m| {
        if m.revision != revision {
            return Err(AppError::conflict());
        }
        if segments.len() != m.segments.len()
            || segments.iter().zip(&m.segments).any(|(a, b)| {
                a.id != b.id
                    || a.start_ms != b.start_ms
                    || a.end_ms != b.end_ms
                    || a.speaker_id != b.speaker_id
                    || a.text.chars().count() > 12000
                    || a.speaker.as_ref().is_some_and(|s| s.chars().count() > 80)
            })
        {
            return Err(AppError::invalid("原文格式无效。"));
        }
        let mut names = std::collections::BTreeMap::new();
        for (edited, old) in segments.iter().zip(&m.segments) {
            if edited.speaker != old.speaker {
                if let Some(id) = &old.speaker_id {
                    let name = edited
                        .speaker
                        .clone()
                        .filter(|name| !name.trim().is_empty())
                        .ok_or_else(|| AppError::invalid("说话人姓名不能为空。"))?;
                    if names
                        .insert(id.clone(), name.clone())
                        .is_some_and(|previous| previous != name)
                    {
                        return Err(AppError::invalid(
                            "同一身份设置了不同姓名，请在“本场说话人”中统一修改。",
                        ));
                    }
                }
            }
        }
        for segment in &mut segments {
            if let Some(name) = segment.speaker_id.as_ref().and_then(|id| names.get(id)) {
                segment.speaker = Some(name.clone());
            }
        }
        archive(&s, m)?;
        m.speaker_names.extend(names);
        m.dirty_segments.extend(
            segments
                .iter()
                .zip(&m.segments)
                .filter(|(a, b)| a.text != b.text || a.speaker != b.speaker)
                .map(|(a, _)| a.id.clone()),
        );
        m.dirty_segments.sort();
        m.dirty_segments.dedup();
        for item in &mut m.items {
            if item.evidence.iter().any(|id| m.dirty_segments.contains(id)) {
                item.support = "review".into();
                item.reviewed = false;
            }
        }
        m.segments = segments;
        m.summary_stale = true;
        Ok(())
    })
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    revision: u64,
    items: Vec<store::Item>,
    segments: Vec<store::Segment>,
}
#[tauri::command]
pub async fn meeting_versions(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
) -> Result<Vec<Version>> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        store::read(&s.0.root, id)?;
        let mut versions = vec![];
        for (revision, path) in version_files(&s.0.root.join(id.to_string())) {
            if versions.len() == KEPT_VERSIONS {
                break;
            }
            // A damaged snapshot only hides that version.
            if let Ok(m) = read_version(&path) {
                versions.push(Version {
                    revision,
                    items: m.items,
                    segments: m.segments,
                });
            }
        }
        Ok(versions)
    })
    .await
    .map_err(store::io)?
}
#[tauri::command]
pub async fn meeting_restore(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    revision: u64,
    version: u64,
) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    tauri::async_runtime::spawn_blocking(move || restore(&s, id, revision, version))
        .await
        .map_err(store::io)?
}
fn restore(s: &Service, id: Uuid, revision: u64, version: u64) -> Result<()> {
    if s.meeting_busy(id) {
        return Err(AppError::busy());
    }
    let old = read_version(
        &s.0.root
            .join(id.to_string())
            .join(format!("summary-{version}.json")),
    )?;
    s.edit(id, |m| {
        if revision != m.revision {
            return Err(AppError::conflict());
        }
        archive(s, m)?;
        store::validate_items(&old.items, &old.segments)?;
        m.notes = old.notes;
        m.project = old.project;
        m.template_prompt = old.template_prompt;
        m.context_revision = old.context_revision;
        m.summarized_context_revision = old.summarized_context_revision;
        m.dirty_segments = old.dirty_segments;
        m.items = old.items;
        m.segments = old.segments;
        m.speaker_names = old.speaker_names;
        m.speaker_profiles = old.speaker_profiles;
        m.template = old.template;
        m.processed_samples = old.processed_samples.min(m.samples());
        m.summary_stale = old.summary_stale || m.processed_samples < m.samples();
        Ok(())
    })
}
#[tauri::command]
pub async fn meeting_retention(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    days: Option<u32>,
) -> Result<()> {
    crate::main_only(&window)?;
    if !matches!(days, None | Some(0 | 7 | 30)) {
        return Err(AppError::invalid("请选择保留期限。"));
    }
    let s = get(&state)?;
    if s.meeting_busy(id) {
        return Err(AppError::busy());
    }
    tauri::async_runtime::spawn_blocking(move || {
        s.edit(id, |m| {
            m.retention_days = days;
            Ok(())
        })?;
        cleanup(&s)
    })
    .await
    .map_err(store::io)?
}
fn expired(m: &Meeting) -> bool {
    // Use creation timestamp in milliseconds, same clock as all meeting metadata.
    !m.audio_deleted
        && m.state == "ready"
        && m.retention_days.is_some_and(|days| {
            crate::now().saturating_sub(m.created_at) >= days as i64 * 86400 * 1000
        })
}
pub fn cleanup(s: &Service) -> Result<()> {
    if s.busy() {
        return Ok(());
    }
    let mut result = Ok(());
    for summary in s.summaries()?.into_iter().filter(expired) {
        // Lock one meeting at a time so recording and edits are not held up by deletions.
        let _lock = s.0.lock.lock().map_err(store::io)?;
        let outcome = (|| {
            let mut m = store::read(&s.0.root, summary.id)?;
            if !expired(&m) {
                return Ok(());
            }
            for e in std::fs::read_dir(s.0.root.join(m.id.to_string())).map_err(store::io)? {
                let e = e.map_err(store::io)?;
                if e.file_name().to_string_lossy().ends_with(".pcm") {
                    std::fs::remove_file(e.path()).map_err(store::io)?;
                }
            }
            m.audio_deleted = true;
            m.revision += 1;
            store::save(&s.0.root, &m)
        })();
        if outcome.is_err() {
            result = outcome;
        }
    }
    result
}
pub fn subtitle_time(ms: u64, vtt: bool) -> String {
    format!(
        "{:02}:{:02}:{:02}{}{:03}",
        ms / 3600000,
        ms / 60000 % 60,
        ms / 1000 % 60,
        if vtt { "." } else { "," },
        ms % 1000
    )
}
pub(super) fn csv(value: &str) -> String {
    let safe = if value
        .trim_start()
        .starts_with(['=', '+', '-', '@', '\t', '\r'])
    {
        format!("'{value}")
    } else {
        value.into()
    };
    format!("\"{}\"", safe.replace('"', "\"\""))
}
pub fn export(m: &Meeting, format: &str) -> Result<String> {
    match format {
        "srt" | "vtt" => {
            let mut out = if format == "vtt" {
                "WEBVTT\n\n".into()
            } else {
                String::new()
            };
            for (n, s) in m.segments.iter().enumerate() {
                out.push_str(&format!(
                    "{}\n{} --> {}\n{}{}\n\n",
                    n + 1,
                    subtitle_time(s.start_ms, format == "vtt"),
                    subtitle_time(s.end_ms, format == "vtt"),
                    s.speaker
                        .as_ref()
                        .map(|x| format!("{x}: "))
                        .unwrap_or_default(),
                    s.text.replace("\n\n", "\n")
                ));
            }
            Ok(out)
        }
        "csv" => {
            let mut out = "\u{feff}类型,内容,负责人,截止日期,已完成,原文依据\r\n".to_string();
            for i in &m.items {
                out.push_str(
                    &[
                        csv(&i.kind),
                        csv(&i.text),
                        csv(i.owner.as_deref().unwrap_or("")),
                        csv(i.due.as_deref().unwrap_or("")),
                        csv(if i.done { "是" } else { "否" }),
                        csv(&i.evidence.join(";")),
                    ]
                    .join(","),
                );
                out.push_str("\r\n");
            }
            Ok(out)
        }
        _ => Err(AppError::invalid("不支持的导出格式。")),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn csv_and_subtitle_safety() {
        assert_eq!(csv("=1+1"), "\"'=1+1\"");
        assert_eq!(csv("a,\"b"), "\"a,\"\"b\"");
        assert_eq!(subtitle_time(3600456, false), "01:00:00,456");
    }
    #[test]
    fn versions_are_ordered_by_the_revision_in_the_file_name() {
        let d = tempfile::tempdir().unwrap();
        for name in [
            "summary-2.json",
            "summary-10.json",
            "summary-x.json",
            "meeting.json",
            "00001.pcm",
        ] {
            std::fs::write(d.path().join(name), b"{").unwrap();
        }
        let files = version_files(d.path());
        assert_eq!(files.iter().map(|f| f.0).collect::<Vec<_>>(), [10, 2]);
        assert!(read_version(&files[0].1).is_err());
    }
}

#[tauri::command]
pub fn meeting_model_choose(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    revision: u64,
    model: Option<String>,
) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    if s.meeting_busy(id) {
        return Err(AppError::busy());
    }
    if model.as_deref().is_some_and(|m| m != "moss") {
        return Err(AppError::invalid("请选择当前语音模型或 MOSS。"));
    }
    s.edit(id, |m| {
        if m.revision != revision {
            return Err(AppError::conflict());
        }
        if m.audio_deleted {
            return Err(AppError::invalid("音频已清理，无法重新转写。"));
        }
        archive(&s, m)?;
        m.model_id = model;
        m.processed_samples = 0;
        m.segments.clear();
        m.items.clear();
        m.summary_stale = false;
        m.state = "saved".into();
        m.error = None;
        Ok(())
    })
}
