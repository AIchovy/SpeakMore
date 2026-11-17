use super::*;
use speakmore_core::storage::activity::ActivityDay;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentMeeting {
    id: Uuid,
    title: String,
    created_at: i64,
    duration_ms: u64,
    state: String,
}
pub fn records(
    s: &Service,
    after: i64,
    before: i64,
) -> Result<(
    Vec<ActivityDay>,
    Vec<RecentMeeting>,
    std::collections::HashSet<String>,
)> {
    let records = s.summaries()?;
    let (days, recent) = summarize(&records, after, before);
    let archives = captions::archive_files(s, None)?
        .iter()
        .filter_map(|path| path.file_name()?.to_str()?.get(..36).map(str::to_string))
        .collect();
    Ok((days, recent, archives))
}
fn summarize(
    records: &[Meeting],
    after: i64,
    before: i64,
) -> (Vec<ActivityDay>, Vec<RecentMeeting>) {
    let mut days = std::collections::BTreeMap::<String, ActivityDay>::new();
    let mut recent = vec![];
    for m in records.iter().filter(|m| m.duration_ms > 0) {
        let Some(at) = chrono::DateTime::from_timestamp_millis(m.created_at) else {
            continue;
        };
        let day = at
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d")
            .to_string();
        let stat = days.entry(day.clone()).or_insert_with(|| ActivityDay {
            day,
            kind: "meeting".into(),
            ..Default::default()
        });
        stat.count += 1;
        stat.duration_ms = stat.duration_ms.saturating_add(m.duration_ms);
        if m.created_at >= after && m.created_at < before {
            recent.push(RecentMeeting {
                id: m.id,
                title: m.title.clone(),
                created_at: m.created_at,
                duration_ms: m.duration_ms,
                state: m.state.clone(),
            });
        }
    }
    recent.sort_by_key(|m| (std::cmp::Reverse(m.created_at), m.id));
    recent.truncate(5);
    (days.into_values().collect(), recent)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_recorded_meetings_count_and_resumes_are_one_meeting() {
        let mut recorded = Meeting::new("三小时评审".into(), "microphone", "zh".into());
        recorded.duration_ms = 3 * 60 * 60 * 1000;
        let failed = Meeting::new("没有权限".into(), "microphone", "zh".into());
        let (days, recent) = summarize(&[failed, recorded.clone()], 0, i64::MAX);
        assert_eq!(days.iter().map(|d| d.count).sum::<u64>(), 1);
        assert_eq!(days[0].duration_ms, recorded.duration_ms);
        assert_eq!(recent[0].id, recorded.id);
        assert!(
            summarize(&[recorded.clone()], recorded.created_at + 1, i64::MAX)
                .1
                .is_empty()
        );
    }
}
