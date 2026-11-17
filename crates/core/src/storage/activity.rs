//! Content-free usage metadata, independent of transcription history retention.
use super::*;
use serde::Serialize;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityDay {
    pub day: String,
    pub kind: String,
    pub count: u64,
    pub characters: u64,
    pub duration_ms: u64,
    pub unknown_characters: u64,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptionUsage {
    pub id: String,
    pub created_at: i64,
    pub duration_ms: u64,
    pub mode: String,
    pub translated: bool,
    pub saved: bool,
}
/// Daily totals keyed by (connection change count, local UTC offset).
pub(super) type DaysCache = std::cell::RefCell<Option<((u64, i64), Vec<ActivityDay>)>>;
pub struct ActivityOverview {
    pub days: Vec<ActivityDay>,
    pub dictations: Vec<ResultRecord>,
    pub captions: Vec<CaptionUsage>,
}
pub fn character_count(text: &str) -> u64 {
    text.chars().filter(|c| c.is_alphanumeric()).count() as u64
}
pub(super) fn record_dictation(tx: &Transaction<'_>, r: &ResultRecord) -> Result<()> {
    tx.prepare_cached(
        "INSERT INTO activity_events(id,kind,created_at,duration_ms,characters) VALUES(?1,'dictation',?2,?3,?4) ON CONFLICT(id) DO NOTHING",
    )?
    .execute(params![r.session_id.to_string(), r.created_at, r.duration_ms, character_count(&r.raw_text)])?;
    Ok(())
}
pub(super) fn backfill(tx: &Transaction<'_>) -> Result<()> {
    tx.execute("INSERT INTO activity_events(id,kind,created_at) SELECT session_id,'dictation',created_at FROM usage_events", [])?;
    let mut stmt =
        tx.prepare("SELECT session_id,raw_text,duration_ms,created_at FROM transcriptions")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, u64>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })?;
    let mut insert = tx.prepare("INSERT INTO activity_events(id,kind,created_at,duration_ms,characters) VALUES(?1,'dictation',?2,?3,?4) ON CONFLICT(id) DO UPDATE SET duration_ms=excluded.duration_ms,characters=excluded.characters")?;
    for row in rows {
        let (id, text, duration, at) = row?;
        insert.execute(params![id, at, duration, character_count(&text)])?;
    }
    Ok(())
}
impl Store {
    pub fn record_caption_usage(&mut self, usage: &CaptionUsage) -> Result<()> {
        if Uuid::parse_str(&usage.id).is_err()
            || usage.created_at <= 0
            || usage.duration_ms == 0
            || usage.duration_ms > i64::MAX as u64
            || !["listen", "speak", "mixed"].contains(&usage.mode.as_str())
        {
            return Err(AppError::invalid("字幕使用信息无效。"));
        }
        self.conn.execute("INSERT INTO activity_events(id,kind,created_at,duration_ms,mode,translated,saved) VALUES(?1,'captions',?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET duration_ms=MAX(activity_events.duration_ms,excluded.duration_ms),translated=excluded.translated,saved=excluded.saved WHERE activity_events.kind='captions'", params![usage.id, usage.created_at, usage.duration_ms, usage.mode, usage.translated, usage.saved])?;
        Ok(())
    }
    pub fn activity_overview(&self, after: i64, before: i64) -> Result<ActivityOverview> {
        let days = self.activity_days()?;
        let mut stmt = self.conn.prepare_cached("SELECT session_id,raw_text,polished_text,model_id,model_sha256,dictionary_revision,language,duration_ms,asr_ms,polish_ms,polish_outcome,delivery_status,app_identity,created_at,needs_review,delivery_reason,polish_reason FROM transcriptions WHERE created_at>=?1 AND created_at<?2 AND created_at>=?3-(SELECT history_days FROM settings WHERE id=1)*86400000 ORDER BY created_at DESC,session_id DESC LIMIT 5")?;
        let dictations = if self.settings()?.history_enabled {
            stmt.query_map(params![after, before, super::now_ms()], super::read_result)?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            vec![]
        };
        let mut stmt = self.conn.prepare_cached("SELECT id,created_at,duration_ms,mode,translated,saved FROM activity_events WHERE kind='captions' AND created_at>=?1 AND created_at<?2 ORDER BY created_at DESC,id DESC LIMIT 5")?;
        let captions = stmt
            .query_map(params![after, before], |r| {
                Ok(CaptionUsage {
                    id: r.get(0)?,
                    created_at: r.get(1)?,
                    duration_ms: r.get(2)?,
                    mode: r.get(3)?,
                    translated: r.get(4)?,
                    saved: r.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(ActivityOverview {
            days,
            dictations,
            captions,
        })
    }
    /// All-time daily totals scan every event, so reuse them until this
    /// connection writes anything or the local UTC offset changes.
    fn activity_days(&self) -> Result<Vec<ActivityDay>> {
        let offset: i64 = self
            .conn
            .prepare_cached("SELECT strftime('%s','now','localtime')-strftime('%s','now')")?
            .query_row([], |r| r.get(0))?;
        let key = (self.conn.total_changes(), offset);
        if let Some((cached, days)) = &*self.activity_days.borrow() {
            if *cached == key {
                return Ok(days.clone());
            }
        }
        let mut stmt = self.conn.prepare("SELECT date(created_at/1000.0,'unixepoch','localtime'),kind,count(*),COALESCE(sum(characters),0),COALESCE(sum(duration_ms),0),sum(CASE WHEN kind='dictation' AND characters IS NULL THEN 1 ELSE 0 END) FROM activity_events GROUP BY 1,2 ORDER BY 1")?;
        let days = stmt
            .query_map([], |r| {
                Ok(ActivityDay {
                    day: r.get(0)?,
                    kind: r.get(1)?,
                    count: r.get(2)?,
                    characters: r.get(3)?,
                    duration_ms: r.get(4)?,
                    unknown_characters: r.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        *self.activity_days.borrow_mut() = Some((key, days.clone()));
        Ok(days)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn statistics_survive_history_removal_and_count_raw_text_once() {
        let mut store = Store::memory().unwrap();
        let mut r = super::super::tests::result(3000);
        r.raw_text = "你好，AI 2026! 👋".into();
        r.polished_text = Some("更长的整理文字不会增加统计".into());
        store
            .persist_generated(&r, "2023-11-14", false, false)
            .unwrap();
        store
            .persist_generated(&r, "2023-11-14", false, false)
            .unwrap();
        store.clear_history().unwrap();
        store.cleanup(r.created_at + 80 * 86_400_000).unwrap();
        let overview = store.activity_overview(0, i64::MAX).unwrap();
        assert!(overview.dictations.is_empty());
        assert_eq!(overview.days.len(), 1);
        assert_eq!(overview.days[0].count, 1);
        assert_eq!(overview.days[0].characters, 8);
        assert_eq!(overview.days[0].duration_ms, 3000);
    }
    #[test]
    fn reused_daily_totals_follow_every_new_event() {
        let mut store = Store::memory().unwrap();
        let total = |store: &Store| {
            store
                .activity_overview(0, i64::MAX)
                .unwrap()
                .days
                .iter()
                .map(|d| d.count)
                .sum::<u64>()
        };
        let r = super::super::tests::result(1000);
        store
            .persist_generated(&r, "2023-11-14", false, false)
            .unwrap();
        assert_eq!(total(&store), 1);
        assert_eq!(total(&store), 1);
        store
            .persist_generated(&r, "2023-11-14", false, false)
            .unwrap();
        assert_eq!(total(&store), 1);
        store
            .persist_generated(
                &super::super::tests::result(1000),
                "2023-11-14",
                false,
                false,
            )
            .unwrap();
        assert_eq!(total(&store), 2);
        let usage = CaptionUsage {
            id: Uuid::new_v4().to_string(),
            created_at: r.created_at,
            duration_ms: 1000,
            mode: "listen".into(),
            translated: false,
            saved: false,
        };
        store.record_caption_usage(&usage).unwrap();
        assert_eq!(total(&store), 3);
    }
    #[test]
    fn caption_resumes_update_one_session_and_ignore_late_shorter_updates() {
        let mut store = Store::memory().unwrap();
        let mut usage = CaptionUsage {
            id: Uuid::new_v4().to_string(),
            created_at: 1_700_000_000_000,
            duration_ms: 1000,
            mode: "listen".into(),
            translated: false,
            saved: false,
        };
        store.record_caption_usage(&usage).unwrap();
        usage.duration_ms = 3000;
        store.record_caption_usage(&usage).unwrap();
        usage.duration_ms = 2000;
        store.record_caption_usage(&usage).unwrap();
        let stats = store.activity_overview(0, i64::MAX).unwrap();
        assert_eq!(stats.days[0].count, 1);
        assert_eq!(stats.days[0].duration_ms, 3000);
        assert_eq!(stats.captions[0].duration_ms, 3000);
        assert!(store
            .activity_overview(usage.created_at + 1, i64::MAX)
            .unwrap()
            .captions
            .is_empty());
        usage.duration_ms = 0;
        assert!(store.record_caption_usage(&usage).is_err());
    }
}
