use super::*;
#[cfg(test)]
use serde_json::Value;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preference {
    pub provider_id: Option<Uuid>,
    #[serde(default)]
    pub follow_ai: bool,
}

impl Default for Preference {
    fn default() -> Self {
        Self {
            provider_id: None,
            follow_ai: true,
        }
    }
}
impl Preference {
    pub fn resolve(&self, s: &Service) -> Result<Option<Uuid>> {
        if self.follow_ai {
            s.0.runtime
                .snapshot()?
                .settings
                .active_provider_id
                .map(|id| Uuid::parse_str(&id).map_err(store::io))
                .transpose()
        } else {
            Ok(self.provider_id)
        }
    }
}
fn preference_path(s: &Service, purpose: &str) -> Result<PathBuf> {
    let name = match purpose {
        "meeting" => "summary-settings.json",
        "captions" => "caption-text-settings.json",
        _ => return Err(AppError::invalid("文字任务无效。")),
    };
    Ok(s.0.root.join(name))
}
pub fn preference(s: &Service) -> Result<Preference> {
    preference_for(s, "meeting")
}
pub fn preference_for(s: &Service, purpose: &str) -> Result<Preference> {
    let path = preference_path(s, purpose)?;
    if !path.exists() {
        return Ok(Preference::default());
    }
    serde_json::from_reader(std::fs::File::open(path).map_err(store::io)?).map_err(store::io)
}

#[tauri::command]
pub fn meeting_summary_settings(
    window: WebviewWindow,
    state: State<'_, Managed>,
    purpose: Option<String>,
) -> Result<Preference> {
    crate::main_only(&window)?;
    preference_for(&get(&state)?, purpose.as_deref().unwrap_or("meeting"))
}

#[tauri::command]
pub async fn meeting_summary_choose(
    window: WebviewWindow,
    state: State<'_, Managed>,
    provider_id: Option<Uuid>,
    meeting_id: Option<Uuid>,
    revision: Option<u64>,
    follow_ai: Option<bool>,
    purpose: Option<String>,
) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    let purpose = purpose.as_deref().unwrap_or("meeting");
    let path = preference_path(&s, purpose)?;
    if purpose == "captions" && meeting_id.is_some() {
        return Err(AppError::invalid("字幕设置不绑定会议。"));
    }
    let preference = Preference {
        provider_id,
        follow_ai: follow_ai.unwrap_or(false),
    };
    if meeting_id.is_some_and(|id| s.meeting_busy(id)) {
        return Err(AppError::busy());
    }
    if let Some(id) = preference.resolve(&s)? {
        let provider = s.0.app.state::<crate::Backend>().providers.clone()?;
        provider.validate_meeting_provider(id).await?;
    }

    if let Some(id) = meeting_id {
        s.edit(id, |m| {
            if Some(m.revision) != revision {
                return Err(AppError::conflict());
            }
            m.summary_provider_id = provider_id;
            m.summary_follow_ai = preference.follow_ai;
            m.summary_stale = !m.items.is_empty();
            Ok(())
        })?;
    } else {
        let mut f = tempfile::NamedTempFile::new_in(&s.0.root).map_err(store::io)?;
        serde_json::to_writer(&mut f, &preference).map_err(store::io)?;
        f.as_file().sync_all().map_err(store::io)?;
        f.persist(path).map_err(store::io)?;
        s.changed();
    }
    Ok(())
}

fn parse(raw: &str) -> Result<Vec<store::Item>> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Response {
        items: Vec<store::Item>,
    }
    let raw = raw.trim();
    let raw = if raw.starts_with("```json\n") || raw.starts_with("```\n") {
        raw.split_once('\n')
            .and_then(|(_, s)| s.strip_suffix("```"))
            .ok_or_else(|| err("SummaryInvalid", "服务返回的纪要不完整，逐字稿已保留。"))?
            .trim()
    } else {
        raw
    };
    let mut response: Response = serde_json::from_str(raw).map_err(|_| {
        err(
            "SummaryInvalid",
            "服务未返回有效纪要，请检查所选模型或重试。",
        )
    })?;
    for item in &mut response.items {
        item.done = false;
        item.edited = false;
        item.reviewed = false;
    }
    Ok(response.items)
}

pub fn run(s: &Service, m: &Meeting, cancel: &AtomicBool) -> Result<Vec<store::Item>> {
    let Some(id) = m.summary_provider_id else {
        return text::summarize(s, m, cancel);
    };
    let provider = s.0.app.state::<crate::Backend>().providers.clone()?;
    let schema = text::schema();
    text::summarize_with(m, cancel, |instructions, data| {
        let prompt = format!(
            "{instructions}\n输出须符合以下 JSON Schema，不使用 Markdown 或额外解释：\n{schema}"
        );
        let raw = tauri::async_runtime::block_on(provider.generate_text(
            id,
            &prompt,
            data,
            crate::providers::TextTask::Meeting,
            cancel,
        ))?;
        parse(&raw)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_service_default_does_not_change_an_explicit_local_preference() {
        assert!(Preference::default().follow_ai);
        let explicit: Preference = serde_json::from_str(r#"{"providerId":null}"#).unwrap();
        assert!(!explicit.follow_ai);
        let shared: Preference =
            serde_json::from_str(r#"{"providerId":null,"followAi":true}"#).unwrap();
        assert!(shared.follow_ai);
    }
    #[test]
    fn service_summary_accepts_json_but_rejects_partial_or_unrelated_output() {
        assert!(parse("```json\n{\"items\":[]}\n```").unwrap().is_empty());
        assert!(parse("{\"items\":[]}").unwrap().is_empty());
        assert!(parse("{\"items\": [").is_err());
        assert!(parse("Here is your summary: {}").is_err());
        assert!(parse("{\"items\":[],\"command\":\"upload\"}").is_err());
    }
    #[test]
    fn legacy_meetings_keep_local_summary_even_if_global_default_changes() {
        let meeting = Meeting::new("test".into(), "import", "zh".into());
        let mut value = serde_json::to_value(meeting).unwrap();
        value.as_object_mut().unwrap().remove("summaryProviderId");
        value.as_object_mut().unwrap().remove("summaryFollowAi");
        let restored: Meeting = serde_json::from_value::<Value>(value)
            .and_then(serde_json::from_value)
            .unwrap();
        assert_eq!(restored.summary_provider_id, None);
        assert!(!restored.summary_follow_ai);
    }
}
