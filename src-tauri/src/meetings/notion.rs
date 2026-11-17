use super::*;
use serde_json::{json, Value};
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub parent_id: String,
    pub connected: bool,
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Export {
    pub state: String,
    pub page_id: Option<String>,
    pub url: Option<String>,
    pub next: usize,
    pub revision: u64,
    pub marker: String,
    pub blocks: Vec<Value>,
    #[serde(default)]
    pub transcript: Option<bool>,
}
fn key() -> Result<keyring::Entry> {
    keyring::Entry::new("com.speakmore.desktop.notion", "internal-connection")
        .map_err(|_| err("Keychain", "无法访问 Notion 钥匙串。"))
}
fn config(s: &Service) -> Result<Connection> {
    let p = s.0.root.join("notion.json");
    if !p.exists() {
        return Ok(Connection::default());
    }
    serde_json::from_reader(std::fs::File::open(p).map_err(store::io)?).map_err(store::io)
}
fn write(s: &Service, name: &str, value: &impl Serialize) -> Result<()> {
    let mut f = tempfile::NamedTempFile::new_in(&s.0.root).map_err(store::io)?;
    serde_json::to_writer(&mut f, value).map_err(store::io)?;
    f.as_file().sync_all().map_err(store::io)?;
    f.persist(s.0.root.join(name)).map_err(store::io)?;
    Ok(())
}
fn journal(s: &Service, id: Uuid) -> Result<Export> {
    let p = s.0.root.join(format!("notion-{id}.json"));
    if !p.exists() {
        return Ok(Export::default());
    }
    serde_json::from_reader(std::fs::File::open(p).map_err(store::io)?).map_err(store::io)
}
// Notion limits rich text by UTF-16 code units, so emoji count twice.
fn utf16_prefix(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (index, c) in text.char_indices() {
        used += c.len_utf16();
        if used > units {
            return &text[..index];
        }
    }
    text
}
fn parts(mut text: &str, units: usize) -> Vec<&str> {
    let mut out = vec![];
    while !text.is_empty() {
        let part = utf16_prefix(text, units.max(2));
        out.push(part);
        text = &text[part.len()..];
    }
    out
}
fn rich(text: &str) -> Value {
    json!([{"type":"text","text":{"content":utf16_prefix(text, 1900)}}])
}
fn same_exported_block(expected: &Value, actual: &Value) -> bool {
    let Some(kind) = expected["type"].as_str() else {
        return false;
    };
    if actual["type"].as_str() != Some(kind)
        || actual["archived"].as_bool() == Some(true)
        || actual["in_trash"].as_bool() == Some(true)
        || actual["has_children"].as_bool() == Some(true)
    {
        return false;
    }
    let text = |block: &Value| -> Option<String> {
        block[kind]["rich_text"]
            .as_array()?
            .iter()
            .map(|r| r["text"]["content"].as_str())
            .collect::<Option<String>>()
    };
    text(expected).is_some()
        && text(expected) == text(actual)
        && (kind != "to_do" || expected[kind]["checked"] == actual[kind]["checked"])
}
fn blocks(m: &Meeting, transcript: bool) -> Vec<Value> {
    let mut out = vec![];
    for (kind, label) in [
        ("decision", "决策事项"),
        ("todo", "待办清单"),
        ("fact", "关键数字与结论"),
        ("next", "下一步行动"),
    ] {
        out.push(
            json!({"object":"block","type":"heading_2","heading_2":{"rich_text":rich(label)}}),
        );
        for item in m.items.iter().filter(|i| i.kind == kind) {
            let text = format!(
                "{}{}{} · 依据 {}",
                item.text,
                item.owner
                    .as_ref()
                    .map(|s| format!(" · {s}"))
                    .unwrap_or_default(),
                item.due
                    .as_ref()
                    .map(|s| format!(" · {s}"))
                    .unwrap_or_default(),
                item.evidence
                    .iter()
                    .filter_map(|id| m.segments.iter().find(|s| &s.id == id))
                    .map(|s| format!("{}:{:02}", s.start_ms / 60000, s.start_ms / 1000 % 60))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for content in parts(&text, 1800) {
                if kind == "todo" {
                    out.push(json!({"object":"block","type":"to_do","to_do":{"rich_text":rich(content),"checked":item.done}}));
                } else {
                    out.push(json!({"object":"block","type":"bulleted_list_item","bulleted_list_item":{"rich_text":rich(content)}}));
                }
            }
        }
    }
    if transcript {
        out.push(
            json!({"object":"block","type":"heading_2","heading_2":{"rich_text":rich("逐字稿")}}),
        );
        for seg in &m.segments {
            let text = format!(
                "[{}:{:02}] {} {}",
                seg.start_ms / 60000,
                seg.start_ms / 1000 % 60,
                seg.speaker.as_deref().unwrap_or(""),
                seg.text
            );
            for part in parts(&text, 1800) {
                out.push(json!({"object":"block","type":"paragraph","paragraph":{"rich_text":rich(part)}}));
            }
        }
    }
    out
}
async fn request(
    token: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value> {
    let client = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(store::io)?;
    let mut req = client
        .request(method, format!("https://api.notion.com/v1/{path}"))
        .bearer_auth(token)
        .header("Notion-Version", "2022-06-28")
        .header("Content-Type", "application/json");
    if let Some(body) = body {
        req = req.body(body.to_string());
    }
    let response = req.send().await.map_err(|_| {
        err(
            "NotionUncertain",
            "网络中断，发送结果待核实。请点击核实发送，避免重复创建。",
        )
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(err(
            if status.is_client_error() {
                "NotionRejected"
            } else {
                "NotionUncertain"
            },
            if status.as_u16() == 401 || status.as_u16() == 403 {
                "Notion 连接无权限，请检查密钥和页面共享设置。"
            } else {
                "Notion 未完成请求，请核实发送状态后重试。"
            },
        ));
    }
    let raw = response.bytes().await.map_err(store::io)?;
    if raw.len() > 4 * 1024 * 1024 {
        return Err(store::io("Notion response bound"));
    }
    serde_json::from_slice(&raw).map_err(store::io)
}
#[tauri::command]
pub fn meeting_notion_connection(
    window: WebviewWindow,
    state: State<'_, Managed>,
) -> Result<Connection> {
    crate::main_only(&window)?;
    config(&get(&state)?)
}
#[tauri::command]
pub fn meeting_notion_save(
    window: WebviewWindow,
    state: State<'_, Managed>,
    parent_id: String,
    token: String,
) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    let parent = Uuid::parse_str(parent_id.trim())
        .map_err(|_| AppError::invalid("请填写 Notion 父页面的 32 位页面 ID。"))?
        .to_string();
    if token.len() < 20 || token.len() > 512 || token.contains(char::is_whitespace) {
        return Err(AppError::invalid("请填写有效的 Notion 内部连接密钥。"));
    }
    key()?
        .set_password(&token)
        .map_err(|_| err("Keychain", "无法保存 Notion 连接密钥。"))?;
    write(
        &s,
        "notion.json",
        &Connection {
            parent_id: parent,
            connected: true,
        },
    )
}
#[tauri::command]
pub fn meeting_notion_disconnect(window: WebviewWindow, state: State<'_, Managed>) -> Result<()> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    let _ = key()?.delete_credential();
    write(&s, "notion.json", &Connection::default())
}
#[tauri::command]
pub fn meeting_notion_status(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
) -> Result<Value> {
    crate::main_only(&window)?;
    let e = journal(&get(&state)?, id)?;
    Ok(json!({"state":e.state,"url":e.url,"revision":e.revision}))
}
// Re-exporting one revision with other options creates another page, so each export gets
// its own nonce; journals from older versions keep their stored `SM-{id8}-{revision}` marker.
fn marker(id: Uuid, revision: u64) -> String {
    let nonce = Uuid::new_v4().simple().to_string();
    format!("SM-{}-{}-{}", &id.to_string()[..8], revision, &nonce[..8])
}
// Notion search matches titles that merely include the query, so `SM-x-3` also finds
// `SM-x-30` and `SM-x-3-nonce`; only the marker as a whole word identifies the page.
fn is_export_page(page: &Value, parent_id: &str, marker: &str) -> bool {
    let title: String = page["properties"]["title"]["title"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["plain_text"].as_str())
        .collect();
    page["parent"]["page_id"].as_str() == Some(parent_id)
        && title.split_whitespace().any(|word| word == marker)
}
#[tauri::command]
pub async fn meeting_notion_send(
    window: WebviewWindow,
    state: State<'_, Managed>,
    id: Uuid,
    transcript: bool,
    verify: bool,
) -> Result<Value> {
    crate::main_only(&window)?;
    let s = get(&state)?;
    let _guard = s.begin(Some(id), "exporting").await?;
    let connection = config(&s)?;
    if !connection.connected {
        return Err(err("NotionConnection", "请先连接 Notion。"));
    }
    let token = zeroize::Zeroizing::new(
        key()?
            .get_password()
            .map_err(|_| err("NotionConnection", "Notion 连接密钥不可用，请重新连接。"))?,
    );
    let m = store::read(&s.0.root, id)?;
    let mut job = journal(&s, id)?;
    if job.state == "complete"
        && job.revision == m.revision
        && job.transcript.is_none_or(|t| t == transcript)
    {
        return Ok(json!({"state":"complete","url":job.url}));
    }
    if job.state == "complete" {
        job = Export::default();
    }
    if job.state == "pending" && !verify {
        return Err(err("NotionUncertain", "上次发送结果待核实，请先核实发送。"));
    }
    let name = format!("notion-{id}.json");
    // Nothing has reached Notion yet, so a retry sends the current revision and options.
    if job.marker.is_empty() || (job.state == "prepared" && job.page_id.is_none()) {
        job = Export {
            state: "prepared".into(),
            revision: m.revision,
            marker: marker(id, m.revision),
            blocks: blocks(&m, transcript),
            transcript: Some(transcript),
            ..Default::default()
        };
        write(&s, &name, &job)?;
    }
    if verify && job.page_id.is_none() {
        let result=request(&token,reqwest::Method::POST,"search",Some(json!({"query":job.marker,"filter":{"value":"page","property":"object"},"page_size":100}))).await?;
        let matches: Vec<_> = result["results"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| is_export_page(p, &connection.parent_id, &job.marker))
            .collect();
        if matches.len() != 1 {
            return Err(err(
                "NotionUncertain",
                "尚未确认唯一的目标页面。请在 Notion 中检查，避免重复发送；可先复制纪要。",
            ));
        }
        job.page_id = matches[0]["id"].as_str().map(str::to_string);
        job.url = matches[0]["url"].as_str().map(str::to_string);
    }
    if let Some(page) = &job.page_id {
        if verify {
            let mut cursor = None::<String>;
            let mut count = 0;
            loop {
                let path = format!(
                    "blocks/{page}/children?page_size=100{}",
                    cursor
                        .as_ref()
                        .map(|c| format!("&start_cursor={c}"))
                        .unwrap_or_default()
                );
                let r = request(&token, reqwest::Method::GET, &path, None).await?;
                let received = r["results"]
                    .as_array()
                    .ok_or_else(|| err("NotionUncertain", "无法核实页面内容，请稍后重试。"))?;
                for actual in received {
                    if !job
                        .blocks
                        .get(count)
                        .is_some_and(|expected| same_exported_block(expected, actual))
                    {
                        return Err(err(
                            "NotionConflict",
                            "Notion 页面内容已被修改，请手动检查后再导出。",
                        ));
                    }
                    count += 1;
                }
                cursor = r["next_cursor"].as_str().map(str::to_string);
                if cursor.is_none() {
                    break;
                }
                if count > job.blocks.len() {
                    return Err(err(
                        "NotionConflict",
                        "Notion 页面已被修改，请在 Notion 中检查导出内容。",
                    ));
                }
            }
            // Only resume append at a previously issued batch boundary.
            if count < job.next || count > job.next + 80 || count > job.blocks.len() {
                return Err(err(
                    "NotionConflict",
                    "页面内容与导出记录不一致，请手动检查。",
                ));
            }
            job.next = count;
            write(&s, &name, &job)?;
        }
    } else {
        job.state = "pending".into();
        write(&s, &name, &job)?;
        let count = job.blocks.len().min(80);
        let body = json!({"parent":{"page_id":connection.parent_id},"properties":{"title":{"type":"title","title":rich(&format!("{} · {}",m.title,job.marker))}},"children":&job.blocks[..count]});
        let r = match request(&token, reqwest::Method::POST, "pages", Some(body)).await {
            Ok(r) => r,
            Err(e) => {
                if e.code == "NotionRejected" {
                    job.state = "prepared".into();
                    write(&s, &name, &job)?;
                }
                return Err(e);
            }
        };
        job.page_id = r["id"].as_str().map(str::to_string);
        job.url = r["url"].as_str().map(str::to_string);
        job.next = count;
        write(&s, &name, &job)?;
    }
    while job.next < job.blocks.len() {
        let end = (job.next + 80).min(job.blocks.len());
        job.state = "pending".into();
        write(&s, &name, &job)?;
        tokio::time::sleep(Duration::from_millis(400)).await;
        request(
            &token,
            reqwest::Method::PATCH,
            &format!(
                "blocks/{}/children",
                job.page_id.as_deref().ok_or_else(|| store::io("page id"))?
            ),
            Some(json!({"children":&job.blocks[job.next..end]})),
        )
        .await?;
        job.next = end;
        write(&s, &name, &job)?;
    }
    job.state = "complete".into();
    job.blocks.clear();
    write(&s, &name, &job)?;
    Ok(json!({"state":job.state,"url":job.url}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_markers_are_unique_and_verification_matches_them_exactly() {
        let id = Uuid::new_v4();
        let (first, second) = (marker(id, 3), marker(id, 3));
        assert_ne!(first, second);
        assert!(first.starts_with(&format!("SM-{}-3-", &id.to_string()[..8])));
        let parent = Uuid::new_v4().to_string();
        let page = |marker: &str| json!({"parent":{"page_id":parent},"properties":{"title":{"title":[{"plain_text":format!("周会 · {marker}")}]}}});
        assert!(is_export_page(&page(&first), &parent, &first));
        assert!(!is_export_page(&page(&second), &parent, &first));
        // Journals written before nonces keep matching their own page only.
        let legacy = format!("SM-{}-3", &id.to_string()[..8]);
        assert!(is_export_page(&page(&legacy), &parent, &legacy));
        assert!(!is_export_page(&page(&first), &parent, &legacy));
        assert!(!is_export_page(
            &page(&format!("{legacy}0")),
            &parent,
            &legacy
        ));
        assert!(!is_export_page(
            &page(&first),
            &Uuid::new_v4().to_string(),
            &first
        ));
    }
    #[test]
    fn verification_rejects_remote_edits_even_when_block_count_matches() {
        let expected =
            json!({"type":"to_do","to_do":{"rich_text":rich("检查预算"),"checked":false}});
        let mut actual = expected.clone();
        actual["id"] = json!("server-added-id");
        actual["archived"] = json!(false);
        assert!(same_exported_block(&expected, &actual));
        actual["to_do"]["rich_text"] = rich("其他内容");
        assert!(!same_exported_block(&expected, &actual));
        actual = expected.clone();
        actual["to_do"]["checked"] = json!(true);
        assert!(!same_exported_block(&expected, &actual));
    }
    #[test]
    fn notion_export_only_contains_selected_text_and_splits_rich_text() {
        let mut m = Meeting::new("test".into(), "microphone", "zh".into());
        m.segments.push(store::Segment {
            id: "s1".into(),
            start_ms: 0,
            end_ms: 1000,
            text: "private transcript".into(),
            speaker: None,
            speaker_id: None,
        });
        m.items.push(store::Item {
            kind: "todo".into(),
            text: "字".repeat(2000),
            owner: Some("李明".into()),
            due: None,
            evidence: vec!["s1".into()],
            done: false,
            ..Default::default()
        });
        let summary = blocks(&m, false);
        let raw = serde_json::to_string(&summary).unwrap();
        assert!(!raw.contains("private transcript"));
        assert!(!raw.contains(".pcm"));
        for block in &summary {
            let kind = block["type"].as_str().unwrap();
            for rich in block[kind]["rich_text"].as_array().unwrap() {
                assert!(rich["text"]["content"].as_str().unwrap().chars().count() <= 1900);
            }
        }
        assert!(serde_json::to_string(&blocks(&m, true))
            .unwrap()
            .contains("private transcript"));
    }
    #[test]
    fn emoji_text_is_split_by_notion_utf16_limit_without_losing_content() {
        let mut m = Meeting::new("test".into(), "microphone", "zh".into());
        m.segments.push(store::Segment {
            id: "s1".into(),
            start_ms: 0,
            end_ms: 1000,
            text: "😀".repeat(1500),
            speaker: None,
            speaker_id: None,
        });
        let mut joined = String::new();
        for block in blocks(&m, true).iter().skip(5) {
            let content = block["paragraph"]["rich_text"][0]["text"]["content"]
                .as_str()
                .unwrap();
            assert!(content.encode_utf16().count() <= 1800);
            joined.push_str(content);
        }
        assert_eq!(joined, format!("[0:00]  {}", "😀".repeat(1500)));
        assert_eq!(
            rich(&"😀".repeat(1500))[0]["text"]["content"]
                .as_str()
                .unwrap()
                .encode_utf16()
                .count(),
            1900
        );
    }
}
