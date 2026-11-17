use super::*;
use crate::providers::{
    CredentialRef, ProviderDraft, ProviderRecord, MAX_PENDING_CREDENTIALS, MAX_PROVIDERS,
};

use super::provider_recovery::records;

fn check_revision(conn: &Connection, expected: i64) -> Result<()> {
    if Store::read_settings(conn)?.revision != expected {
        Err(AppError::conflict())
    } else {
        Ok(())
    }
}

fn enqueue(conn: &Connection, reference: &CredentialRef, purpose: &str, now: i64) -> Result<()> {
    let count: usize = conn.query_row(
        "SELECT count(*) FROM provider_credentials_pending",
        [],
        |r| r.get(0),
    )?;
    if count >= MAX_PENDING_CREDENTIALS {
        return Err(AppError::new(
            "CredentialCleanupRequired",
            "待清理凭据较多，请先完成清理再更换密钥。",
            true,
        ));
    }
    conn.execute("INSERT INTO provider_credentials_pending(credential_ref,provider_id,purpose,created_at) VALUES(?1,?2,?3,?4)", params![reference.account(), reference.provider_id().to_string(), purpose, now])?;
    Ok(())
}

fn bump(conn: &Connection, now: i64) -> Result<()> {
    conn.execute(
        "UPDATE settings SET revision=revision+1,updated_at=?1 WHERE id=1",
        [now],
    )?;
    Ok(())
}

impl Store {
    pub fn providers(&self) -> Result<Vec<ProviderRecord>> {
        records(&self.conn)
    }

    /// Durable intent is written before touching Keychain. A crash afterwards
    /// leaves either no key or a precisely identified orphan for cleanup.
    pub fn reserve_provider_credential(
        &mut self,
        expected: i64,
        provider_id: Uuid,
        now: i64,
    ) -> Result<CredentialRef> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        check_revision(&tx, expected)?;
        let reference = CredentialRef::new(provider_id)?;
        enqueue(&tx, &reference, "candidate", now)?;
        tx.commit()?;
        self.refresh_processing_policy();
        Ok(reference)
    }

    pub fn save_provider(
        &mut self,
        expected: i64,
        expected_provider: Option<i64>,
        next: ProviderDraft,
        now: i64,
    ) -> Result<ProviderRecord> {
        next.validate()?;
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        check_revision(&tx, expected)?;
        let all = records(&tx)?;
        let old = all.iter().find(|p| p.draft.provider_id == next.provider_id);
        if old.map(|p| p.revision) != expected_provider {
            return Err(AppError::conflict());
        }
        if old.is_none()
            && tx.query_row("SELECT count(*) FROM providers", [], |r| {
                r.get::<_, usize>(0)
            })? >= MAX_PROVIDERS
        {
            return Err(AppError::invalid("最多保存 20 个 AI 服务配置。"));
        }
        let old_reference = old.and_then(|p| p.draft.credential_ref.as_ref());
        if old_reference != next.credential_ref.as_ref() {
            if let Some(reference) = &next.credential_ref {
                let purpose: Option<String> = tx
                    .query_row(
                        "SELECT purpose FROM provider_credentials_pending WHERE credential_ref=?1",
                        [reference.account()],
                        |r| r.get(0),
                    )
                    .optional()?;
                if purpose.as_deref() != Some("candidate") {
                    return Err(AppError::invalid("新凭据尚未完成保存准备。"));
                }
                tx.execute(
                    "DELETE FROM provider_credentials_pending WHERE credential_ref=?1",
                    [reference.account()],
                )?;
            }
            if let Some(reference) = old_reference {
                enqueue(&tx, reference, "cleanup", now)?;
            }
        }
        if let Some(old) = old {
            let previous = url::Url::parse(&old.draft.endpoint)
                .map_err(|_| AppError::invalid("原 AI 服务地址无效。"))?;
            let target = url::Url::parse(&next.endpoint)
                .map_err(|_| AppError::invalid("AI 服务地址无效。"))?;
            if previous.origin() != target.origin()
                && old_reference.is_some()
                && old_reference == next.credential_ref.as_ref()
            {
                return Err(AppError::new(
                    "CredentialRebindRequired",
                    "服务地址已改变，请为新地址重新提供密钥。",
                    false,
                ));
            }
        }
        let revision = expected_provider
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(AppError::conflict)?;
        tx.execute("INSERT INTO providers(provider_id,revision,kind,endpoint,model_name,credential_ref,request_profile_json,consent_origin,tested_at,updated_at,display_name) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL,?9,?10) ON CONFLICT(provider_id) DO UPDATE SET revision=excluded.revision,kind=excluded.kind,endpoint=excluded.endpoint,model_name=excluded.model_name,credential_ref=excluded.credential_ref,request_profile_json=excluded.request_profile_json,consent_origin=excluded.consent_origin,tested_at=NULL,test_checkpoint=NULL,updated_at=excluded.updated_at,display_name=excluded.display_name",
            params![next.provider_id.to_string(),revision,next.kind,next.endpoint,next.model_name,next.credential_ref.as_ref().map(CredentialRef::account),next.request_profile_json,next.consent_origin,now,next.display_name])?;
        tx.execute(
            "UPDATE settings SET polish_enabled=0 WHERE active_provider_id=?1",
            [next.provider_id.to_string()],
        )?;
        bump(&tx, now)?;
        tx.commit()?;
        self.refresh_processing_policy();
        Ok(ProviderRecord {
            draft: next,
            revision,
            tested_at: None,
            test_checkpoint: None,
            updated_at: now,
        })
    }

    pub fn mark_provider_tested(
        &mut self,
        id: Uuid,
        revision: i64,
        checkpoint: &str,
        now: i64,
    ) -> Result<()> {
        if checkpoint.len() != 67
            || !checkpoint.starts_with("v1.")
            || !checkpoint[3..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(AppError::invalid("AI 服务测试记录无效。"));
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if tx.execute(
            "UPDATE providers SET tested_at=?1,test_checkpoint=?4 WHERE provider_id=?2 AND revision=?3",
            params![now, id.to_string(), revision, checkpoint],
        )? != 1
        {
            return Err(AppError::conflict());
        }
        bump(&tx, now)?;
        tx.commit()?;
        self.refresh_processing_policy();
        Ok(())
    }

    /// Re-testing revokes the previous checkpoint before reading credentials or
    /// sending anything. Failure/cancellation must not resurrect a previous pass.
    pub fn invalidate_provider_test(&mut self, id: Uuid, revision: i64, now: i64) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let actual: Option<i64> = tx
            .query_row(
                "SELECT revision FROM providers WHERE provider_id=?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if actual != Some(revision) {
            return Err(AppError::conflict());
        }
        let changed = tx.execute("UPDATE providers SET tested_at=NULL,test_checkpoint=NULL WHERE provider_id=?1 AND (tested_at IS NOT NULL OR test_checkpoint IS NOT NULL)", [id.to_string()])?;
        let disabled = tx.execute(
            "UPDATE settings SET polish_enabled=0 WHERE active_provider_id=?1 AND polish_enabled=1",
            [id.to_string()],
        )?;
        if changed != 0 || disabled != 0 {
            bump(&tx, now)?;
        }
        tx.commit()?;
        self.refresh_processing_policy();
        Ok(())
    }

    pub fn select_provider(&mut self, expected: i64, id: Option<Uuid>, now: i64) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        check_revision(&tx, expected)?;
        if let Some(id) = id {
            let ready: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM providers WHERE provider_id=?1 AND tested_at IS NOT NULL AND test_checkpoint IS NOT NULL)", [id.to_string()], |r| r.get(0))?;
            if !ready {
                return Err(AppError::new(
                    "ProviderNotTested",
                    "请先测试此 AI 服务的连接。",
                    false,
                ));
            }
        }
        tx.execute(
            "UPDATE settings SET active_provider_id=?1,polish_enabled=0 WHERE id=1",
            [id.map(|v| v.to_string())],
        )?;
        bump(&tx, now)?;
        tx.commit()?;
        self.refresh_processing_policy();
        Ok(())
    }

    pub fn delete_provider(&mut self, expected: i64, id: Uuid, now: i64) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        check_revision(&tx, expected)?;
        let old = records(&tx)?
            .into_iter()
            .find(|p| p.draft.provider_id == id)
            .ok_or_else(AppError::conflict)?;
        if let Some(reference) = &old.draft.credential_ref {
            enqueue(&tx, reference, "cleanup", now)?;
        }
        tx.execute(
            "UPDATE settings SET polish_enabled=0 WHERE active_provider_id=?1",
            [id.to_string()],
        )?;
        tx.execute(
            "DELETE FROM providers WHERE provider_id=?1",
            [id.to_string()],
        )?;
        bump(&tx, now)?;
        tx.commit()?;
        self.refresh_processing_policy();
        Ok(())
    }

    pub fn pending_provider_credentials(&self) -> Result<Vec<CredentialRef>> {
        super::provider_recovery::pending(&self.conn)
    }

    pub fn provider_credential_in_use(&self, reference: &CredentialRef) -> Result<bool> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM providers WHERE credential_ref=?1)",
                [reference.account()],
                |r| r.get(0),
            )
            .map_err(Into::into)
    }

    pub fn acknowledge_provider_cleanup(&mut self, reference: &CredentialRef) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let in_use: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM providers WHERE credential_ref=?1)",
            [reference.account()],
            |r| r.get(0),
        )?;
        if in_use {
            return Err(AppError::new(
                "CredentialInUse",
                "此凭据仍被 AI 服务使用，不能清理。",
                false,
            ));
        }
        tx.execute(
            "DELETE FROM provider_credentials_pending WHERE credential_ref=?1",
            [reference.account()],
        )?;
        tx.commit()?;
        self.refresh_processing_policy();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_test_revocation_is_atomic_and_survives_reopen_without_changing_usage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test-checkpoint.sqlite3");
        let id = Uuid::new_v4();
        let mut store = Store::open(&path).unwrap();
        let checkpoint = format!("v1.{}", "a".repeat(64));
        store.save_provider(1, None, draft(id, None), 1).unwrap();
        store.mark_provider_tested(id, 1, &checkpoint, 2).unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        let record = store.providers().unwrap().pop().unwrap();
        assert_eq!(record.test_checkpoint.as_deref(), Some(checkpoint.as_str()));
        assert_eq!(record.tested_at, Some(2));
        assert!(store.mark_provider_tested(id, 1, "malformed", 3).is_err());
        assert!(store.invalidate_provider_test(id, 2, 3).is_err());
        store
            .select_provider(store.settings().unwrap().revision, Some(id), 3)
            .unwrap();
        store.conn.execute_batch("UPDATE settings SET polish_enabled=1; CREATE TRIGGER reject_test_revoke BEFORE UPDATE OF test_checkpoint ON providers BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert!(store.invalidate_provider_test(id, 1, 4).is_err());
        assert!(store.settings().unwrap().polish_enabled);
        assert_eq!(
            store.providers().unwrap()[0].test_checkpoint.as_deref(),
            Some(checkpoint.as_str())
        );
        store
            .conn
            .execute_batch("DROP TRIGGER reject_test_revoke;")
            .unwrap();
        store.invalidate_provider_test(id, 1, 5).unwrap();
        let revision = store.settings().unwrap().revision;
        store.invalidate_provider_test(id, 1, 6).unwrap();
        assert_eq!(store.settings().unwrap().revision, revision);
        assert!(!store.settings().unwrap().polish_enabled);
        assert!(store.providers().unwrap()[0].test_checkpoint.is_none());
        assert!(store.providers().unwrap()[0].tested_at.is_none());
        assert!(store.select_provider(revision, Some(id), 7).is_err());
        assert_eq!(store.usage_today("2026-09-15").unwrap(), 0);
        drop(store);
        let store = Store::open(&path).unwrap();
        assert!(store.providers().unwrap()[0].test_checkpoint.is_none());
    }

    fn draft(id: Uuid, reference: Option<CredentialRef>) -> ProviderDraft {
        ProviderDraft {
            provider_id: id,
            display_name: "Work AI".into(),
            kind: "openai".into(),
            endpoint: "https://api.example.test/v1/chat/completions".into(),
            model_name: "model".into(),
            request_profile_json: "{}".into(),
            consent_origin: Some("https://api.example.test".into()),
            credential_ref: reference,
        }
    }

    #[test]
    fn candidate_rotation_and_selection_preserve_credential_journal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.sqlite3");
        let mut store = Store::open(&path).unwrap();
        let id = Uuid::new_v4();
        let revision = store.settings().unwrap().revision;
        let first = store.reserve_provider_credential(revision, id, 1).unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(
            store.pending_provider_credentials().unwrap(),
            vec![first.clone()]
        );
        assert!(store.providers().unwrap().is_empty());
        store
            .save_provider(revision, None, draft(id, Some(first.clone())), 2)
            .unwrap();
        assert!(store.pending_provider_credentials().unwrap().is_empty());
        assert!(store.provider_credential_in_use(&first).unwrap());
        assert!(store
            .select_provider(store.settings().unwrap().revision, Some(id), 3)
            .is_err());
        store
            .mark_provider_tested(id, 1, &format!("v1.{}", "a".repeat(64)), 4)
            .unwrap();
        store
            .select_provider(store.settings().unwrap().revision, Some(id), 5)
            .unwrap();
        let revision = store.settings().unwrap().revision;
        let second = store.reserve_provider_credential(revision, id, 6).unwrap();
        store
            .save_provider(revision, Some(1), draft(id, Some(second.clone())), 7)
            .unwrap();
        assert_eq!(
            store.pending_provider_credentials().unwrap(),
            vec![first.clone()]
        );
        assert!(!store.provider_credential_in_use(&first).unwrap());
        assert!(store.provider_credential_in_use(&second).unwrap());
        assert!(store.providers().unwrap()[0].tested_at.is_none());
        assert!(store.providers().unwrap()[0].test_checkpoint.is_none());
        assert!(!store.settings().unwrap().polish_enabled);
        assert_eq!(
            store.settings().unwrap().active_provider_id,
            Some(id.to_string())
        );
        store.acknowledge_provider_cleanup(&first).unwrap();
        assert!(store.pending_provider_credentials().unwrap().is_empty());
        assert_eq!(
            store
                .acknowledge_provider_cleanup(&second)
                .unwrap_err()
                .code,
            "CredentialInUse"
        );
    }

    #[test]
    fn failed_save_keeps_old_configuration_and_new_candidate_for_cleanup() {
        let mut store = Store::memory().unwrap();
        let id = Uuid::new_v4();
        let reference = store.reserve_provider_credential(1, id, 1).unwrap();
        store
            .save_provider(1, None, draft(id, Some(reference.clone())), 2)
            .unwrap();
        let revision = store.settings().unwrap().revision;
        let candidate = store.reserve_provider_credential(revision, id, 3).unwrap();
        store.conn.execute_batch("CREATE TRIGGER fixture_save_failure BEFORE UPDATE ON providers BEGIN SELECT RAISE(ABORT,'private fixture SQL detail'); END;").unwrap();
        let error = store
            .save_provider(revision, Some(1), draft(id, Some(candidate.clone())), 4)
            .err()
            .unwrap();
        assert_eq!(error.code, "StorageUnavailable");
        assert!(!error.user_message.contains("private fixture"));
        assert_eq!(store.settings().unwrap().revision, revision);
        assert_eq!(
            store.providers().unwrap()[0].draft.credential_ref,
            Some(reference)
        );
        assert_eq!(
            store.pending_provider_credentials().unwrap(),
            vec![candidate.clone()]
        );
        assert!(!store.provider_credential_in_use(&candidate).unwrap());
        store.acknowledge_provider_cleanup(&candidate).unwrap();
    }

    #[test]
    fn stale_revision_and_new_origin_cannot_rebind_existing_credential() {
        let mut store = Store::memory().unwrap();
        let id = Uuid::new_v4();
        let reference = store.reserve_provider_credential(1, id, 1).unwrap();
        store
            .save_provider(1, None, draft(id, Some(reference.clone())), 2)
            .unwrap();
        let before = store.settings().unwrap().revision;
        assert_eq!(
            store
                .save_provider(1, Some(1), draft(id, Some(reference.clone())), 3)
                .err()
                .unwrap()
                .code,
            "RevisionConflict"
        );
        let mut changed = draft(id, Some(reference.clone()));
        changed.endpoint = "https://new.example.test/chat".into();
        changed.consent_origin = Some("https://new.example.test".into());
        assert_eq!(
            store
                .save_provider(before, Some(1), changed, 4)
                .err()
                .unwrap()
                .code,
            "CredentialRebindRequired"
        );
        assert_eq!(store.settings().unwrap().revision, before);
        assert!(store.pending_provider_credentials().unwrap().is_empty());
        let unreserved = CredentialRef::new(id).unwrap();
        assert!(store
            .save_provider(before, Some(1), draft(id, Some(unreserved)), 5)
            .is_err());
        assert_eq!(
            store.providers().unwrap()[0].draft.credential_ref,
            Some(reference)
        );
    }

    #[test]
    fn deleting_provider_disables_selection_but_keeps_retryable_cleanup() {
        let mut store = Store::memory().unwrap();
        let id = Uuid::new_v4();
        let reference = store.reserve_provider_credential(1, id, 1).unwrap();
        store
            .save_provider(1, None, draft(id, Some(reference.clone())), 2)
            .unwrap();
        store
            .mark_provider_tested(id, 1, &format!("v1.{}", "a".repeat(64)), 3)
            .unwrap();
        store
            .select_provider(store.settings().unwrap().revision, Some(id), 4)
            .unwrap();
        store
            .conn
            .execute_batch("UPDATE settings SET polish_enabled=1")
            .unwrap();
        store
            .delete_provider(store.settings().unwrap().revision, id, 5)
            .unwrap();
        assert!(store.providers().unwrap().is_empty());
        assert!(!store.settings().unwrap().polish_enabled);
        assert!(store.settings().unwrap().active_provider_id.is_none());
        assert_eq!(
            store.pending_provider_credentials().unwrap(),
            vec![reference.clone()]
        );
        assert!(!store.provider_credential_in_use(&reference).unwrap());
        store.acknowledge_provider_cleanup(&reference).unwrap();
        assert!(store.pending_provider_credentials().unwrap().is_empty());
    }
}
