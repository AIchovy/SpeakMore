use super::*;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Term {
    pub heard: String,
    pub replacement: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub my_name: String,
    pub default_template: String,
    pub terms: Vec<Term>,
    #[serde(default)]
    pub caption_translation_enabled: bool,
    #[serde(default)]
    pub template_rules: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub experience_shortcuts: bool,
}
pub fn preferences(s: &Service) -> Result<Preferences> {
    let path = s.0.root.join("workspace.json");
    if !path.exists() {
        return Ok(Preferences::default());
    }
    let file = std::fs::File::open(path).map_err(store::io)?;
    if file.metadata().map_err(store::io)?.len() > 512 * 1024 {
        return Err(store::io("preferences bound"));
    }
    serde_json::from_reader(file).map_err(store::io)
}
pub fn template_instruction(template: &str) -> Result<&'static str> {
    match template {
        "" | "general" => Ok("突出明确的决策、任务、关键结论和下一步。"),
        "customer" => Ok("客户沟通：突出客户需求、顾虑、报价与承诺、待确认问题和跟进任务。"),
        "review" => {
            Ok("产品评审：突出通过/否决的方案、验收条件、争议与未决问题、负责人和交付任务。")
        }
        "weekly" => Ok("项目周会：突出进度、阻塞、风险、负责人与截止日期、下一次检查事项。"),
        "interview" => Ok("面试：保留候选人的关键回答与原文事实、待追问点；不虚构评价或录用决策。"),
        "learning" => Ok(
            "课程学习：突出知识结构、概念、例子、明确结论和复习重点；没有真实任务时不虚构待办。",
        ),
        "one_on_one" => Ok("一对一：突出双方关注事项、支持需求、明确承诺和后续行动。"),
        _ => Err(AppError::invalid("请选择有效的会议模板。")),
    }
}
fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}
pub fn apply_terms(text: &str, terms: &[Term]) -> String {
    let mut terms: Vec<_> = terms.iter().filter(|t| !t.heard.is_empty()).collect();
    terms.sort_by_key(|t| std::cmp::Reverse(t.heard.len()));
    let mut output = String::new();
    let mut offset = 0;
    while offset < text.len() {
        let matched = terms.iter().find(|t| {
            text[offset..].starts_with(&t.heard)
                && !(t.heard.chars().next().is_some_and(word)
                    && text[..offset].chars().next_back().is_some_and(word))
                && !(t.heard.chars().next_back().is_some_and(word)
                    && text[offset + t.heard.len()..]
                        .chars()
                        .next()
                        .is_some_and(word))
        });
        if let Some(t) = matched {
            output.push_str(&t.replacement);
            offset += t.heard.len();
        } else {
            let c = text[offset..].chars().next().unwrap();
            output.push(c);
            offset += c.len_utf8();
        }
    }
    output
}
#[tauri::command]
pub fn meeting_preferences(
    window: WebviewWindow,
    state: State<'_, Managed>,
) -> Result<Preferences> {
    crate::main_only(&window)?;
    preferences(&get(&state)?)
}
#[tauri::command]
pub fn meeting_preferences_save(
    window: WebviewWindow,
    state: State<'_, Managed>,
    preferences: Preferences,
) -> Result<()> {
    crate::main_only(&window)?;
    template_instruction(&preferences.default_template)?;
    if preferences.template_rules.len() > 7
        || preferences
            .template_rules
            .iter()
            .any(|(key, value)| template_instruction(key).is_err() || value.chars().count() > 2000)
    {
        return Err(AppError::invalid("模板要求最多 2000 个字。"));
    }
    if preferences.my_name.chars().count() > 120
        || preferences.terms.len() > 300
        || preferences.terms.iter().any(|t| {
            t.heard.trim().is_empty()
                || t.replacement.trim().is_empty()
                || t.heard.chars().count() > 120
                || t.replacement.chars().count() > 120
        })
    {
        return Err(AppError::invalid("请填写有效的术语和姓名。"));
    }
    let mut seen = std::collections::HashSet::new();
    if preferences
        .terms
        .iter()
        .any(|t| !seen.insert(t.heard.as_str()))
    {
        return Err(AppError::invalid("同一术语只能设置一项修正。"));
    }
    write_preferences(&get(&state)?, &preferences)
}
pub(super) fn write_preferences(s: &Service, preferences: &Preferences) -> Result<()> {
    let _lock = s.0.lock.lock().map_err(store::io)?;
    let mut file = tempfile::NamedTempFile::new_in(&s.0.root).map_err(store::io)?;
    serde_json::to_writer(&mut file, preferences).map_err(store::io)?;
    file.as_file().sync_all().map_err(store::io)?;
    file.persist(s.0.root.join("workspace.json"))
        .map_err(store::io)?;
    s.changed();
    Ok(())
}

#[tauri::command]
pub fn meeting_template_choose(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    revision: u64,
    template: String,
) -> Result<()> {
    crate::main_only(&window)?;
    template_instruction(&template)?;
    let s = get(&state)?;
    if s.meeting_busy(id) {
        return Err(AppError::busy());
    }
    s.edit(id, |m| {
        if m.revision != revision {
            return Err(AppError::conflict());
        }
        m.template_prompt = preferences(&s)?
            .template_rules
            .get(&template)
            .cloned()
            .unwrap_or_default();
        m.template = template;
        m.context_changed();
        m.summary_stale = !m.items.is_empty();
        Ok(())
    })
}
#[tauri::command]
pub fn meeting_speaker_rename(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    revision: u64,
    speaker_id: String,
    name: String,
    merge_with: Option<String>,
) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    if s.meeting_busy(id) {
        return Err(AppError::busy());
    }
    if name.trim().is_empty() || name.chars().count() > 80 {
        return Err(AppError::invalid("说话人名称请填写 1–80 个字。"));
    }
    s.edit(id, |m| {
        if m.revision != revision {
            return Err(AppError::conflict());
        }
        if !m
            .segments
            .iter()
            .any(|seg| seg.speaker_id.as_deref().or(seg.speaker.as_deref()) == Some(&speaker_id))
        {
            return Err(AppError::invalid("说话人不存在。"));
        }
        let target = merge_with.as_deref().unwrap_or(&speaker_id);
        if merge_with.is_some()
            && !m
                .segments
                .iter()
                .any(|seg| seg.speaker_id.as_deref() == Some(target))
        {
            return Err(AppError::invalid("请选择已有的说话人。"));
        }
        advanced::archive(&s, m)?;
        if target != speaker_id {
            m.speaker_names.remove(&speaker_id);
        }
        m.speaker_names.insert(target.into(), name.trim().into());
        for seg in &mut m.segments {
            if seg.speaker_id.as_deref().or(seg.speaker.as_deref()) == Some(&speaker_id)
                || seg.speaker_id.as_deref() == Some(target)
            {
                seg.speaker_id = Some(target.into());
                seg.speaker = Some(name.trim().into());
            }
        }
        for profile in &mut m.speaker_profiles {
            if profile.id == speaker_id {
                profile.id = target.into();
            }
        }
        m.summary_stale = true;
        Ok(())
    })
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub meeting_id: Uuid,
    pub title: String,
    pub created_at: i64,
    pub snippet: String,
    pub segment_id: Option<String>,
    pub start_ms: Option<u64>,
}
fn snippet(text: &str, query: &str) -> String {
    let lower = text.to_lowercase();
    let position = lower
        .find(query)
        .map(|pos| lower[..pos].chars().count())
        .unwrap_or(0);
    let chars: Vec<_> = text.chars().collect();
    let start = position.saturating_sub(50).min(chars.len());
    let end = (start + 240).min(chars.len());
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        chars[start..end].iter().collect::<String>(),
        if end < chars.len() { "…" } else { "" }
    )
}
pub fn search_records(meetings: &[Meeting], query: &str) -> Vec<SearchHit> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return vec![];
    }
    let mut hits = vec![];
    for m in meetings {
        let title_match = m.title.to_lowercase().contains(&query);
        let mut found = false;
        for seg in m.segments.iter().filter(|seg| {
            seg.text.to_lowercase().contains(&query)
                || seg
                    .speaker
                    .as_ref()
                    .is_some_and(|name| name.to_lowercase().contains(&query))
        }) {
            hits.push(SearchHit {
                meeting_id: m.id,
                title: m.title.clone(),
                created_at: m.created_at,
                snippet: snippet(&seg.text, &query),
                segment_id: Some(seg.id.clone()),
                start_ms: Some(seg.start_ms),
            });
            found = true;
            if hits.len() == 200 {
                return hits;
            }
        }
        for item in m.items.iter().filter(|i| {
            i.text.to_lowercase().contains(&query)
                || i.owner
                    .as_ref()
                    .is_some_and(|owner| owner.to_lowercase().contains(&query))
        }) {
            let segment = item
                .evidence
                .first()
                .and_then(|id| m.segments.iter().find(|s| &s.id == id));
            hits.push(SearchHit {
                meeting_id: m.id,
                title: m.title.clone(),
                created_at: m.created_at,
                snippet: item.text.clone(),
                segment_id: segment.map(|s| s.id.clone()),
                start_ms: segment.map(|s| s.start_ms),
            });
            found = true;
            if hits.len() == 200 {
                return hits;
            }
        }
        if title_match && !found {
            hits.push(SearchHit {
                meeting_id: m.id,
                title: m.title.clone(),
                created_at: m.created_at,
                snippet: m.title.clone(),
                segment_id: None,
                start_ms: None,
            });
        }
        if hits.len() == 200 {
            return hits;
        }
    }
    hits
}
#[tauri::command]
pub async fn meeting_search(
    window: WebviewWindow,
    state: State<'_, Managed>,
    query: String,
    project: Option<String>,
    after: Option<i64>,
    before: Option<i64>,
    speaker: Option<String>,
) -> Result<Vec<SearchHit>> {
    crate::main_only(&window)?;
    if query.chars().count() > 200 {
        return Err(AppError::invalid("搜索文字过长。"));
    }
    let s = get(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut records = store::list(&s.0.root)?;
        records.retain(|m| {
            project
                .as_ref()
                .is_none_or(|p| m.project.to_lowercase().contains(&p.trim().to_lowercase()))
                && after.is_none_or(|v| m.created_at >= v)
                && before.is_none_or(|v| m.created_at <= v)
                && speaker.as_ref().is_none_or(|v| {
                    v.is_empty()
                        || m.segments.iter().any(|s| {
                            s.speaker
                                .as_ref()
                                .is_some_and(|name| name.to_lowercase().contains(&v.to_lowercase()))
                        })
                        || m.items.iter().any(|i| {
                            i.owner
                                .as_ref()
                                .is_some_and(|name| name.to_lowercase().contains(&v.to_lowercase()))
                        })
                })
        });
        Ok(search_records(&records, &query))
    })
    .await
    .map_err(store::io)?
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub meeting_id: Uuid,
    pub title: String,
    pub project: String,
    pub created_at: i64,
    pub revision: u64,
    pub index: usize,
    pub item: store::Item,
    pub summary_stale: bool,
    pub state: String,
}
#[tauri::command]
pub async fn meeting_tasks(window: WebviewWindow, state: State<'_, Managed>) -> Result<Vec<Task>> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut tasks = vec![];
        for m in store::list(&s.0.root)? {
            for (index, item) in m.items.iter().enumerate() {
                if item.kind == "todo" {
                    tasks.push(Task {
                        meeting_id: m.id,
                        title: m.title.clone(),
                        project: m.project.clone(),
                        created_at: m.created_at,
                        revision: m.revision,
                        index,
                        item: item.clone(),
                        summary_stale: m.summary_stale,
                        state: m.state.clone(),
                    });
                }
            }
        }
        Ok(tasks)
    })
    .await
    .map_err(store::io)?
}

pub(super) fn task_confirmed(item: &store::Item, stale: bool) -> bool {
    item.kind == "todo"
        && ["", "commitment"].contains(&item.status.as_str())
        && item.support == "supported"
        && (!stale || item.reviewed)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskEdit {
    text: String,
    owner: Option<String>,
    due: Option<String>,
    recipient: Option<String>,
    confirmed: bool,
}
fn clean_name(value: Option<String>) -> Option<String> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}
fn edit_task(item: &mut store::Item, edit: TaskEdit, segments: &[store::Segment]) -> Result<()> {
    if item.kind != "todo" {
        return Err(AppError::invalid("待办不存在。"));
    }
    let owner = clean_name(edit.owner);
    let recipient = clean_name(edit.recipient);
    let due = clean_name(edit.due);
    if due != item.due {
        if let Some(value) = &due {
            let date = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|_| AppError::invalid("请填写有效的具体日期，或留空待确认。"))?;
            if date.format("%Y-%m-%d").to_string() != *value {
                return Err(AppError::invalid("请填写有效的具体日期，或留空待确认。"));
            }
        }
    }
    if edit.confirmed && !["", "commitment"].contains(&item.status.as_str()) {
        return Err(AppError::invalid(
            "这条内容尚不是明确承诺，请在来源会议中检查内容性质。",
        ));
    }
    let text = edit.text.trim().to_string();
    if text != item.text || owner != item.owner || due != item.due || recipient != item.recipient {
        item.edited_from.get_or_insert(item.text.clone());
        item.edited = true;
        item.reviewed = false;
        item.support = "review".into();
    }
    item.text = text;
    item.owner = owner;
    item.due = due;
    item.recipient = recipient;
    if edit.confirmed {
        item.reviewed = true;
        item.support = "supported".into();
    }
    // Older live results could collect more sources than an edited item may keep.
    item.evidence.truncate(20);
    store::validate_items(std::slice::from_ref(item), segments)
}
#[tauri::command]
pub fn meeting_task_update(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    revision: u64,
    index: usize,
    edit: TaskEdit,
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
        let mut item = m
            .items
            .get(index)
            .cloned()
            .ok_or_else(|| AppError::invalid("待办不存在。"))?;
        edit_task(&mut item, edit, &m.segments)?;
        advanced::archive(&s, m)?;
        m.items[index] = item;
        Ok(())
    })
}
#[tauri::command]
pub fn meeting_task_toggle(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    revision: u64,
    index: usize,
    done: bool,
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
        if m.items.get(index).is_none_or(|item| item.kind != "todo") {
            return Err(AppError::invalid("待办不存在。"));
        }
        advanced::archive(&s, m)?;
        m.items[index].done = done;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn followup_fixture() -> (store::Item, Vec<store::Segment>) {
        (
            store::Item {
                kind: "todo".into(),
                text: "发送报价".into(),
                owner: Some("李明".into()),
                due: Some("周五前".into()),
                evidence: vec!["s1".into()],
                support: "supported".into(),
                status: "commitment".into(),
                ..Default::default()
            },
            vec![store::Segment {
                id: "s1".into(),
                start_ms: 0,
                end_ms: 1000,
                text: "李明发送报价给陈雪".into(),
                speaker: None,
                speaker_id: None,
            }],
        )
    }
    #[test]
    fn task_field_corrections_preserve_completion_and_require_a_new_source_check() {
        let (mut item, segments) = followup_fixture();
        item.done = true;
        item.reviewed = true;
        edit_task(
            &mut item,
            TaskEdit {
                text: "  发送报价  ".into(),
                owner: Some(" 陈雪 ".into()),
                due: Some("2026-10-09".into()),
                recipient: Some(" 李明 ".into()),
                confirmed: false,
            },
            &segments,
        )
        .unwrap();
        assert!(item.done);
        assert!(item.edited);
        assert_eq!(item.owner.as_deref(), Some("陈雪"));
        assert_eq!(item.recipient.as_deref(), Some("李明"));
        assert_eq!(item.due.as_deref(), Some("2026-10-09"));
        assert_eq!(item.support, "review");
        assert!(!item.reviewed);
        assert_eq!(item.evidence, vec!["s1"]);
    }
    #[test]
    fn confirming_existing_fields_does_not_claim_a_manual_edit_or_promote_a_proposal() {
        let (mut item, segments) = followup_fixture();
        let edit = || TaskEdit {
            text: "发送报价".into(),
            owner: Some("李明".into()),
            due: Some("周五前".into()),
            recipient: None,
            confirmed: true,
        };
        edit_task(&mut item, edit(), &segments).unwrap();
        assert!(item.reviewed);
        assert!(!item.edited);
        assert!(!item.done);
        assert!(task_confirmed(&item, true));
        item.status = "proposal".into();
        assert!(edit_task(&mut item, edit(), &segments).is_err());
        assert!(!task_confirmed(&item, false));
    }
    #[test]
    fn impossible_changed_dates_and_missing_evidence_cannot_be_confirmed() {
        let (mut item, segments) = followup_fixture();
        let edit = || TaskEdit {
            text: "发送报价".into(),
            owner: Some("李明".into()),
            due: Some("2026-02-30".into()),
            recipient: None,
            confirmed: true,
        };
        assert!(edit_task(&mut item, edit(), &segments).is_err());
        assert!(!item.reviewed);
        let mut invalid = edit();
        invalid.due = item.due.clone();
        assert!(edit_task(&mut item, invalid, &[]).is_err());
    }
    #[test]
    fn search_snippets_include_late_unicode_matches_and_owners_without_audio() {
        let text = format!("{}预算五十万", "原文".repeat(300));
        assert!(snippet(&text, "预算").contains("预算五十万"));
        let mut m = Meeting::new("customer".into(), "microphone", "zh".into());
        m.audio_deleted = true;
        m.segments.push(store::Segment {
            id: "s1".into(),
            start_ms: 0,
            end_ms: 1000,
            text,
            speaker: Some("李明".into()),
            speaker_id: Some("p1".into()),
        });
        m.items.push(store::Item {
            kind: "todo".into(),
            text: "发送报价".into(),
            owner: Some("陈雪".into()),
            due: None,
            evidence: vec!["s1".into()],
            done: true,
            ..Default::default()
        });
        assert_eq!(
            search_records(&[m.clone()], "预算")[0]
                .segment_id
                .as_deref(),
            Some("s1")
        );
        assert_eq!(search_records(&[m], "陈雪")[0].start_ms, Some(0));
    }
    #[test]
    fn templates_and_legacy_preferences_are_compatible() {
        for template in [
            "general",
            "customer",
            "review",
            "weekly",
            "interview",
            "learning",
            "one_on_one",
        ] {
            assert!(template_instruction(template).is_ok());
        }
        assert!(template_instruction("invented").is_err());
        let p: Preferences =
            serde_json::from_str(r#"{"myName":"李明","defaultTemplate":"review","terms":[]}"#)
                .unwrap();
        assert!(!p.caption_translation_enabled);
    }
    #[test]
    fn terms_are_unicode_safe_non_recursive_and_respect_word_boundaries() {
        let terms = vec![
            Term {
                heard: "飞书".into(),
                replacement: "Lark".into(),
            },
            Term {
                heard: "cat".into(),
                replacement: "dog".into(),
            },
            Term {
                heard: "Lark".into(),
                replacement: "wrong".into(),
            },
        ];
        assert_eq!(apply_terms("飞书 cat catalog", &terms), "Lark dog catalog");
    }
    #[test]
    fn search_finds_the_final_hour_and_keeps_source_location() {
        let mut meeting = Meeting::new("review".into(), "microphone", "zh".into());
        meeting.segments.push(store::Segment {
            id: "late".into(),
            start_ms: 7200000,
            end_ms: 7201000,
            text: "预算五十万".into(),
            speaker: None,
            speaker_id: None,
        });
        let hits = search_records(&[meeting.clone()], "预算");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].meeting_id, meeting.id);
        assert_eq!(hits[0].start_ms, Some(7200000));
    }
}
