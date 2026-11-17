use crate::*;
use chrono::{Local, NaiveDate, TimeZone};
use serde::Serialize;
use speakmore_core::storage::activity::{ActivityDay, CaptionUsage};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityOverview {
    days: Vec<ActivityDay>,
    dictations: Vec<ResultRecord>,
    meetings: Vec<meetings::activity::RecentMeeting>,
    captions: Vec<CaptionUsage>,
}
fn bounds(day: Option<&str>) -> Result<(i64, i64)> {
    bounds_in(&Local, day)
}
fn bounds_in<Tz: TimeZone>(zone: &Tz, day: Option<&str>) -> Result<(i64, i64)> {
    let invalid = || AppError::invalid("请选择有效的使用日期。");
    let Some(day) = day else {
        return Ok((0, i64::MAX));
    };
    let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").map_err(|_| invalid())?;
    if date.format("%Y-%m-%d").to_string() != day {
        return Err(invalid());
    }
    let start = day_start(zone, date).ok_or_else(invalid)?;
    let end = day_start(zone, date.succ_opt().ok_or_else(invalid)?).ok_or_else(invalid)?;
    Ok((start, end))
}
/// A DST change may skip local midnight; the day then starts at its first valid minute.
fn day_start<Tz: TimeZone>(zone: &Tz, date: NaiveDate) -> Option<i64> {
    let midnight = date.and_hms_opt(0, 0, 0)?;
    (0..=26 * 60).find_map(|minute| {
        let local = midnight.checked_add_signed(chrono::TimeDelta::minutes(minute))?;
        zone.from_local_datetime(&local)
            .earliest()
            .map(|time| time.timestamp_millis())
    })
}
#[tauri::command]
pub async fn overview_activity(
    window: WebviewWindow,
    backend: State<'_, Backend>,
    meeting_state: State<'_, meetings::Managed>,
    day: Option<String>,
) -> Result<ActivityOverview> {
    main_only(&window)?;
    let (after, before) = bounds(day.as_deref())?;
    let usage = with_store(&backend, move |store| {
        store.activity_overview(after, before)
    })
    .await?;
    let service = meeting_state.0.clone()?;
    let (meeting_days, meetings, archives) = tauri::async_runtime::spawn_blocking(move || {
        meetings::activity::records(&service, after, before)
    })
    .await
    .map_err(|_| AppError::new("StorageUnavailable", "使用记录暂时无法读取，请重试。", true))??;
    let mut days = usage.days;
    days.extend(meeting_days);
    let mut captions = usage.captions;
    for caption in &mut captions {
        caption.saved &= archives.contains(&caption.id);
    }
    Ok(ActivityOverview {
        days,
        dictations: usage.dictations,
        meetings,
        captions,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn date_filters_are_canonical_and_use_calendar_boundaries() {
        assert!(bounds(Some("2026-02-30")).is_err());
        assert!(bounds(Some("2026-1-5")).is_err());
        let (start, end) = bounds(Some("2026-10-05")).unwrap();
        assert_eq!(
            Local
                .timestamp_millis_opt(start)
                .unwrap()
                .format("%Y-%m-%d")
                .to_string(),
            "2026-10-05"
        );
        assert_eq!(
            Local
                .timestamp_millis_opt(end)
                .unwrap()
                .format("%Y-%m-%d")
                .to_string(),
            "2026-10-06"
        );
        assert!(end > start);
    }
    /// UTC-4 until 2026-09-06 04:00Z, then UTC-3: local 00:00–01:00 never occurs.
    #[derive(Clone)]
    struct SkipsMidnight;
    impl TimeZone for SkipsMidnight {
        type Offset = chrono::FixedOffset;
        fn from_offset(_: &Self::Offset) -> Self {
            Self
        }
        fn offset_from_local_date(&self, local: &NaiveDate) -> chrono::LocalResult<Self::Offset> {
            self.offset_from_local_datetime(&local.and_hms_opt(12, 0, 0).unwrap())
        }
        fn offset_from_local_datetime(
            &self,
            local: &chrono::NaiveDateTime,
        ) -> chrono::LocalResult<Self::Offset> {
            let gap = NaiveDate::from_ymd_opt(2026, 9, 6)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap();
            match *local {
                l if l < gap => {
                    chrono::LocalResult::Single(chrono::FixedOffset::west_opt(4 * 3600).unwrap())
                }
                l if l < gap + chrono::TimeDelta::hours(1) => chrono::LocalResult::None,
                _ => chrono::LocalResult::Single(chrono::FixedOffset::west_opt(3 * 3600).unwrap()),
            }
        }
        fn offset_from_utc_date(&self, utc: &NaiveDate) -> Self::Offset {
            self.offset_from_utc_datetime(&utc.and_hms_opt(0, 0, 0).unwrap())
        }
        fn offset_from_utc_datetime(&self, utc: &chrono::NaiveDateTime) -> Self::Offset {
            let switch = NaiveDate::from_ymd_opt(2026, 9, 6)
                .unwrap()
                .and_hms_opt(4, 0, 0)
                .unwrap();
            chrono::FixedOffset::west_opt(if *utc < switch { 4 } else { 3 } * 3600).unwrap()
        }
    }
    #[test]
    fn skipped_midnight_starts_the_day_at_its_first_valid_time() {
        let (start, end) = bounds_in(&SkipsMidnight, Some("2026-09-06")).unwrap();
        let switch = NaiveDate::from_ymd_opt(2026, 9, 6)
            .unwrap()
            .and_hms_opt(4, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();
        assert_eq!(start, switch);
        assert_eq!(end - start, 23 * 3_600_000);
        let (previous, previous_end) = bounds_in(&SkipsMidnight, Some("2026-09-05")).unwrap();
        assert_eq!(previous_end, start);
        assert_eq!(previous_end - previous, 24 * 3_600_000);
    }
}
