//! Transcribe committed audio while capture continues on its own thread.
use super::*;

pub(super) struct LiveNotes(Arc<AtomicBool>);
impl Drop for LiveNotes {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
struct LiveJob(Service, Uuid);
impl Drop for LiveJob {
    fn drop(&mut self) {
        self.0 .0.live_jobs.lock().unwrap().remove(&self.1);
        self.0.changed();
    }
}
pub(super) fn start(s: &Service, id: Uuid) -> LiveNotes {
    let cancel = Arc::new(AtomicBool::new(false));
    if !s.0.live_jobs.lock().unwrap().insert(id) {
        return LiveNotes(cancel);
    }
    let task_cancel = cancel.clone();
    let work = s.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _job = LiveJob(work.clone(), id);
        let result = (|| {
            let meeting = store::read(&work.0.root, id)?;
            let selected =
                meeting
                    .model_id
                    .clone()
                    .or(work.0.runtime.snapshot()?.settings.active_model_id);
            let Some(selected) = selected else {
                return Ok(());
            };
            if selected == "moss" {
                return moss::transcribe(&work, id, &task_cancel, true);
            }
            let lease = tauri::async_runtime::block_on(work.0.models.lease(&selected))
                .map_err(crate::model_error)?;
            let spec = crate::session_runtime::model_spec(&lease)?;
            work.edit(id, |m| {
                check_cancel(&task_cancel)?;
                m.model_id = Some(selected);
                Ok(())
            })?;
            work.transcribe(id, spec, &task_cancel, true)
        })();
        if let Err(e) = result {
            if !task_cancel.load(Ordering::Acquire) {
                // A preview failure never stops or discards recording.
                let _ = work.edit(id, |m| {
                    if work.is_recording(id) {
                        m.error = Some(format!(
                            "实时整理暂不可用：{} 录音继续，结束后可完整整理。",
                            e.user_message
                        ));
                    }
                    Ok(())
                });
            }
        }
    });
    LiveNotes(cancel)
}

pub(super) fn summarize(
    s: &Service,
    id: Uuid,
    since_samples: usize,
    cancel: &AtomicBool,
) -> Result<()> {
    check_cancel(cancel)?;
    let mut partial = store::read(&s.0.root, id)?;
    partial
        .segments
        .retain(|seg| seg.end_ms > since_samples as u64 / 16);
    partial.summary_provider_id = if partial.summary_follow_ai {
        summary::Preference::default().resolve(s)?
    } else {
        partial.summary_provider_id
    };
    if !partial.segments.is_empty()
        && (partial.summary_provider_id.is_some() || text::available(&s.0.text_root))
    {
        if let Ok(items) = text::background(|| summary::run(s, &partial, cancel)) {
            s.edit(id, |m| {
                check_cancel(cancel)?;
                m.items = merge_items(&m.items, items);
                m.summary_stale = true;
                Ok(())
            })?;
        }
    }
    Ok(())
}

/// Keeps the limits of store::validate_items so later edits of the meeting stay valid.
pub(super) fn merge_items(
    existing: &[store::Item],
    incoming: Vec<store::Item>,
) -> Vec<store::Item> {
    let mut items = existing.to_vec();
    for item in incoming {
        if let Some(previous) = items.iter_mut().find(|i| {
            i.kind == item.kind && i.text == item.text && i.owner == item.owner && i.due == item.due
        }) {
            if !previous.edited {
                previous.status = item.status;
                previous.support = item.support;
                previous.recipient = item.recipient;
            }
            for source in item.evidence {
                if previous.evidence.len() >= 20 {
                    break;
                }
                if !previous.evidence.contains(&source) {
                    previous.evidence.push(source);
                }
            }
        } else if items.len() < 200 {
            items.push(item);
        }
    }
    items
}
pub(super) fn preserve_completed(existing: &[store::Item], incoming: &mut [store::Item]) {
    for item in incoming {
        if existing.iter().any(|old| {
            old.done && old.kind == item.kind && old.text == item.text && old.owner == item.owner
        }) {
            item.done = true;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_partial_results_keep_previous_decisions_and_completed_tasks() {
        let old = store::Item {
            kind: "todo".into(),
            text: "更新报价".into(),
            owner: Some("张三".into()),
            due: None,
            evidence: vec!["s1".into()],
            done: true,
            ..Default::default()
        };
        let mut same = old.clone();
        same.done = false;
        same.evidence = vec!["s2".into()];
        let mut next = same.clone();
        next.text = "发送合同".into();
        let merged = merge_items(&[old.clone()], vec![same.clone(), next]);
        assert_eq!(merged.len(), 2);
        assert!(merged[0].done);
        assert_eq!(merged[0].evidence, ["s1", "s2"]);
        let mut final_items = vec![same];
        preserve_completed(&[old], &mut final_items);
        assert!(final_items[0].done);
    }
    #[test]
    fn repeated_partial_results_stay_within_saved_item_limits() {
        let segments: Vec<_> = (0..30)
            .map(|i| store::Segment {
                id: format!("s{i}"),
                start_ms: i * 1000,
                end_ms: i * 1000 + 1000,
                text: "确认预算".into(),
                speaker: None,
                speaker_id: None,
            })
            .collect();
        let item = |text: String, evidence: &str| store::Item {
            kind: "fact".into(),
            text,
            evidence: vec![evidence.into()],
            ..Default::default()
        };
        let mut items = vec![];
        for i in 0..30 {
            items = merge_items(&items, vec![item("确认预算".into(), &format!("s{i}"))]);
        }
        assert_eq!(items[0].evidence.len(), 20);
        let many: Vec<_> = (0..250).map(|i| item(format!("事项 {i}"), "s0")).collect();
        items = merge_items(&items, many);
        assert_eq!(items.len(), 200);
        assert!(store::validate_items(&items, &segments).is_ok());
    }
}
