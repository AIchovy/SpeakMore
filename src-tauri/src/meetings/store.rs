use super::*;
use std::io::{Read, Write};

pub const RATE: u32 = 16_000;
// Import decoding has its own bound; live recordings are persisted incrementally.
pub const IMPORT_LIMIT: usize = RATE as usize * 3600;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    pub index: usize,
    pub samples: usize,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    #[serde(default)]
    pub speaker: Option<String>,
    #[serde(default)]
    pub speaker_id: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingGap {
    pub after_ms: u64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub reason: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct VoiceProfile {
    pub id: String,
    pub embedding: Vec<f32>,
    pub count: u32,
}
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Item {
    pub kind: String,
    pub text: String,
    pub owner: Option<String>,
    pub due: Option<String>,
    pub evidence: Vec<String>,
    pub done: bool,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub recipient: Option<String>,
    #[serde(default)]
    pub edited: bool,
    #[serde(default)]
    pub edited_from: Option<String>,
    #[serde(default)]
    pub support: String,
    #[serde(default)]
    pub reviewed: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: Uuid,
    pub at_ms: u64,
    pub text: String,
    pub kind: String,
    #[serde(default)]
    pub image: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemApp {
    pub bundle_id: String,
    pub name: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meeting {
    #[serde(default)]
    pub system_app: Option<SystemApp>,
    pub id: Uuid,
    pub title: String,
    pub created_at: i64,
    pub duration_ms: u64,
    pub source: String,
    pub state: String,
    pub error: Option<String>,
    pub chunks: Vec<Chunk>,
    pub segments: Vec<Segment>,
    pub items: Vec<Item>,
    pub revision: u64,
    pub model_id: Option<String>,
    pub language: String,
    pub processed_samples: usize,
    pub summary_stale: bool,
    #[serde(default)]
    pub summary_provider_id: Option<Uuid>,
    #[serde(default)]
    pub assistant_session_id: Option<Uuid>,
    #[serde(default)]
    pub summary_follow_ai: bool,
    #[serde(default)]
    pub audio_deleted: bool,
    #[serde(default = "retention_default")]
    pub retention_days: Option<u32>,
    #[serde(default)]
    pub gaps: Vec<RecordingGap>,
    #[serde(default)]
    pub last_audio_at: Option<i64>,
    #[serde(default)]
    pub template: String,
    #[serde(default)]
    pub speaker_names: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub speaker_profiles: Vec<VoiceProfile>,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub template_prompt: String,
    #[serde(default)]
    pub dirty_segments: Vec<String>,
    #[serde(default)]
    pub artifacts: Vec<super::experience::Insight>,
    #[serde(default)]
    pub participants: Vec<String>,
    #[serde(default)]
    pub context_revision: u64,
    #[serde(default)]
    pub summarized_context_revision: u64,
}
impl Meeting {
    pub fn new(title: String, source: &str, language: String) -> Self {
        Self {
            system_app: None,
            id: Uuid::new_v4(),
            title: if title.trim().is_empty() {
                format!("会议 {}", chrono::Local::now().format("%m月%d日 %H:%M"))
            } else {
                title.trim().chars().take(120).collect()
            },
            created_at: crate::now(),
            duration_ms: 0,
            source: source.into(),
            state: "saved".into(),
            error: None,
            chunks: vec![],
            segments: vec![],
            items: vec![],
            revision: 1,
            model_id: None,
            language,
            processed_samples: 0,
            summary_stale: false,
            summary_provider_id: None,
            assistant_session_id: None,
            summary_follow_ai: false,
            audio_deleted: false,
            retention_days: Some(7),
            gaps: vec![],
            last_audio_at: None,
            template: "general".into(),
            speaker_names: Default::default(),
            speaker_profiles: vec![],
            notes: vec![],
            project: String::new(),
            template_prompt: String::new(),
            dirty_segments: vec![],
            artifacts: vec![],
            participants: vec![],
            context_revision: 0,
            summarized_context_revision: 0,
        }
    }
    pub fn samples(&self) -> usize {
        self.chunks.iter().map(|c| c.samples).sum()
    }
    pub(super) fn context_changed(&mut self) {
        self.context_revision = self.context_revision.saturating_add(1);
        self.summary_stale = true;
    }
    pub fn text(&self) -> String {
        let mut result = format!("# {}\n\n", self.title);
        if !self.project.is_empty() {
            result.push_str(&format!("项目：{}\n\n", self.project));
        }
        if self.summary_stale {
            result.push_str("内容或整理要求已更新，以下纪要可能过时。\n\n");
        }
        for (kind, label) in [
            ("decision", "决策事项"),
            ("todo", "待办清单"),
            ("fact", "关键数字与结论"),
            ("next", "下一步行动"),
        ] {
            result.push_str(&format!("## {label}\n\n"));
            let items: Vec<_> = self.items.iter().filter(|i| i.kind == kind).collect();
            if items.is_empty() {
                result.push_str("未发现明确内容。\n\n");
            }
            for item in items {
                result.push_str(&format!(
                    "- {}{}",
                    if kind == "todo" {
                        if item.done {
                            "[x] "
                        } else {
                            "[ ] "
                        }
                    } else {
                        ""
                    },
                    item.text
                ));
                let status = match item.status.as_str() {
                    "proposal" => "提议",
                    "decision" => "决定",
                    "commitment" => "承诺",
                    "pending" => "待确认",
                    "information" => "信息",
                    _ => "",
                };
                if !status.is_empty() {
                    result.push_str(&format!(" · {status}"));
                }
                if item.support == "review" {
                    result.push_str(" · 依据待核对");
                }
                if kind == "todo" {
                    result.push_str(&format!(
                        " · {} · {}",
                        item.owner.as_deref().unwrap_or("负责人待确认"),
                        item.due.as_deref().unwrap_or("截止日期未指定")
                    ));
                }
                if let Some(recipient) = &item.recipient {
                    result.push_str(&format!(" · 交付给：{recipient}"));
                }
                for id in &item.evidence {
                    if let Some(s) = self.segments.iter().find(|s| &s.id == id) {
                        result.push_str(&format!(
                            " [{}:{:02}]",
                            s.start_ms / 60000,
                            s.start_ms / 1000 % 60
                        ));
                    }
                }
                result.push('\n');
            }
            result.push('\n');
        }
        if !self.notes.is_empty() {
            result.push_str("## 我的关注与补充（与会议事实分别保留）\n\n");
            for note in &self.notes {
                let label = match note.kind.as_str() {
                    "supplement" => "个人补充",
                    "screenshot" => "讨论截图",
                    "highlight" => "重点标记",
                    _ => "会中随手记",
                };
                result.push_str(&format!(
                    "- [{label}] [{}:{:02}] {}\n",
                    note.at_ms / 60000,
                    note.at_ms / 1000 % 60,
                    note.text
                ));
            }
        }
        result
    }
}

pub fn io(_: impl std::fmt::Debug) -> AppError {
    err(
        "MeetingStorage",
        "会议文件读写失败，请检查剩余空间和文件权限。",
    )
}
pub fn save(root: &Path, m: &Meeting) -> Result<()> {
    let dir = root.join(m.id.to_string());
    std::fs::create_dir_all(&dir).map_err(io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(io)?;
    }
    let mut temp = tempfile::NamedTempFile::new_in(&dir).map_err(io)?;
    temp.write_all(&serde_json::to_vec(m).map_err(io)?)
        .map_err(io)?;
    temp.as_file().sync_all().map_err(io)?;
    temp.persist(dir.join("meeting.json")).map_err(io)?;
    std::fs::File::open(&dir)
        .and_then(|f| f.sync_all())
        .map_err(io)?;
    Ok(())
}
/// Identifies one persisted meeting.json; every save replaces the file atomically.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    node: (u64, u64, i64, i64),
}
impl Stamp {
    pub fn of(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            #[cfg(unix)]
            node: (meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec()),
        }
    }
}
pub(super) fn read_bounded(path: &Path, limit: u64) -> Result<(Vec<u8>, std::fs::Metadata)> {
    let mut file = std::fs::File::open(path).map_err(io)?;
    let meta = file.metadata().map_err(io)?;
    if meta.len() > limit {
        return Err(io("oversized"));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() as u64 > limit {
        return Err(io("oversized"));
    }
    Ok((bytes, meta))
}
fn open(root: &Path, id: Uuid) -> Result<(Meeting, Stamp)> {
    let (bytes, meta) = read_bounded(
        &root.join(id.to_string()).join("meeting.json"),
        16 * 1024 * 1024,
    )?;
    let m: Meeting = serde_json::from_slice(&bytes).map_err(io)?;
    if m.id != id
        || m.chunks
            .iter()
            .try_fold(0usize, |total, c| total.checked_add(c.samples))
            .is_none()
        || m.chunks
            .iter()
            .enumerate()
            .any(|(i, c)| c.index != i || c.samples > 100_000)
    {
        return Err(io("invalid manifest"));
    }
    Ok((m, Stamp::of(&meta)))
}
pub fn read(root: &Path, id: Uuid) -> Result<Meeting> {
    open(root, id).map(|(m, _)| m)
}
fn ids(root: &Path) -> Result<Vec<Uuid>> {
    let mut ids = vec![];
    for e in std::fs::read_dir(root).map_err(io)?.flatten() {
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        if let Ok(id) = Uuid::parse_str(&e.file_name().to_string_lossy()) {
            // A directory without meeting.json is still being created or was never completed.
            if e.path().join("meeting.json").exists() {
                ids.push(id);
            }
        }
    }
    Ok(ids)
}
/// One unreadable meeting must not hide every other meeting.
pub fn list(root: &Path) -> Result<Vec<Meeting>> {
    let mut records = vec![];
    for id in ids(root)? {
        match read(root, id) {
            Ok(m) => records.push(m),
            Err(_) => eprintln!("SpeakMore: skipped unreadable meeting {id}"),
        }
    }
    records.sort_by_key(|m| std::cmp::Reverse(m.created_at));
    Ok(records)
}
/// Parsed list summaries, reused until a meeting.json is replaced.
#[derive(Default)]
pub struct Listing(std::collections::HashMap<Uuid, (Stamp, Meeting)>);
pub fn summarize(m: &mut Meeting) {
    m.participants = m
        .segments
        .iter()
        .filter_map(|seg| seg.speaker.clone())
        .chain(m.items.iter().filter_map(|item| item.owner.clone()))
        .collect();
    m.participants.sort();
    m.participants.dedup();
    m.notes.clear();
    m.artifacts.clear();
    m.dirty_segments.clear();
    m.template_prompt.clear();
    m.segments.clear();
    m.chunks.clear();
    m.items.clear();
    m.speaker_profiles.clear();
}
pub fn summaries(root: &Path, listing: &std::sync::Mutex<Listing>) -> Result<Vec<Meeting>> {
    let ids = ids(root)?;
    let mut cache = listing.lock().unwrap_or_else(|e| e.into_inner());
    let present: std::collections::HashSet<_> = ids.iter().copied().collect();
    cache.0.retain(|id, _| present.contains(id));
    let mut records = vec![];
    for id in ids {
        let Ok(meta) = std::fs::metadata(root.join(id.to_string()).join("meeting.json")) else {
            continue;
        };
        if let Some((stamp, cached)) = cache.0.get(&id) {
            if *stamp == Stamp::of(&meta) {
                records.push(cached.clone());
                continue;
            }
        }
        match open(root, id) {
            Ok((mut m, stamp)) => {
                summarize(&mut m);
                cache.0.insert(id, (stamp, m.clone()));
                records.push(m);
            }
            // Failures are never cached: a transient error (EMFILE, EIO) must not hide the
            // meeting until its file changes.
            Err(_) => {
                eprintln!("SpeakMore: skipped unreadable meeting {id}");
                cache.0.remove(&id);
            }
        }
    }
    records.sort_by_key(|m| std::cmp::Reverse(m.created_at));
    Ok(records)
}
/// Moves a meeting out of the listing in O(1); the caller removes the returned directory later.
pub fn trash(root: &Path, id: Uuid) -> Result<PathBuf> {
    let bin = root.join(".trash");
    std::fs::create_dir_all(&bin).map_err(io)?;
    let target = bin.join(format!("{id}-{}", Uuid::new_v4()));
    std::fs::rename(root.join(id.to_string()), &target).map_err(io)?;
    Ok(target)
}
pub fn empty_trash(root: &Path) {
    if let Ok(entries) = std::fs::read_dir(root.join(".trash")) {
        for e in entries.flatten() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}
pub fn append(root: &Path, m: &mut Meeting, samples: &[f32]) -> Result<()> {
    if samples.is_empty() {
        return Ok(());
    }
    if m.samples().checked_add(samples.len()).is_none()
        || samples.len() > 100_000
        || samples.iter().any(|s| !s.is_finite())
    {
        return Err(io("invalid samples"));
    }
    let dir = root.join(m.id.to_string());
    let index = m.chunks.len();
    let mut temp = tempfile::NamedTempFile::new_in(&dir).map_err(io)?;
    let bytes: Vec<u8> = samples
        .iter()
        .flat_map(|s| s.clamp(-1.0, 1.0).to_le_bytes())
        .collect();
    temp.write_all(&bytes).map_err(io)?;
    temp.as_file().sync_all().map_err(io)?;
    temp.persist(dir.join(format!("{index:05}.pcm")))
        .map_err(io)?;
    m.chunks.push(Chunk {
        index,
        samples: samples.len(),
    });
    m.duration_ms = m.samples() as u64 * 1000 / RATE as u64;
    m.last_audio_at = Some(crate::now());
    m.revision += 1;
    save(root, m)
}
pub fn samples(root: &Path, m: &Meeting, from: usize, count: usize) -> Result<Vec<f32>> {
    use std::io::{Seek, SeekFrom};
    if count > RATE as usize * 60 {
        return Err(io("read bound"));
    }
    let mut out = Vec::with_capacity(count);
    let mut offset = 0;
    for c in &m.chunks {
        let start = from.saturating_sub(offset).min(c.samples);
        let len = (c.samples - start).min(count - out.len());
        if len > 0 {
            let mut file = std::fs::File::open(
                root.join(m.id.to_string())
                    .join(format!("{:05}.pcm", c.index)),
            )
            .map_err(io)?;
            if file.metadata().map_err(io)?.len() != c.samples as u64 * 4 {
                return Err(io("chunk truncated"));
            }
            file.seek(SeekFrom::Start(start as u64 * 4)).map_err(io)?;
            let mut bytes = vec![0; len * 4];
            file.read_exact(&mut bytes).map_err(io)?;
            for b in bytes.chunks_exact(4) {
                let v = f32::from_le_bytes(b.try_into().unwrap());
                if !v.is_finite() {
                    return Err(io("invalid PCM"));
                }
                out.push(v);
            }
        }
        offset += c.samples;
        if out.len() == count {
            break;
        }
    }
    Ok(out)
}

pub fn validate_items(items: &[Item], segments: &[Segment]) -> Result<()> {
    if items.len() > 200 {
        return Err(err("SummaryInvalid", "纪要条目过多，请缩短输入后重试。"));
    }
    for i in items {
        if !["decision", "todo", "fact", "next"].contains(&i.kind.as_str())
            || ![
                "",
                "proposal",
                "decision",
                "commitment",
                "pending",
                "information",
            ]
            .contains(&i.status.as_str())
            || !["", "supported", "review"].contains(&i.support.as_str())
            || i.recipient
                .as_ref()
                .is_some_and(|v| v.chars().count() > 120)
            || i.text.trim().is_empty()
            || i.text.chars().count() > 2000
            || i.evidence.is_empty()
            || i.evidence.len() > 20
            || i.evidence
                .iter()
                .any(|id| !segments.iter().any(|s| &s.id == id))
            || i.owner.as_ref().is_some_and(|s| s.chars().count() > 120)
            || i.due.as_ref().is_some_and(|s| s.chars().count() > 120)
        {
            return Err(err(
                "SummaryInvalid",
                "纪要的原文依据或内容格式无效，已保留逐字稿。",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunks_reopen_and_ranges_cross_boundaries() {
        let d = tempfile::tempdir().unwrap();
        let mut m = Meeting::new("test".into(), "microphone", "zh".into());
        save(d.path(), &m).unwrap();
        append(d.path(), &mut m, &vec![0.25; 80000]).unwrap();
        append(d.path(), &mut m, &vec![-0.5; 4000]).unwrap();
        let r = read(d.path(), m.id).unwrap();
        assert_eq!(r.duration_ms, 5250);
        assert_eq!(
            samples(d.path(), &r, 79998, 4).unwrap(),
            vec![0.25, 0.25, -0.5, -0.5]
        );
        std::fs::write(d.path().join(m.id.to_string()).join("00001.pcm"), []).unwrap();
        assert!(samples(d.path(), &r, 80000, 4).is_err());
    }
    #[test]
    fn three_hour_recording_reopens_appends_and_reads_past_the_old_limit() {
        use std::io::{Seek, SeekFrom};
        let d = tempfile::tempdir().unwrap();
        let mut m = Meeting::new("long meeting".into(), "mixed", "auto".into());
        save(d.path(), &m).unwrap();
        // Sparse PCM files represent the full duration without writing hours of silence.
        for index in 0..3 * 3600 / 5 {
            let mut file = std::fs::File::create(
                d.path()
                    .join(m.id.to_string())
                    .join(format!("{index:05}.pcm")),
            )
            .unwrap();
            file.set_len(80000 * 4).unwrap();
            file.write_all(&0.25f32.to_le_bytes()).unwrap();
            file.seek(SeekFrom::End(-4)).unwrap();
            file.write_all(&0.5f32.to_le_bytes()).unwrap();
            m.chunks.push(Chunk {
                index,
                samples: 80000,
            });
        }
        m.duration_ms = m.samples() as u64 / 16;
        m.processed_samples = IMPORT_LIMIT;
        m.state = "transcribing".into();
        save(d.path(), &m).unwrap();
        let mut reopened = read(d.path(), m.id).unwrap();
        assert_eq!(reopened.id, m.id);
        assert_eq!(reopened.duration_ms, 3 * 3600000);
        assert_eq!(reopened.processed_samples, IMPORT_LIMIT);
        for boundary in [
            IMPORT_LIMIT,
            2 * IMPORT_LIMIT,
            3 * IMPORT_LIMIT - 30 * RATE as usize,
        ] {
            assert_eq!(
                samples(d.path(), &reopened, boundary - 1, 2).unwrap(),
                vec![0.5, 0.25]
            );
            assert_eq!(
                samples(d.path(), &reopened, boundary, 30 * RATE as usize)
                    .unwrap()
                    .len(),
                30 * RATE as usize
            );
        }
        save_track(d.path(), &reopened, "microphone", &[0.75; 4000]).unwrap();
        save_track(d.path(), &reopened, "system", &[0.25; 4000]).unwrap();
        append(d.path(), &mut reopened, &[0.5; 4000]).unwrap();
        let reopened = read(d.path(), m.id).unwrap();
        assert_eq!(reopened.duration_ms, 3 * 3600000 + 250);
        assert_eq!(
            samples(d.path(), &reopened, 3 * IMPORT_LIMIT - 1, 3).unwrap(),
            vec![0.5; 3]
        );
        assert_eq!(list(d.path()).unwrap().len(), 1);
    }
    #[test]
    fn resuming_after_interruption_preserves_id_progress_and_speaker_names() {
        let d = tempfile::tempdir().unwrap();
        let mut m = Meeting::new("resume".into(), "microphone", "zh".into());
        save(d.path(), &m).unwrap();
        append(d.path(), &mut m, &[0.25; 8000]).unwrap();
        m.processed_samples = 8000;
        m.segments.push(Segment {
            id: "s1".into(),
            start_ms: 0,
            end_ms: 500,
            text: "预算五十万".into(),
            speaker: Some("李明".into()),
            speaker_id: Some("p1".into()),
        });
        m.speaker_names.insert("p1".into(), "李明".into());
        m.gaps.push(RecordingGap {
            after_ms: 500,
            started_at: 1000,
            ended_at: Some(61000),
            reason: "设备断开".into(),
        });
        save(d.path(), &m).unwrap();
        let mut resumed = read(d.path(), m.id).unwrap();
        append(d.path(), &mut resumed, &[0.5; 4000]).unwrap();
        let reopened = read(d.path(), m.id).unwrap();
        assert_eq!(reopened.id, m.id);
        assert_eq!(reopened.processed_samples, 8000);
        assert_eq!(reopened.duration_ms, 750);
        assert_eq!(
            reopened.speaker_names.get("p1").map(String::as_str),
            Some("李明")
        );
        assert_eq!(reopened.segments[0].id, "s1");
        assert_eq!(reopened.gaps[0].ended_at, Some(61000));
        assert_eq!(samples(d.path(), &reopened, 7999, 2).unwrap(), [0.25, 0.5]);
        assert_eq!(list(d.path()).unwrap().len(), 1);
    }
    #[test]
    fn unreadable_meetings_are_skipped_and_cached_summaries_follow_saves() {
        let d = tempfile::tempdir().unwrap();
        let mut m = Meeting::new("ok".into(), "microphone", "zh".into());
        m.segments.push(Segment {
            id: "s1".into(),
            start_ms: 0,
            end_ms: 500,
            text: "预算".into(),
            speaker: Some("李明".into()),
            speaker_id: None,
        });
        save(d.path(), &m).unwrap();
        let broken = d.path().join(Uuid::new_v4().to_string());
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("meeting.json"), b"{").unwrap();
        std::fs::create_dir_all(d.path().join(Uuid::new_v4().to_string())).unwrap();
        assert_eq!(list(d.path()).unwrap().len(), 1);
        let listing = std::sync::Mutex::new(Listing::default());
        let first = summaries(d.path(), &listing).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(
            listing.lock().unwrap().0.len(),
            1,
            "failures are not cached"
        );
        assert!(first[0].segments.is_empty());
        assert_eq!(first[0].participants, ["李明"]);
        m.title = "renamed".into();
        m.revision += 1;
        save(d.path(), &m).unwrap();
        assert_eq!(summaries(d.path(), &listing).unwrap()[0].title, "renamed");
        let trashed = trash(d.path(), m.id).unwrap();
        assert!(summaries(d.path(), &listing).unwrap().is_empty());
        assert!(list(d.path()).unwrap().is_empty());
        empty_trash(d.path());
        assert!(!trashed.exists());
    }
    #[test]
    fn unsupported_evidence_is_rejected() {
        let i = Item {
            kind: "todo".into(),
            text: "check".into(),
            owner: None,
            due: None,
            evidence: vec!["unknown".into()],
            done: false,
            ..Default::default()
        };
        assert!(validate_items(&[i], &[]).is_err());
    }
}

fn retention_default() -> Option<u32> {
    None
} // Existing recordings retain their original policy.
pub fn save_track(root: &Path, m: &Meeting, track: &str, samples: &[f32]) -> Result<()> {
    let dir = root.join(m.id.to_string());
    let mut f = tempfile::NamedTempFile::new_in(&dir).map_err(io)?;
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    f.write_all(&bytes).map_err(io)?;
    f.as_file().sync_all().map_err(io)?;
    f.persist(dir.join(format!("{}-{:05}.pcm", track, m.chunks.len())))
        .map_err(io)?;
    Ok(())
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    #[test]
    fn existing_meetings_keep_audio_and_speaker_migration_is_nullable() {
        let m = Meeting::new("old".into(), "microphone", "auto".into());
        let mut v = serde_json::to_value(m).unwrap();
        v.as_object_mut().unwrap().remove("audioDeleted");
        v.as_object_mut().unwrap().remove("retentionDays");
        v.as_object_mut().unwrap().remove("contextRevision");
        v.as_object_mut()
            .unwrap()
            .remove("summarizedContextRevision");
        let old: Meeting = serde_json::from_value(v).unwrap();
        assert_eq!(old.retention_days, None);
        assert!(!old.audio_deleted);
        assert_eq!(old.context_revision, 0);
        assert_eq!(old.summarized_context_revision, 0);
        let segment: Segment = serde_json::from_value(
            serde_json::json!({"id":"s1","startMs":0,"endMs":1000,"text":"old"}),
        )
        .unwrap();
        assert!(segment.speaker.is_none());
    }
    #[test]
    fn exported_notes_keep_proposals_and_personal_additions_distinct() {
        let mut m = Meeting::new("评审".into(), "microphone", "zh".into());
        m.items.push(Item {
            kind: "next".into(),
            status: "proposal".into(),
            support: "review".into(),
            text: "考虑下周上线".into(),
            ..Default::default()
        });
        m.notes.push(Note {
            id: Uuid::new_v4(),
            at_ms: 120000,
            text: "我认为预算存在风险".into(),
            kind: "supplement".into(),
            image: None,
        });
        let output = m.text();
        assert!(output.contains("考虑下周上线 · 提议 · 依据待核对"));
        assert!(output.contains("我的关注与补充（与会议事实分别保留）"));
        assert!(output.contains("[个人补充] [2:00] 我认为预算存在风险"));
    }
}
