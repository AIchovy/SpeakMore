use super::*;
use crate::processing::Reason;

#[test]
fn committed_privacy_changes_latch_and_failed_writes_do_not_revoke_live_attempts() {
    let mut store = Store::memory().unwrap();
    let patch = |enabled, cloud| SettingsPatch {
        polish_enabled: Some(enabled),
        processing_policy: Some(if cloud { "allow_cloud" } else { "local_only" }.into()),
        ..Default::default()
    };
    store.update_settings(1, patch(true, true), 1).unwrap();
    let policies = store.processing_policies();
    let remote = policies.watch(true);
    let local = policies.watch(false);
    assert!(!remote.revoked() && !local.revoked());
    assert!(store.update_settings(1, patch(false, false), 2).is_err());
    assert!(!remote.revoked() && !local.revoked());
    store.conn.execute_batch("CREATE TRIGGER reject_privacy BEFORE UPDATE OF processing_policy ON settings BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
    assert!(store.update_settings(2, patch(false, false), 2).is_err());
    assert!(!remote.revoked() && !local.revoked());
    store
        .conn
        .execute_batch("DROP TRIGGER reject_privacy;")
        .unwrap();
    store.update_settings(2, patch(true, false), 2).unwrap();
    assert!(remote.token().is_cancelled());
    assert!(!local.revoked());
    store.update_settings(3, patch(true, true), 3).unwrap();
    assert!(remote.revoked());
    let new_remote = policies.watch(true);
    assert!(!new_remote.revoked());
    store.update_settings(4, patch(false, true), 4).unwrap();
    assert!(new_remote.revoked() && local.revoked());
}

#[test]
fn polish_finishes_only_the_existing_raw_row_without_recreating_deleted_history_or_usage() {
    let mut store = Store::memory().unwrap();
    store
        .update_settings(
            1,
            SettingsPatch {
                history_enabled: Some(true),
                ..Default::default()
            },
            1,
        )
        .unwrap();
    for disposition in ["keep", "delete", "revoke", "excluded"] {
        let guard = store.history_guard();
        let mut raw = super::tests::result(300_000);
        store
            .persist_generated(&raw, "2023-11-14", true, false)
            .unwrap();
        raw.polished_text = Some("Private transcript.".into());
        raw.polish_outcome = "applied".into();
        raw.polish_ms = Some(18);
        match disposition {
            "delete" => {
                store.delete_history(&[raw.session_id]).unwrap();
            }
            "revoke" => {
                for enabled in [false, true] {
                    store
                        .update_settings(
                            store.settings().unwrap().revision,
                            SettingsPatch {
                                history_enabled: Some(enabled),
                                ..Default::default()
                            },
                            2,
                        )
                        .unwrap();
                }
            }
            _ => (),
        }
        let changed = store
            .transition_polish(&raw, guard, disposition == "excluded")
            .unwrap();
        assert_eq!(changed, disposition == "keep");
        assert!(store.usage_recorded(raw.session_id).unwrap());
        let saved = store.find_result(raw.session_id).unwrap();
        if disposition == "keep" {
            assert_eq!(saved.unwrap().polished_text, raw.polished_text);
            raw.polished_text = None;
            raw.polish_reason = Some(Reason::Deadline);
            raw.polish_outcome = "fallback".into();
            assert!(!store.transition_polish(&raw, guard, false).unwrap());
        } else if disposition == "delete" {
            assert!(saved.is_none());
        } else {
            assert!(saved.is_none_or(|row| row.polished_text.is_none()));
        }
    }
    assert_eq!(store.usage_today("2023-11-14").unwrap(), 4);
}
