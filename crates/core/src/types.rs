use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub revision: i64,
    pub language: String,
    pub ui_locale: String,
    pub theme: String,
    pub motion_policy: String,
    pub sidebar_collapsed: bool,
    pub delivery_mode: String,
    pub polish_enabled: bool,
    pub polish_mode: String,
    pub custom_prompt: String,
    pub processing_policy: String,
    pub history_enabled: bool,
    pub history_limit: u32,
    pub history_days: u32,
    pub input_device_id: Option<String>,
    pub active_model_id: Option<String>,
    pub active_provider_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsPatch {
    pub language: Option<String>,
    pub ui_locale: Option<String>,
    pub theme: Option<String>,
    pub motion_policy: Option<String>,
    pub sidebar_collapsed: Option<bool>,
    pub delivery_mode: Option<String>,
    pub polish_enabled: Option<bool>,
    pub polish_mode: Option<String>,
    pub custom_prompt: Option<String>,
    pub processing_policy: Option<String>,
    pub history_enabled: Option<bool>,
    pub history_limit: Option<u32>,
    pub history_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DictionaryEntry {
    pub entry_id: Uuid,
    pub term: String,
    pub locale: String,
    pub enabled: bool,
    pub pinned: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DictionaryInput {
    pub entry_id: Option<Uuid>,
    pub term: String,
    pub locale: String,
    pub enabled: bool,
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultRecord {
    pub session_id: Uuid,
    pub raw_text: String,
    pub polished_text: Option<String>,
    pub model_id: String,
    pub model_sha256: String,
    pub dictionary_revision: i64,
    pub language: String,
    pub duration_ms: u64,
    pub asr_ms: u64,
    pub needs_review: bool,
    pub polish_ms: Option<u64>,
    pub polish_outcome: String,
    #[serde(default)]
    pub polish_reason: Option<crate::processing::Reason>,
    pub delivery_status: String,
    #[serde(default)]
    pub delivery_reason: Option<crate::delivery::ManualReason>,
    pub app_identity: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRule {
    pub platform: String,
    pub app_identity: String,
    pub display_name: String,
    pub manual_only: bool,
    pub history_excluded: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub items: Vec<ResultRecord>,
    pub next_cursor: Option<String>,
}
