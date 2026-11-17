use super::*;
use crate::writing::{self, Rule};
impl Store {
    pub fn writing_rules(&self) -> Result<Vec<Rule>> {
        read(&self.conn)
    }
    pub fn upsert_writing_rule(&mut self, expected: i64, rule: Rule, now: i64) -> Result<()> {
        let rule = writing::validate(rule)?;
        let tx = self.conn.transaction()?;
        check(&tx, expected)?;
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM writing_rules WHERE id<>?1",
            [rule.id.to_string()],
            |r| r.get(0),
        )?;
        if count >= 100 {
            return Err(AppError::invalid("最多保存 100 条整理规则。"));
        }
        let duplicate: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM writing_rules WHERE kind=?1 AND target=?2 AND id<>?3)",
            params![rule.kind, rule.target, rule.id.to_string()],
            |r| r.get(0),
        )?;
        if duplicate {
            return Err(AppError::invalid(
                "这个应用或网站已有规则，请编辑已有规则。",
            ));
        }
        tx.execute("INSERT INTO writing_rules(id,kind,target,display_name,prompt,enabled) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET kind=excluded.kind,target=excluded.target,display_name=excluded.display_name,prompt=excluded.prompt,enabled=excluded.enabled",params![rule.id.to_string(),rule.kind,rule.target,rule.display_name,rule.prompt,rule.enabled])?;
        tx.execute(
            "UPDATE settings SET revision=revision+1,updated_at=?1 WHERE id=1",
            [now],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn delete_writing_rule(&mut self, expected: i64, id: Uuid, now: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        check(&tx, expected)?;
        if tx.execute("DELETE FROM writing_rules WHERE id=?1", [id.to_string()])? != 1 {
            return Err(AppError::new("NotFound", "规则已移除，请刷新。", true));
        }
        tx.execute(
            "UPDATE settings SET revision=revision+1,updated_at=?1 WHERE id=1",
            [now],
        )?;
        tx.commit()?;
        Ok(())
    }
}
fn check(tx: &Transaction<'_>, expected: i64) -> Result<()> {
    if tx.query_row("SELECT revision FROM settings WHERE id=1", [], |r| {
        r.get::<_, i64>(0)
    })? != expected
    {
        return Err(AppError::conflict());
    }
    Ok(())
}
pub(super) fn read(conn: &Connection) -> Result<Vec<Rule>> {
    let mut stmt=conn.prepare_cached("SELECT id,kind,target,display_name,prompt,enabled FROM writing_rules ORDER BY kind,display_name,target LIMIT 101")?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Rule {
                id: r.get::<_, String>(0)?.parse().map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
                kind: r.get(1)?,
                target: r.get(2)?,
                display_name: r.get(3)?,
                prompt: r.get(4)?,
                enabled: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if rows.len() > 100 {
        return Err(AppError::invalid("整理规则数量超出上限。"));
    }
    rows.into_iter().map(writing::validate).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rules_survive_reopen_and_failed_edits_leave_revision_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.db");
        let mut store = Store::open(&path).unwrap();
        let rule = Rule {
            id: Uuid::new_v4(),
            kind: "website".into(),
            target: "https://Example.com/mail?private=yes".into(),
            display_name: "Mail".into(),
            prompt: "Use short paragraphs".into(),
            enabled: true,
        };
        let rev = store.settings().unwrap().revision;
        store.upsert_writing_rule(rev, rule.clone(), 1).unwrap();
        assert!(store.upsert_writing_rule(rev, rule, 2).is_err());
        let rev = store.settings().unwrap().revision;
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.writing_rules().unwrap()[0].target, "example.com/mail");
        let mut duplicate = store.writing_rules().unwrap()[0].clone();
        duplicate.id = Uuid::new_v4();
        assert!(store.upsert_writing_rule(rev, duplicate, 3).is_err());
        assert_eq!(store.settings().unwrap().revision, rev);
        store
            .delete_writing_rule(rev, store.writing_rules().unwrap()[0].id, 4)
            .unwrap();
        assert!(store.writing_rules().unwrap().is_empty());
    }
}
