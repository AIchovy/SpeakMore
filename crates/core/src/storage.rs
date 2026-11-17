use crate::{
    error::{AppError, Result},
    types::*,
    MAX_RECORDING_MS, MAX_TEXT_BYTES,
};
use rusqlite::{params, types::Value, Connection, OptionalExtension, Transaction};
use std::{collections::HashSet, path::Path};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

const MIGRATION: &str = include_str!("../../../migrations/001_initial.sql");
pub mod activity;
mod app_rules;
#[cfg(test)]
mod processing_tests;
mod provider_recovery;
mod providers;
mod writing;
pub use provider_recovery::{ProviderEntry, ProviderInventory, RecoveryKind, RecoveryTarget};
mod startup;
pub use startup::recovery_directory;
pub struct Store {
    conn: Connection,
    history_guard: Uuid,
    provider_recovery_guard: Uuid,
    app_policies: std::sync::Arc<crate::app_rules::Policies>,
    processing_policies: std::sync::Arc<crate::processing::Policies>,
    migration_backup_retained: bool,
    activity_days: activity::DaysCache,
}

/// One verified multi-file bundle. Paths stay in native code; the worker receives
/// per-artifact hashes from the trusted catalog, not this catalog-record digest.
pub struct ModelInstall {
    pub model_id: String,
    pub revision: String,
    pub bundle_sha256: String,
    pub size_bytes: u64,
    pub local_path: String,
    pub verified_at: i64,
}

impl Store {
    pub fn shortcuts(&self) -> Result<crate::shortcuts::Preferences> {
        Self::read_shortcuts(&self.conn)
    }
    fn read_shortcuts(conn: &Connection) -> Result<crate::shortcuts::Preferences> {
        let mut preferences = crate::shortcuts::Preferences::default();
        let mut stmt = conn.prepare("SELECT action,accelerator,enabled FROM shortcuts")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?;
        for row in rows {
            let (action, accelerator, enabled) = row?;
            let binding = crate::shortcuts::Binding {
                accelerator,
                enabled,
            };
            match action.as_str() {
                "push_to_talk" => preferences.push_to_talk = binding,
                // Ignore the retired cancel action when loading old profiles.
                "cancel" => {}
                _ => return Err(AppError::invalid("本地快捷键配置无效。")),
            }
        }
        preferences.validate()?;
        Ok(preferences)
    }
    /// Compare the complete prior preferences, independent of unrelated settings.
    /// The native adapter has already registered the candidate before calling this.
    pub fn update_shortcuts(
        &mut self,
        expected: &crate::shortcuts::Preferences,
        next: &crate::shortcuts::Preferences,
        now: i64,
    ) -> Result<()> {
        next.validate()?;
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if Self::read_shortcuts(&tx)? != *expected {
            return Err(AppError::conflict());
        }
        if expected == next {
            return Ok(());
        }
        tx.execute("DELETE FROM shortcuts", [])?;
        let binding = &next.push_to_talk;
        tx.execute(
            "INSERT INTO shortcuts(action,accelerator,enabled) VALUES('push_to_talk',?1,?2)",
            params![binding.accelerator, binding.enabled],
        )?;
        tx.execute(
            "UPDATE settings SET revision=revision+1,updated_at=?1 WHERE id=1",
            [now],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn register_model(&mut self, model: &ModelInstall) -> Result<()> {
        if model.model_id.is_empty()
            || model.model_id.len() > 120
            || !model
                .model_id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_".contains(&b))
            || model.bundle_sha256.len() != 64
            || !model
                .bundle_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || model.revision.len() != 40
            || !model
                .revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || model.size_bytes == 0
            || model.size_bytes > i64::MAX as u64
            || !Path::new(&model.local_path).is_absolute()
            || model.local_path.contains('\0')
            || model.verified_at <= 0
        {
            return Err(AppError::invalid("已安装模型记录无效。"));
        }
        self.conn.execute("INSERT INTO models(model_id,filename,revision,sha256,size_bytes,install_state,local_path,verified_at) VALUES(?1,?1,?2,?3,?4,'installed',?5,?6) ON CONFLICT(model_id) DO UPDATE SET filename=excluded.filename,revision=excluded.revision,sha256=excluded.sha256,size_bytes=excluded.size_bytes,install_state='installed',local_path=excluded.local_path,verified_at=excluded.verified_at",
            params![model.model_id,model.revision,model.bundle_sha256,model.size_bytes as i64,model.local_path,model.verified_at])?;
        Ok(())
    }
    /// Called only after the actor has observed ModelReady for this verified bundle.
    /// Compare the previously selected model while preserving unrelated settings edits.
    pub fn activate_model(
        &mut self,
        model_id: Option<&str>,
        bundle_sha256: Option<&str>,
        previous: Option<&str>,
        now: i64,
    ) -> Result<Settings> {
        let tx = self.conn.transaction()?;
        let selected: Option<String> =
            tx.query_row("SELECT active_model_id FROM settings WHERE id=1", [], |r| {
                r.get(0)
            })?;
        if selected.as_deref() != previous {
            return Err(AppError::conflict());
        }
        if let Some(model_id) = model_id {
            let available: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM models WHERE model_id=?1 AND sha256=?2 AND install_state='installed')", params![model_id,bundle_sha256], |r| r.get(0))?;
            if !available {
                return Err(AppError::new(
                    "ModelUnavailable",
                    "模型文件已改变，请重新校验。",
                    true,
                ));
            }
        }
        if selected.as_deref() != model_id {
            tx.execute("UPDATE settings SET active_model_id=?1,revision=revision+1,updated_at=?2 WHERE id=1",params![model_id,now])?;
        }
        tx.commit()?;
        self.settings()
    }
    pub fn usage_recorded(&self, session_id: Uuid) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM usage_events WHERE session_id=?1)",
            [session_id.to_string()],
            |r| r.get(0),
        )?)
    }
    pub fn forget_model(&mut self, model_id: &str, now: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("UPDATE settings SET active_model_id=NULL,revision=revision+1,updated_at=?2 WHERE id=1 AND active_model_id=?1",params![model_id,now])?;
        tx.execute("DELETE FROM models WHERE model_id=?1", params![model_id])?;
        tx.commit()?;
        Ok(())
    }
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_initial_locale(path, "zh-CN")
    }
    /// Apply a platform-selected UI language only while creating the first
    /// settings row. Existing preferences and upgraded databases are untouched.
    pub fn open_with_initial_locale(path: &Path, locale: &str) -> Result<Self> {
        if !["zh-CN", "en"].contains(&locale) {
            return Err(AppError::invalid("界面语言无效。"));
        }
        let path = startup::canonical_path(path)?;
        Self::from_connection(startup::open_connection(&path)?, Some(&path), locale)
    }
    pub fn memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?, None, "zh-CN")
    }
    pub fn migration_backup_retained(&self) -> bool {
        self.migration_backup_retained
    }
    fn from_connection(mut conn: Connection, path: Option<&Path>, locale: &str) -> Result<Self> {
        let migration_backup_retained = startup::prepare(&mut conn, path, locale)?;
        let store = Self {
            conn,
            history_guard: Uuid::new_v4(),
            provider_recovery_guard: Uuid::new_v4(),
            app_policies: Default::default(),
            processing_policies: Default::default(),
            migration_backup_retained,
            activity_days: Default::default(),
        };
        store.app_policies.replace(store.app_rules()?);
        store.refresh_processing_policy();
        Ok(store)
    }

    pub fn settings(&self) -> Result<Settings> {
        Self::read_settings(&self.conn)
    }
    pub fn processing_policies(&self) -> std::sync::Arc<crate::processing::Policies> {
        self.processing_policies.clone()
    }
    fn refresh_processing_policy(&self) {
        let (enabled, cloud) = self.settings().map_or((false, false), |s| {
            (s.polish_enabled, s.processing_policy == "allow_cloud")
        });
        self.processing_policies.replace(enabled, cloud);
    }
    fn read_settings(conn: &Connection) -> Result<Settings> {
        Ok(conn.prepare_cached("SELECT revision,language,ui_locale,theme,motion_policy,sidebar_collapsed,delivery_mode,polish_enabled,polish_mode,custom_prompt,processing_policy,history_enabled,history_limit,history_days,input_device_id,active_model_id,active_provider_id FROM settings WHERE id=1")?.query_row([],|r|Ok(Settings {
            revision:r.get(0)?,language:r.get(1)?,ui_locale:r.get(2)?,theme:r.get(3)?,motion_policy:r.get(4)?,sidebar_collapsed:r.get(5)?,delivery_mode:r.get(6)?,polish_enabled:r.get(7)?,polish_mode:r.get(8)?,custom_prompt:r.get(9)?,processing_policy:r.get(10)?,history_enabled:r.get(11)?,history_limit:r.get(12)?,history_days:r.get(13)?,input_device_id:r.get(14)?,active_model_id:r.get(15)?,active_provider_id:r.get(16)?
        }))?)
    }

    pub fn update_settings(
        &mut self,
        expected_revision: i64,
        patch: SettingsPatch,
        now: i64,
    ) -> Result<Settings> {
        let disables_history = patch.history_enabled == Some(false);
        let retention = patch.history_limit.is_some() || patch.history_days.is_some();
        let mut values: Vec<(&str, Value)> = Vec::new();
        for (name, value, allowed) in [
            ("language", patch.language, vec!["auto", "zh", "en"]),
            ("ui_locale", patch.ui_locale, vec!["zh-CN", "en"]),
            ("theme", patch.theme, vec!["system", "light", "dark"]),
            (
                "motion_policy",
                patch.motion_policy,
                vec!["system", "reduced"],
            ),
            (
                "delivery_mode",
                patch.delivery_mode,
                vec!["paste", "manual"],
            ),
            (
                "polish_mode",
                patch.polish_mode,
                vec!["standard", "email", "code_comment", "custom"],
            ),
            (
                "processing_policy",
                patch.processing_policy,
                vec!["local_only", "allow_cloud"],
            ),
        ] {
            if let Some(value) = value {
                if !allowed.contains(&value.as_str()) {
                    return Err(AppError::invalid("设置选项无效。"));
                }
                values.push((name, Value::Text(value)));
            }
        }
        for (name, value) in [
            ("sidebar_collapsed", patch.sidebar_collapsed),
            ("polish_enabled", patch.polish_enabled),
            ("history_enabled", patch.history_enabled),
        ] {
            if let Some(value) = value {
                values.push((name, Value::Integer(i64::from(value))));
            }
        }
        if let Some(value) = patch.custom_prompt {
            if value.len() > 4096 || value.contains('\0') {
                return Err(AppError::invalid("自定义要求最多 4 KiB，不能包含空字符。"));
            }
            values.push(("custom_prompt", Value::Text(value)));
        }
        if let Some(value) = patch.history_limit {
            if ![100, 500, 1000, 5000].contains(&value) {
                return Err(AppError::invalid("历史条数选项无效。"));
            }
            values.push(("history_limit", Value::Integer(value.into())));
        }
        if let Some(value) = patch.history_days {
            if ![1, 7, 30].contains(&value) {
                return Err(AppError::invalid("历史保留时间无效。"));
            }
            values.push(("history_days", Value::Integer(value.into())));
        }
        let tx = self.conn.transaction()?;
        check_revision(&tx, expected_revision)?;
        for (name, value) in values {
            tx.execute(&format!("UPDATE settings SET {name}=? WHERE id=1"), [value])?;
        }
        bump(&tx, now)?;
        // A shorter retention applies immediately, not at the next dictation or launch.
        let removed = if retention { prune(&tx, now)? } else { 0 };
        tx.commit()?;
        if removed > 0 {
            self.scrub();
        }
        if disables_history {
            self.history_guard = Uuid::new_v4();
        }
        self.refresh_processing_policy();
        self.settings()
    }

    pub fn dictionary(&self) -> Result<Vec<DictionaryEntry>> {
        let mut statement=self.conn.prepare_cached("SELECT entry_id,term,locale,enabled,pinned,created_at,updated_at FROM dictionary_entries ORDER BY enabled DESC,pinned DESC,term COLLATE NOCASE,entry_id")?;
        let rows = statement.query_map([], |r| {
            Ok(DictionaryEntry {
                entry_id: read_uuid(r, 0)?,
                term: r.get(1)?,
                locale: r.get(2)?,
                enabled: r.get(3)?,
                pinned: r.get(4)?,
                created_at: r.get(5)?,
                updated_at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The desktop boundary validates that a non-null ID is an available input
    /// device. Storage keeps the stable ID; null explicitly restores default.
    pub fn set_input_device(
        &mut self,
        expected_revision: i64,
        device_id: Option<String>,
        now: i64,
    ) -> Result<Settings> {
        if device_id
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 1024 || id.chars().any(char::is_control))
        {
            return Err(AppError::invalid("录音设备标识无效。"));
        }
        let tx = self.conn.transaction()?;
        check_revision(&tx, expected_revision)?;
        tx.execute(
            "UPDATE settings SET input_device_id=? WHERE id=1",
            [device_id],
        )?;
        bump(&tx, now)?;
        tx.commit()?;
        self.settings()
    }

    pub fn upsert_dictionary(
        &mut self,
        expected_revision: i64,
        input: DictionaryInput,
        now: i64,
    ) -> Result<DictionaryEntry> {
        let term: String = input.term.trim().nfc().collect();
        if term.is_empty()
            || term.chars().count() > 40
            || term.len() > 160
            || term.chars().any(char::is_control)
            || !["all", "zh", "en"].contains(&input.locale.as_str())
        {
            return Err(AppError::invalid(
                "词条需要 1–40 个字符，不能包含换行或控制字符。",
            ));
        }
        let key: String = term.case_fold().nfc().collect();
        let id = input.entry_id.unwrap_or_else(Uuid::new_v4);
        let tx = self.conn.transaction()?;
        check_revision(&tx, expected_revision)?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT created_at FROM dictionary_entries WHERE entry_id=?",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if input.entry_id.is_some() && existing.is_none() {
            return Err(AppError::new(
                "NotFound",
                "词条已被删除，请刷新列表。",
                true,
            ));
        }
        let count: i64 =
            tx.query_row("SELECT count(*) FROM dictionary_entries", [], |r| r.get(0))?;
        if existing.is_none() && count >= 200 {
            return Err(AppError::invalid("词典最多保存 200 个词条。"));
        }
        let duplicate:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM dictionary_entries WHERE normalized_key=?1 AND locale=?2 AND entry_id!=?3)",params![key,input.locale,id.to_string()],|r|r.get(0))?;
        if duplicate {
            return Err(AppError::new(
                "TermAlreadyExists",
                "该语言下已存在这个词条。",
                false,
            ));
        }
        let created = existing.unwrap_or(now);
        tx.execute("INSERT INTO dictionary_entries(entry_id,term,normalized_key,locale,enabled,pinned,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(entry_id) DO UPDATE SET term=excluded.term,normalized_key=excluded.normalized_key,locale=excluded.locale,enabled=excluded.enabled,pinned=excluded.pinned,updated_at=excluded.updated_at",params![id.to_string(),term,key,input.locale,input.enabled,input.pinned,created,now])?;
        bump(&tx, now)?;
        tx.commit()?;
        Ok(DictionaryEntry {
            entry_id: id,
            term,
            locale: input.locale,
            enabled: input.enabled,
            pinned: input.pinned,
            created_at: created,
            updated_at: now,
        })
    }

    pub fn delete_dictionary(&mut self, id: Uuid, expected_revision: i64, now: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        check_revision(&tx, expected_revision)?;
        if tx.execute(
            "DELETE FROM dictionary_entries WHERE entry_id=?",
            [id.to_string()],
        )? == 0
        {
            return Err(AppError::new("NotFound", "词条已被删除。", true));
        }
        bump(&tx, now)?;
        tx.commit()?;
        Ok(())
    }

    /// Changes whenever history is disabled, even if it is re-enabled before ASR ends.
    pub fn history_guard(&self) -> Uuid {
        self.history_guard
    }

    pub fn dictionary_snapshot(&self, language: &str) -> Result<(i64, Vec<String>)> {
        if !["auto", "zh", "en"].contains(&language) {
            return Err(AppError::invalid("识别语言无效。"));
        }
        let revision = self.settings()?.revision;
        let mut statement=self.conn.prepare_cached("SELECT term,normalized_key FROM dictionary_entries WHERE enabled=1 AND (?1='auto' OR locale='all' OR locale=?1) ORDER BY pinned DESC,updated_at DESC,entry_id ASC")?;
        let rows = statement.query_map([language], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut keys = HashSet::new();
        let mut terms = Vec::new();
        for row in rows {
            let (term, key) = row?;
            if keys.insert(key) {
                terms.push(term);
            }
        }
        Ok((revision, terms))
    }

    pub fn usage_today(&self, day: &str) -> Result<u32> {
        Ok(self
            .conn
            .prepare_cached("SELECT count(*) FROM usage_events WHERE quota_day=?")?
            .query_row([day], |r| r.get(0))?)
    }

    pub fn persist_generated(
        &mut self,
        record: &ResultRecord,
        quota_day: &str,
        history_at_start: bool,
        app_excluded: bool,
    ) -> Result<bool> {
        validate_record(record)?;
        let current = self.settings()?;
        let tx = self.conn.transaction()?;
        let fresh=tx.prepare_cached("INSERT INTO usage_events(session_id,quota_day,created_at) VALUES(?1,?2,?3) ON CONFLICT(session_id) DO NOTHING")?.execute(params![record.session_id.to_string(),quota_day,record.created_at])?==1;
        if fresh {
            activity::record_dictation(&tx, record)?;
        }
        let save = fresh && history_at_start && current.history_enabled && !app_excluded;
        if save {
            tx.prepare_cached("INSERT INTO transcriptions(session_id,raw_text,polished_text,model_id,model_sha256,dictionary_revision,language,duration_ms,asr_ms,polish_ms,polish_outcome,delivery_status,app_identity,created_at,needs_review,delivery_reason,polish_reason) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)")?.execute(params![record.session_id.to_string(),record.raw_text,record.polished_text,record.model_id,record.model_sha256,record.dictionary_revision,record.language,record.duration_ms,record.asr_ms,record.polish_ms,record.polish_outcome,record.delivery_status,record.app_identity,record.created_at,record.needs_review,record.delivery_reason.map(|r| r.as_str()),record.polish_reason.map(|r| r.as_str())])?;
        }
        tx.commit()?;
        // The result is already committed; reporting it unsaved would block delivery.
        // Retention is retried after the next dictation and at launch.
        if let Err(error) = self.retain(record.created_at) {
            eprintln!("SpeakMore history cleanup: {}", error.code);
        }
        Ok(save)
    }

    /// Complete refinement of an already saved raw result. This never inserts a
    /// row or usage event, and a revoked history consent cannot be restored.
    pub fn transition_polish(
        &mut self,
        record: &ResultRecord,
        guard: Uuid,
        excluded: bool,
    ) -> Result<bool> {
        validate_record(record)?;
        if !matches!(
            record.polish_outcome.as_str(),
            "applied" | "fallback" | "skipped"
        ) || (record.polish_outcome == "applied") != record.polished_text.is_some()
            || (record.polish_outcome == "applied") == record.polish_reason.is_some()
        {
            return Err(AppError::invalid("文字整理结果状态无效。"));
        }
        if guard != self.history_guard || excluded || !self.settings()?.history_enabled {
            return Ok(false);
        }
        Ok(self.conn.execute("UPDATE transcriptions SET polished_text=?1,polish_outcome=?2,polish_ms=?3,polish_reason=?4 WHERE session_id=?5 AND raw_text=?6 AND polish_outcome='off' AND polish_ms IS NULL AND delivery_status='ready_for_copy'",
            params![record.polished_text,record.polish_outcome,record.polish_ms,record.polish_reason.map(|r| r.as_str()),record.session_id.to_string(),record.raw_text])? == 1)
    }

    /// Update only an existing history row. Deleted/disabled history is never
    /// recreated, and delivery does not create or change a usage event.
    pub fn transition_delivery(
        &mut self,
        id: Uuid,
        expected: crate::delivery::Status,
        next: crate::delivery::Status,
        reason: Option<crate::delivery::ManualReason>,
    ) -> Result<bool> {
        use crate::delivery::Status as S;
        if !matches!(
            (expected, next),
            (S::ReadyForCopy, S::Uncertain | S::ReadyForCopy)
                | (
                    S::Uncertain,
                    S::ReadyForCopy | S::CopiedOnly | S::Submitted | S::Uncertain
                )
        ) {
            return Err(AppError::invalid("交付状态不能回退或重复提交。"));
        }
        let tx = self.conn.transaction()?;
        let old: Option<String> = tx
            .query_row(
                "SELECT delivery_status FROM transcriptions WHERE session_id=?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        if old.as_ref().is_some_and(|value| value != expected.as_str()) {
            return Err(AppError::conflict());
        }
        let changed = tx.execute("UPDATE transcriptions SET delivery_status=?1, delivery_reason=?2 WHERE session_id=?3 AND delivery_status=?4", params![next.as_str(), reason.map(|r| r.as_str()), id.to_string(), expected.as_str()])? == 1;
        tx.commit()?;
        Ok(changed)
    }

    pub fn history(&self, query: &str, cursor: Option<&str>) -> Result<HistoryPage> {
        if query.len() > 512 {
            return Err(AppError::invalid("搜索文字过长。"));
        }
        let (time, id) = if let Some(cursor) = cursor {
            let (time, id) = cursor
                .split_once(':')
                .ok_or_else(|| AppError::invalid("分页游标无效。"))?;
            (
                time.parse::<i64>()
                    .map_err(|_| AppError::invalid("分页游标无效。"))?,
                Uuid::parse_str(id)
                    .map_err(|_| AppError::invalid("分页游标无效。"))?
                    .to_string(),
            )
        } else {
            (i64::MAX, "~".into())
        };
        let query = format!(
            "%{}%",
            query
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        // Rows past the retention window stay hidden until cleanup removes them.
        let mut statement=self.conn.prepare_cached(r"SELECT session_id,raw_text,polished_text,model_id,model_sha256,dictionary_revision,language,duration_ms,asr_ms,polish_ms,polish_outcome,delivery_status,app_identity,created_at,needs_review,delivery_reason,polish_reason FROM transcriptions WHERE created_at>=?4-(SELECT history_days FROM settings WHERE id=1)*86400000 AND (created_at<?1 OR (created_at=?1 AND session_id<?2)) AND (raw_text LIKE ?3 ESCAPE '\' OR polished_text LIKE ?3 ESCAPE '\') ORDER BY created_at DESC,session_id DESC LIMIT 51")?;
        let mut items = statement
            .query_map(params![time, id, query, now_ms()], read_result)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let next_cursor = if items.len() > 50 {
            items.pop();
            items
                .last()
                .map(|r| format!("{}:{}", r.created_at, r.session_id))
        } else {
            None
        };
        Ok(HistoryPage { items, next_cursor })
    }

    pub fn delete_history(&mut self, ids: &[Uuid]) -> Result<usize> {
        if ids.is_empty() || ids.len() > 5000 {
            return Err(AppError::invalid("请选择需要删除的记录。"));
        }
        let tx = self.conn.transaction()?;
        let mut count = 0;
        {
            let mut statement = tx.prepare("DELETE FROM transcriptions WHERE session_id=?")?;
            for id in ids {
                count += statement.execute([id.to_string()])?;
            }
        }
        tx.commit()?;
        if count > 0 {
            self.scrub();
        }
        Ok(count)
    }

    /// One SQLite read statement supplies a consistent snapshot, including
    /// records beyond the currently loaded UI page. Only one row is buffered.
    pub fn export_history(
        &self,
        scope: &crate::history_export::Scope,
        format: crate::history_export::Format,
        writer: &mut impl std::io::Write,
    ) -> Result<usize> {
        use crate::history_export::{self, Scope};
        scope.validate()?;
        let (query, wanted) = match scope {
            Scope::Filtered { query } => (query.as_str(), None),
            Scope::Selected { ids } => ("", Some(ids.iter().copied().collect::<HashSet<_>>())),
        };
        let query = format!(
            "%{}%",
            query
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let mut statement = self.conn.prepare(r"SELECT session_id,raw_text,polished_text,model_id,model_sha256,dictionary_revision,language,duration_ms,asr_ms,polish_ms,polish_outcome,delivery_status,app_identity,created_at,needs_review,delivery_reason,polish_reason FROM transcriptions WHERE created_at>=?2-(SELECT history_days FROM settings WHERE id=1)*86400000 AND (raw_text LIKE ?1 ESCAPE '\' OR polished_text LIKE ?1 ESCAPE '\') ORDER BY created_at DESC,session_id DESC")?;
        let rows = statement.query_map(params![query, now_ms()], read_result)?;
        history_export::header(writer, format)?;
        let mut count = 0;
        for row in rows {
            let row = row?;
            if wanted
                .as_ref()
                .is_some_and(|ids| !ids.contains(&row.session_id))
            {
                continue;
            }
            if count >= 5000 {
                return Err(AppError::invalid("记录数量超过导出上限，请缩小范围。"));
            }
            validate_record(&row)?;
            history_export::record(writer, format, &row)?;
            count += 1;
        }
        if count == 0 || wanted.is_some_and(|ids| ids.len() != count) {
            return Err(AppError::new(
                "HistoryChanged",
                "所选记录已删除或没有匹配内容，请刷新后重新选择。",
                true,
            ));
        }
        Ok(count)
    }

    pub fn clear_history(&mut self) -> Result<usize> {
        let count = self.conn.execute("DELETE FROM transcriptions", [])?;
        self.scrub();
        Ok(count)
    }

    pub fn find_result(&self, id: Uuid) -> Result<Option<ResultRecord>> {
        Ok(self.conn.prepare_cached("SELECT session_id,raw_text,polished_text,model_id,model_sha256,dictionary_revision,language,duration_ms,asr_ms,polish_ms,polish_outcome,delivery_status,app_identity,created_at,needs_review,delivery_reason,polish_reason FROM transcriptions WHERE session_id=?")?.query_row([id.to_string()],read_result).optional()?)
    }

    pub fn cleanup(&mut self, now: i64) -> Result<()> {
        if self.retain(now)? > 0 {
            self.scrub();
        }
        Ok(())
    }
    fn retain(&mut self, now: i64) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let removed = prune(&tx, now)?;
        tx.prepare_cached("DELETE FROM usage_events WHERE created_at<?")?
            .execute([now.saturating_sub(40 * 86_400_000)])?;
        tx.commit()?;
        Ok(removed)
    }
    /// Best effort after history deletion: secure_delete has zeroed the freed
    /// pages in the WAL; copy them into the main file and drop older frames.
    fn scrub(&self) {
        let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().min(i64::MAX as u128) as i64)
}
/// Remove history beyond the configured age and count; returns removed rows.
fn prune(conn: &Connection, now: i64) -> Result<usize> {
    let settings = Store::read_settings(conn)?;
    let cutoff = now.saturating_sub(settings.history_days as i64 * 86_400_000);
    Ok(conn
        .prepare_cached("DELETE FROM transcriptions WHERE created_at<?")?
        .execute([cutoff])?
        + conn.prepare_cached("DELETE FROM transcriptions WHERE session_id IN (SELECT session_id FROM transcriptions ORDER BY created_at DESC,session_id DESC LIMIT -1 OFFSET ?)")?
            .execute([settings.history_limit])?)
}

fn check_revision(tx: &Transaction<'_>, expected: i64) -> Result<()> {
    let current: i64 =
        tx.query_row("SELECT revision FROM settings WHERE id=1", [], |r| r.get(0))?;
    if current != expected {
        Err(AppError::conflict())
    } else {
        Ok(())
    }
}
fn bump(tx: &Transaction<'_>, now: i64) -> Result<()> {
    tx.execute(
        "UPDATE settings SET revision=revision+1,updated_at=? WHERE id=1",
        [now],
    )?;
    Ok(())
}
fn read_uuid(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Uuid> {
    let value: String = row.get(index)?;
    Uuid::parse_str(&value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))
    })
}
fn read_result(r: &rusqlite::Row<'_>) -> rusqlite::Result<ResultRecord> {
    Ok(ResultRecord {
        session_id: read_uuid(r, 0)?,
        raw_text: r.get(1)?,
        polished_text: r.get(2)?,
        model_id: r.get(3)?,
        model_sha256: r.get(4)?,
        dictionary_revision: r.get(5)?,
        language: r.get(6)?,
        duration_ms: r.get(7)?,
        asr_ms: r.get(8)?,
        needs_review: r.get(14)?,
        polish_ms: r.get(9)?,
        polish_outcome: r.get(10)?,
        polish_reason: r
            .get::<_, Option<String>>(16)?
            .map(|value| {
                serde_json::from_value(serde_json::Value::String(value)).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        16,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })
            })
            .transpose()?,
        delivery_status: r.get(11)?,
        delivery_reason: r
            .get::<_, Option<String>>(15)?
            .map(|value| {
                serde_json::from_value(serde_json::Value::String(value)).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        15,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })
            })
            .transpose()?,
        app_identity: r.get(12)?,
        created_at: r.get(13)?,
    })
}
fn validate_record(record: &ResultRecord) -> Result<()> {
    if record.raw_text.is_empty()
        || record.raw_text.len() > MAX_TEXT_BYTES
        || record.raw_text.contains('\0')
        || record
            .polished_text
            .as_ref()
            .is_some_and(|s| s.is_empty() || s.len() > MAX_TEXT_BYTES || s.contains('\0'))
        || record.duration_ms > MAX_RECORDING_MS
        || record.model_sha256.len() != 64
        || !record.model_sha256.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(AppError::invalid("转写结果不符合保存约定。"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn legacy_license_and_usage_survive_reopen_without_changing_refinement_or_history() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("open-edition.sqlite3");
        let mut store = Store::open(&path).unwrap();
        store
            .update_settings(
                1,
                SettingsPatch {
                    history_enabled: Some(true),
                    polish_enabled: Some(true),
                    ..Default::default()
                },
                0,
            )
            .unwrap();
        let record = result(300_000);
        store
            .persist_generated(&record, "2023-11-14", true, false)
            .unwrap();
        store.conn.execute("INSERT INTO license(id,signed_token,imported_at) VALUES(1,CAST(x'feff' AS TEXT),1)", []).unwrap();
        let before = serde_json::to_value(store.settings().unwrap()).unwrap();
        drop(store);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            serde_json::to_value(reopened.settings().unwrap()).unwrap(),
            before
        );
        assert!(reopened.settings().unwrap().polish_enabled);
        assert_eq!(reopened.usage_today("2023-11-14").unwrap(), 1);
        assert_eq!(
            reopened
                .find_result(record.session_id)
                .unwrap()
                .unwrap()
                .raw_text,
            record.raw_text
        );
        let old: String = reopened
            .conn
            .query_row("SELECT hex(signed_token) FROM license", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(old, "FEFF");
    }

    #[test]
    fn delivery_intent_survives_reopen_and_receipts_never_recreate_history_or_charge_again() {
        use crate::delivery::Status as S;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("delivery.sqlite");
        let mut store = Store::open(&path).unwrap();
        store
            .update_settings(
                1,
                SettingsPatch {
                    history_enabled: Some(true),
                    ..Default::default()
                },
                0,
            )
            .unwrap();
        let record = result(300_000);
        store
            .persist_generated(&record, "2023-11-14", true, false)
            .unwrap();
        store
            .transition_delivery(record.session_id, S::ReadyForCopy, S::Uncertain, None)
            .unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .find_result(record.session_id)
                .unwrap()
                .unwrap()
                .delivery_status,
            "uncertain"
        );
        store
            .transition_delivery(record.session_id, S::Uncertain, S::Submitted, None)
            .unwrap();
        assert!(store
            .transition_delivery(record.session_id, S::Uncertain, S::ReadyForCopy, None)
            .is_err());
        assert!(store
            .transition_delivery(record.session_id, S::Submitted, S::Uncertain, None)
            .is_err());
        let saved = store.find_result(record.session_id).unwrap().unwrap();
        assert_eq!(saved.raw_text, record.raw_text);
        assert_eq!(saved.delivery_status, "submitted");
        assert_eq!(store.usage_today("2023-11-14").unwrap(), 1);
        store.delete_history(&[record.session_id]).unwrap();
        assert!(!store
            .transition_delivery(record.session_id, S::Uncertain, S::CopiedOnly, None)
            .unwrap());
        assert!(store.find_result(record.session_id).unwrap().is_none());
        assert_eq!(store.usage_today("2023-11-14").unwrap(), 1);
    }
    #[test]
    fn failed_delivery_intent_is_atomic_and_does_not_change_text_or_usage() {
        use crate::delivery::Status as S;
        let mut store = Store::memory().unwrap();
        store
            .update_settings(
                1,
                SettingsPatch {
                    history_enabled: Some(true),
                    ..Default::default()
                },
                0,
            )
            .unwrap();
        let record = result(300_000);
        store
            .persist_generated(&record, "2023-11-14", true, false)
            .unwrap();
        store.conn.execute_batch("CREATE TRIGGER fail_delivery BEFORE UPDATE OF delivery_status ON transcriptions BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;").unwrap();
        assert!(store
            .transition_delivery(record.session_id, S::ReadyForCopy, S::Uncertain, None)
            .is_err());
        let saved = store.find_result(record.session_id).unwrap().unwrap();
        assert_eq!(saved.delivery_status, "ready_for_copy");
        assert_eq!(saved.raw_text, record.raw_text);
        assert_eq!(store.usage_today("2023-11-14").unwrap(), 1);
    }
    #[test]
    fn export_snapshot_includes_unloaded_pages_and_respects_exact_scope() {
        use crate::history_export::{Format, Scope};
        let mut s = Store::memory().unwrap();
        s.update_settings(
            1,
            SettingsPatch {
                history_enabled: Some(true),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let mut ids = vec![];
        for i in 0..75 {
            let mut r = result(300_000);
            r.created_at += i;
            r.raw_text = format!("fixture {i}, literal_% and 中文\nnext line");
            ids.push(r.session_id);
            s.persist_generated(&r, "2023-11-14", true, false).unwrap();
        }
        assert_eq!(s.history("", None).unwrap().items.len(), 50);
        let mut output = vec![];
        assert_eq!(
            s.export_history(
                &Scope::Filtered {
                    query: "literal_%".into()
                },
                Format::Txt,
                &mut output
            )
            .unwrap(),
            75
        );
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("fixture 74, literal_% and 中文\nnext line"));
        assert!(text.contains("fixture 0, literal_% and 中文\nnext line"));
        assert!(text.find("fixture 74,").unwrap() < text.find("fixture 0,").unwrap());
        let scope = Scope::Selected {
            ids: vec![ids[0], ids[74], ids[0]],
        };
        assert_eq!(
            s.export_history(&scope, Format::Csv, &mut vec![]).unwrap(),
            2
        );
        s.delete_history(&[ids[0]]).unwrap();
        assert_eq!(
            s.export_history(&scope, Format::Txt, &mut vec![])
                .unwrap_err()
                .code,
            "HistoryChanged"
        );
        assert_eq!(s.usage_today("2023-11-14").unwrap(), 75);
    }
    #[test]
    fn legacy_cancel_is_not_loaded_and_cannot_block_the_single_dictation_binding() {
        use crate::shortcuts::{Action, Binding};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("shortcuts.sqlite");
        let mut store = super::Store::open(&path).unwrap();
        store
            .conn
            .execute(
                "INSERT INTO shortcuts(action,accelerator,enabled) VALUES('cancel','Fn',1)",
                [],
            )
            .unwrap();
        let previous = store.shortcuts().unwrap();
        assert!(!previous.cancel.enabled);
        let next = previous
            .replace(
                Action::PushToTalk,
                Binding {
                    accelerator: "Fn".into(),
                    enabled: true,
                },
            )
            .unwrap();
        store.update_shortcuts(&previous, &next, 1).unwrap();
        // Even the old default cancel combination may become the dictation key.
        let rebound = next
            .replace(
                Action::PushToTalk,
                Binding {
                    accelerator: next.cancel.accelerator.clone(),
                    enabled: true,
                },
            )
            .unwrap();
        store.update_shortcuts(&next, &rebound, 2).unwrap();
        drop(store);
        let store = super::Store::open(&path).unwrap();
        assert_eq!(store.shortcuts().unwrap(), rebound);
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM shortcuts", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn shortcut_save_is_atomic_persistent_and_preserves_concurrent_preferences() {
        use crate::shortcuts::{Action, Binding, Preferences};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("shortcuts.sqlite");
        let mut store = super::Store::open(&path).unwrap();
        let initial = store.shortcuts().unwrap();
        assert_eq!(initial, Preferences::default());
        let next = initial
            .replace(
                Action::PushToTalk,
                Binding {
                    accelerator: "Control+Alt+Space".into(),
                    enabled: true,
                },
            )
            .unwrap();
        store.update_shortcuts(&initial, &next, 1).unwrap();
        let revision = store.settings().unwrap().revision;
        store.update_shortcuts(&next, &next, 2).unwrap();
        assert_eq!(store.settings().unwrap().revision, revision);
        assert!(store
            .update_shortcuts(&initial, &Preferences::default(), 3)
            .is_err());
        store.conn.execute_batch("CREATE TRIGGER fail_shortcuts BEFORE INSERT ON shortcuts WHEN new.action='push_to_talk' BEGIN SELECT RAISE(ABORT, 'test'); END;").unwrap();
        assert!(store.update_shortcuts(&next, &initial, 4).is_err());
        assert_eq!(store.shortcuts().unwrap(), next);
        assert_eq!(store.settings().unwrap().revision, revision);
        drop(store);
        assert_eq!(
            super::Store::open(&path).unwrap().shortcuts().unwrap(),
            next
        );
    }
    use super::*;
    fn term(text: &str) -> DictionaryInput {
        DictionaryInput {
            entry_id: None,
            term: text.into(),
            locale: "all".into(),
            enabled: true,
            pinned: false,
        }
    }
    pub(super) fn result(duration: u64) -> ResultRecord {
        ResultRecord {
            needs_review: false,
            session_id: Uuid::new_v4(),
            raw_text: "private transcript".into(),
            polished_text: None,
            model_id: "test-model".into(),
            model_sha256: "a".repeat(64),
            dictionary_revision: 1,
            language: "en".into(),
            duration_ms: duration,
            asr_ms: 2,
            polish_ms: None,
            polish_outcome: "off".into(),
            polish_reason: None,
            delivery_status: "ready_for_copy".into(),
            delivery_reason: None,
            app_identity: None,
            created_at: now_ms() - 60_000,
        }
    }
    #[test]
    fn v2_upgrade_preserves_quality_and_stores_only_enumerated_delivery_reason() {
        use crate::{
            delivery::ManualReason,
            history_export::{Format, Scope},
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("delivery-upgrade.sqlite3");
        let old_id = Uuid::new_v4();
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch(MIGRATION).unwrap();
            old.execute_batch(include_str!("../../../migrations/002_result_review.sql"))
                .unwrap();
            old.execute("UPDATE settings SET history_enabled=1 WHERE id=1", [])
                .unwrap();
            old.execute("INSERT INTO transcriptions(session_id,raw_text,model_id,model_sha256,dictionary_revision,language,duration_ms,asr_ms,polish_outcome,delivery_status,created_at,needs_review) VALUES(?1,'old text','test-model',?2,1,'en',1000,100,'off','ready_for_copy',?3,0)", params![old_id.to_string(), "a".repeat(64), now_ms()]).unwrap();
        }
        let mut new = result(300_000);
        new.delivery_reason = Some(ManualReason::TargetChanged);
        {
            let mut store = Store::open(&path).unwrap();
            let old = store.find_result(old_id).unwrap().unwrap();
            assert!(!old.needs_review);
            assert_eq!(old.delivery_reason, None);
            store
                .persist_generated(&new, "2023-11-14", true, false)
                .unwrap();
            assert!(store.conn.execute("UPDATE transcriptions SET delivery_reason='unknown / arbitrary context' WHERE session_id=?1", [new.session_id.to_string()]).is_err());
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .find_result(new.session_id)
                .unwrap()
                .unwrap()
                .delivery_reason,
            Some(ManualReason::TargetChanged)
        );
        assert_eq!(store.usage_today("2023-11-14").unwrap(), 1);
        for format in [Format::Txt, Format::Csv] {
            let mut bytes = vec![];
            assert_eq!(
                store
                    .export_history(
                        &Scope::Filtered {
                            query: String::new()
                        },
                        format,
                        &mut bytes
                    )
                    .unwrap(),
                2
            );
            assert!(String::from_utf8(bytes).unwrap().contains("target_changed"));
        }
    }
    #[test]
    fn v1_upgrade_preserves_history_and_quality_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("upgrade.sqlite3");
        let old_id = Uuid::new_v4();
        {
            let old = Connection::open(&path).unwrap();
            old.execute_batch(MIGRATION).unwrap();
            old.execute("UPDATE settings SET history_enabled=1 WHERE id=1", [])
                .unwrap();
            old.execute("INSERT INTO transcriptions(session_id,raw_text,model_id,model_sha256,dictionary_revision,language,duration_ms,asr_ms,polish_outcome,delivery_status,created_at) VALUES(?1,'old text','whisper-small-q5_1',?2,1,'en',1000,100,'off','ready_for_copy',?3)", params![old_id.to_string(), "a".repeat(64), now_ms()]).unwrap();
        }
        let ids;
        {
            let mut store = Store::open(&path).unwrap();
            assert_eq!(
                store
                    .conn
                    .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                10
            );
            let old = store.find_result(old_id).unwrap().unwrap();
            assert_eq!(old.raw_text, "old text");
            assert_eq!(old.delivery_reason, None);
            assert!(old.needs_review);
            assert!(store.settings().unwrap().history_enabled);
            let mut checked = result(1000);
            checked.needs_review = false;
            let mut review = result(300000);
            review.needs_review = true;
            ids = (checked.session_id, review.session_id);
            for r in [checked, review] {
                store
                    .persist_generated(&r, "2023-11-14", true, false)
                    .unwrap();
            }
        }
        let store = Store::open(&path).unwrap();
        assert!(!store.find_result(ids.0).unwrap().unwrap().needs_review);
        assert!(store.find_result(ids.1).unwrap().unwrap().needs_review);
        assert_eq!(store.history("", None).unwrap().items.len(), 3);
    }
    #[test]
    fn defaults_and_real_reopen_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        {
            let mut s = Store::open(&path).unwrap();
            assert!(!s.settings().unwrap().history_enabled);
            s.upsert_dictionary(1, term("SpeakMore"), 10).unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.dictionary().unwrap()[0].term, "SpeakMore");
        assert_eq!(s.settings().unwrap().revision, 2);
    }
    #[test]
    fn dictionary_normalization_and_revision_conflicts_are_atomic() {
        let mut s = Store::memory().unwrap();
        s.upsert_dictionary(1, term(" Straße "), 1).unwrap();
        assert_eq!(
            s.upsert_dictionary(1, term("Other"), 2).unwrap_err().code,
            "RevisionConflict"
        );
        assert_eq!(
            s.upsert_dictionary(2, term("STRASSE"), 2).unwrap_err().code,
            "TermAlreadyExists"
        );
        assert_eq!(s.settings().unwrap().revision, 2);
        assert_eq!(s.dictionary().unwrap().len(), 1);
    }
    #[test]
    fn accepts_five_minutes_and_rejects_overrun_without_counting() {
        let mut s = Store::memory().unwrap();
        assert!(!s
            .persist_generated(&result(300_000), "2023-11-14", false, false)
            .unwrap());
        assert!(s
            .persist_generated(&result(300_001), "2023-11-14", false, false)
            .is_err());
        assert_eq!(s.usage_today("2023-11-14").unwrap(), 1);
    }
    #[test]
    fn history_off_and_mid_session_opt_out_leave_no_text() {
        let mut s = Store::memory().unwrap();
        s.update_settings(
            1,
            SettingsPatch {
                history_enabled: Some(true),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        s.update_settings(
            2,
            SettingsPatch {
                history_enabled: Some(false),
                ..Default::default()
            },
            1,
        )
        .unwrap();
        let r = result(5000);
        assert!(!s.persist_generated(&r, "2023-11-14", true, false).unwrap());
        assert!(s.history("", None).unwrap().items.is_empty());
        assert_eq!(s.usage_today("2023-11-14").unwrap(), 1);
        s.persist_generated(&r, "2023-11-14", true, false).unwrap();
        assert_eq!(s.usage_today("2023-11-14").unwrap(), 1);
    }
    #[test]
    fn deleting_history_preserves_quota_and_search_is_literal() {
        let mut s = Store::memory().unwrap();
        s.update_settings(
            1,
            SettingsPatch {
                history_enabled: Some(true),
                ..Default::default()
            },
            0,
        )
        .unwrap();
        let r = result(300_000);
        assert!(s.persist_generated(&r, "2023-11-14", true, false).unwrap());
        assert!(s.history("%", None).unwrap().items.is_empty());
        assert_eq!(s.history("private", None).unwrap().items.len(), 1);
        s.delete_history(&[r.session_id]).unwrap();
        assert_eq!(s.usage_today("2023-11-14").unwrap(), 1);
    }
    #[test]
    fn shorter_retention_applies_immediately_and_expired_rows_stay_hidden() {
        use crate::history_export::{Format, Scope};
        let mut s = Store::memory().unwrap();
        let on = SettingsPatch {
            history_enabled: Some(true),
            ..Default::default()
        };
        s.update_settings(1, on, 0).unwrap();
        let now = now_ms();
        let mut ids = vec![];
        for age in [0, 2, 10] {
            let mut r = result(1000);
            r.created_at = now - age * 86_400_000 - 60_000;
            ids.push(r.session_id);
            s.persist_generated(&r, "2023-11-14", true, false).unwrap();
        }
        assert_eq!(s.history("", None).unwrap().items.len(), 3);
        let revision = s.settings().unwrap().revision;
        let days = SettingsPatch {
            history_days: Some(7),
            ..Default::default()
        };
        s.update_settings(revision, days, now).unwrap();
        assert!(s.find_result(ids[2]).unwrap().is_none());
        assert_eq!(s.history("", None).unwrap().items.len(), 2);
        // Between cleanups, rows past the window are on disk but never shown or exported.
        s.conn
            .execute("UPDATE settings SET history_days=1", [])
            .unwrap();
        assert!(s.find_result(ids[1]).unwrap().is_some());
        assert_eq!(s.history("", None).unwrap().items.len(), 1);
        let all = Scope::Filtered {
            query: String::new(),
        };
        assert_eq!(s.export_history(&all, Format::Txt, &mut vec![]).unwrap(), 1);
        let expired = Scope::Selected { ids: vec![ids[1]] };
        assert_eq!(
            s.export_history(&expired, Format::Txt, &mut vec![])
                .unwrap_err()
                .code,
            "HistoryChanged"
        );
        assert_eq!(
            s.activity_overview(0, i64::MAX).unwrap().dictations.len(),
            1
        );
        for _ in 0..101 {
            s.persist_generated(&result(1000), "2023-11-14", true, false)
                .unwrap();
        }
        let revision = s.settings().unwrap().revision;
        let limit = SettingsPatch {
            history_limit: Some(100),
            ..Default::default()
        };
        s.update_settings(revision, limit, now).unwrap();
        let count: i64 = s
            .conn
            .query_row("SELECT count(*) FROM transcriptions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 100);
        assert_eq!(s.usage_today("2023-11-14").unwrap(), 104);
    }
    #[test]
    fn committed_result_is_reported_saved_when_retention_cleanup_fails() {
        let mut s = Store::memory().unwrap();
        let on = SettingsPatch {
            history_enabled: Some(true),
            ..Default::default()
        };
        s.update_settings(1, on, 0).unwrap();
        s.conn.execute_batch("INSERT INTO usage_events(session_id,quota_day,created_at) VALUES('expired','2020-01-01',0); CREATE TRIGGER fail_cleanup BEFORE DELETE ON usage_events BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        let r = result(1000);
        assert!(s.persist_generated(&r, "2023-11-14", true, false).unwrap());
        assert!(s.find_result(r.session_id).unwrap().is_some());
        assert_eq!(s.usage_today("2023-11-14").unwrap(), 1);
        assert!(s.cleanup(now_ms()).is_err());
    }
    #[test]
    fn deleted_history_text_does_not_remain_in_database_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scrub.sqlite3");
        let wal = dir.path().join("scrub.sqlite3-wal");
        let mut s = Store::open(&path).unwrap();
        let on = SettingsPatch {
            history_enabled: Some(true),
            ..Default::default()
        };
        s.update_settings(1, on, 0).unwrap();
        let mut ids = vec![];
        for i in 0..3 {
            let mut r = result(1000);
            r.raw_text = format!("scrub-marker-{i} {}", "x".repeat(3000));
            ids.push(r.session_id);
            s.persist_generated(&r, "2023-11-14", true, false).unwrap();
        }
        let on_disk = |i: usize| {
            let marker = format!("scrub-marker-{i}");
            [&path, &wal].iter().any(|file| {
                std::fs::read(file)
                    .is_ok_and(|bytes| bytes.windows(marker.len()).any(|w| w == marker.as_bytes()))
            })
        };
        assert!((0..3).all(on_disk));
        assert_eq!(s.delete_history(&ids[..1]).unwrap(), 1);
        assert!(!on_disk(0));
        assert!(on_disk(1) && on_disk(2));
        assert_eq!(s.clear_history().unwrap(), 2);
        assert!(!(0..3).any(on_disk));
    }
    #[test]
    fn dictionary_capacity_and_disabled_words() {
        let mut s = Store::memory().unwrap();
        for n in 0..200 {
            s.upsert_dictionary(n + 1, term(&format!("word{n}")), n)
                .unwrap();
        }
        assert!(s.upsert_dictionary(201, term("overflow"), 201).is_err());
        assert_eq!(s.dictionary().unwrap().len(), 200);
    }
}
#[test]
fn selected_device_survives_reopen_and_null_restores_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.sqlite3");
    let mut store = Store::open(&path).unwrap();
    store
        .set_input_device(1, Some("coreaudio:test-device".into()), 1)
        .unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store.settings().unwrap().input_device_id.as_deref(),
        Some("coreaudio:test-device")
    );
    assert_eq!(
        store.set_input_device(1, None, 2).unwrap_err().code,
        "RevisionConflict"
    );
    assert_eq!(
        store.settings().unwrap().input_device_id.as_deref(),
        Some("coreaudio:test-device")
    );
    assert!(store
        .set_input_device(2, Some("invalid\nidentifier".into()), 2)
        .is_err());
    let settings = store.set_input_device(2, None, 3).unwrap();
    assert!(settings.input_device_id.is_none());
    assert_eq!(settings.revision, 3);
}

#[test]
fn verified_bundle_registration_and_removal_survive_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("models.sqlite3");
    let mut store = Store::open(&path).unwrap();
    let mut model = ModelInstall {
        model_id: "whisper-small-q5_1".into(),
        revision: "1".repeat(40),
        bundle_sha256: "a".repeat(64),
        size_bytes: 190_970_585,
        local_path: directory
            .path()
            .join("bundles/model")
            .to_string_lossy()
            .into(),
        verified_at: 1,
    };
    store.register_model(&model).unwrap();
    assert!(store.settings().unwrap().active_model_id.is_none()); // install must not silently select
    store
        .conn
        .execute(
            "UPDATE settings SET active_model_id=?1 WHERE id=1",
            [&model.model_id],
        )
        .unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    let (state, size): (String, i64) = store
        .conn
        .query_row(
            "SELECT install_state,size_bytes FROM models WHERE model_id=?1",
            [&model.model_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "installed");
    assert_eq!(size, 190_970_585);
    model.bundle_sha256 = "invalid".into();
    assert!(store.register_model(&model).is_err());
    store.forget_model(&model.model_id, 2).unwrap();
    assert!(store.settings().unwrap().active_model_id.is_none());
    assert_eq!(store.settings().unwrap().revision, 2);
    store.forget_model(&model.model_id, 3).unwrap();
    assert_eq!(store.settings().unwrap().revision, 2);
}

#[test]
fn model_activation_preserves_settings_and_rejects_stale_identity() {
    let mut store = Store::memory().unwrap();
    let model = ModelInstall {
        model_id: "whisper-small-q5_1".into(),
        revision: "1".repeat(40),
        bundle_sha256: "a".repeat(64),
        size_bytes: 190_970_585,
        local_path: "/private/tmp/speakmore-test/bundle".into(),
        verified_at: 1,
    };
    store.register_model(&model).unwrap();
    store
        .update_settings(
            1,
            SettingsPatch {
                language: Some("zh".into()),
                ..Default::default()
            },
            2,
        )
        .unwrap();
    assert!(store
        .activate_model(Some(&model.model_id), Some(&"b".repeat(64)), None, 3)
        .is_err());
    let settings = store
        .activate_model(Some(&model.model_id), Some(&model.bundle_sha256), None, 4)
        .unwrap();
    assert_eq!(settings.language, "zh");
    assert_eq!(settings.revision, 3);
    assert_eq!(
        store.activate_model(None, None, None, 5).unwrap_err().code,
        "RevisionConflict"
    );
    assert_eq!(
        store.settings().unwrap().active_model_id.as_deref(),
        Some(model.model_id.as_str())
    );
    assert_eq!(
        store
            .activate_model(
                Some(&model.model_id),
                Some(&model.bundle_sha256),
                Some(&model.model_id),
                6
            )
            .unwrap()
            .revision,
        3
    );
    assert_eq!(
        store
            .activate_model(None, None, Some(&model.model_id), 7)
            .unwrap()
            .revision,
        4
    );
}
