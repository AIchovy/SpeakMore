use super::*;
use crate::app_rules::{self, Policies};
use std::sync::Arc;

impl Store {
    pub fn app_policies(&self) -> Arc<Policies> {
        self.app_policies.clone()
    }
    pub fn app_rules(&self) -> Result<Vec<AppRule>> {
        read(&self.conn)
    }
    pub fn upsert_app_rule(
        &mut self,
        expected_revision: i64,
        rule: AppRule,
        now: i64,
    ) -> Result<()> {
        app_rules::validate(&rule)?;
        let tx = self.conn.transaction()?;
        revision(&tx, expected_revision)?;
        let mut rules = read(&tx)?;
        let previous = rules
            .iter()
            .position(|r| r.platform == rule.platform && r.app_identity == rule.app_identity);
        if previous.is_none() && rules.len() >= app_rules::MAX_RULES {
            return Err(AppError::invalid(
                "最多保存 100 个应用规则，请先移除不再使用的规则。",
            ));
        }
        tx.execute("INSERT INTO app_rules(platform,app_identity,display_name,manual_only,history_excluded) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(platform,app_identity) DO UPDATE SET display_name=excluded.display_name,manual_only=excluded.manual_only,history_excluded=excluded.history_excluded", params![rule.platform,rule.app_identity,rule.display_name,rule.manual_only,rule.history_excluded])?;
        if let Some(i) = previous {
            rules[i] = rule;
        } else {
            rules.push(rule);
        }
        tx.execute(
            "UPDATE settings SET revision=revision+1,updated_at=?1 WHERE id=1",
            [now],
        )?;
        tx.commit()?;
        self.app_policies.replace(rules);
        Ok(())
    }
    pub fn delete_app_rule(
        &mut self,
        expected_revision: i64,
        identity: &str,
        now: i64,
    ) -> Result<()> {
        let identity = app_rules::macos_identity(identity)?;
        let tx = self.conn.transaction()?;
        revision(&tx, expected_revision)?;
        if tx.execute(
            "DELETE FROM app_rules WHERE platform='macos' AND app_identity=?1",
            [&identity],
        )? != 1
        {
            return Err(AppError::new(
                "NotFound",
                "该应用规则已被移除，请刷新列表。",
                true,
            ));
        }
        let rules = read(&tx)?;
        tx.execute(
            "UPDATE settings SET revision=revision+1,updated_at=?1 WHERE id=1",
            [now],
        )?;
        tx.commit()?;
        self.app_policies.replace(rules);
        Ok(())
    }
}
fn revision(tx: &Transaction<'_>, expected: i64) -> Result<()> {
    if tx.query_row("SELECT revision FROM settings WHERE id=1", [], |r| {
        r.get::<_, i64>(0)
    })? != expected
    {
        return Err(AppError::conflict());
    }
    Ok(())
}
pub(super) fn read(conn: &Connection) -> Result<Vec<AppRule>> {
    let mut stmt = conn.prepare("SELECT platform,app_identity,display_name,manual_only,history_excluded FROM app_rules ORDER BY display_name COLLATE NOCASE,app_identity LIMIT 101")?;
    let rules = stmt
        .query_map([], |r| {
            Ok(AppRule {
                platform: r.get(0)?,
                app_identity: r.get(1)?,
                display_name: r.get(2)?,
                manual_only: r.get(3)?,
                history_excluded: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rules.len() > app_rules::MAX_RULES {
        return Err(AppError::invalid("应用规则数量超出上限。"));
    }
    Ok(rules)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rule(id: &str, manual: bool, excluded: bool) -> AppRule {
        AppRule {
            platform: "macos".into(),
            app_identity: id.into(),
            display_name: "Private Notes".into(),
            manual_only: manual,
            history_excluded: excluded,
        }
    }
    fn save(store: &mut Store, rule: AppRule) {
        store
            .upsert_app_rule(store.settings().unwrap().revision, rule, 1700000000000)
            .unwrap();
    }
    #[test]
    fn restrictions_latch_per_source_across_transient_changes_deletion_and_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("rules.sqlite3");
        let mut store = Store::open(&path).unwrap();
        let a = store.app_policies().watch(Some("ORG.Test.Private"));
        let other = store.app_policies().watch(Some("org.test.public"));
        let unknown = store.app_policies().watch(None);
        save(&mut store, rule("org.test.private", true, true));
        save(&mut store, rule("org.test.private", false, false));
        store
            .delete_app_rule(
                store.settings().unwrap().revision,
                "ORG.TEST.PRIVATE",
                1700000000000,
            )
            .unwrap();
        assert!(a.manual_only() && a.history_excluded());
        assert!(unknown.manual_only() && unknown.history_excluded());
        assert!(!other.manual_only() && !other.history_excluded());
        assert!(!store
            .app_policies()
            .watch(Some("org.test.private"))
            .history_excluded());
        save(&mut store, rule("org.test.private", false, true));
        drop(store);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened.app_rules().unwrap()[0].display_name,
            "Private Notes"
        );
        assert!(reopened
            .app_policies()
            .watch(Some("org.test.private"))
            .history_excluded());
    }
    #[test]
    fn failed_writes_and_stale_revisions_do_not_publish_rules_or_revoke_consent() {
        let mut store = Store::memory().unwrap();
        save(&mut store, rule("org.test.private", false, false));
        let watch = store.app_policies().watch(Some("org.test.private"));
        let rev = store.settings().unwrap().revision;
        assert!(store
            .upsert_app_rule(rev - 1, rule("org.test.private", true, true), 1)
            .is_err());
        store.conn.execute_batch("CREATE TRIGGER deny_rule_update BEFORE UPDATE ON app_rules BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert!(store
            .upsert_app_rule(rev, rule("org.test.private", true, true), 1)
            .is_err());
        assert!(!watch.manual_only() && !watch.history_excluded());
        assert_eq!(store.settings().unwrap().revision, rev);
        assert!(!store.app_rules().unwrap()[0].history_excluded);
        assert!(store
            .upsert_app_rule(rev, rule("../private.app", true, true), 1)
            .is_err());
        assert_eq!(
            app_rules::macos_identity("com.Apple.TextEdit").unwrap(),
            "com.apple.textedit"
        );
    }
    #[test]
    fn migration_merges_case_variants_without_weakening_existing_restrictions() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(super::super::MIGRATION).unwrap();
        conn.execute_batch(include_str!("../../../../migrations/002_result_review.sql"))
            .unwrap();
        conn.execute_batch(include_str!(
            "../../../../migrations/003_delivery_reason.sql"
        ))
        .unwrap();
        conn.execute_batch("INSERT INTO app_rules VALUES('macos','org.Test.Private',1,0),('macos','org.test.private',0,1);").unwrap();
        let store = Store::from_connection(conn, None, "zh-CN").unwrap();
        let rules = store.app_rules().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].app_identity, "org.test.private");
        assert!(rules[0].manual_only && rules[0].history_excluded);
        assert_eq!(
            store
                .conn
                .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            10
        );
    }
    #[test]
    fn rule_limit_allows_updates_but_rejects_new_entries_without_changing_revision() {
        let mut store = Store::memory().unwrap();
        for n in 0..100 {
            save(&mut store, rule(&format!("org.test.app{n}"), false, false));
        }
        let rev = store.settings().unwrap().revision;
        assert!(store
            .upsert_app_rule(rev, rule("org.test.extra", true, true), 1)
            .is_err());
        assert_eq!(store.settings().unwrap().revision, rev);
        save(&mut store, rule("org.test.app0", true, true));
        assert_eq!(store.app_rules().unwrap().len(), 100);
    }
}
