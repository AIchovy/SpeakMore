//! Bounded, per-row reads and review-bound recovery. Raw invalid metadata and
//! system credential references never cross the native boundary.
use super::*;
use crate::providers::{CredentialRef, ProviderDraft, ProviderRecord, MAX_PENDING_CREDENTIALS};
use rusqlite::types::ValueRef;

const PAGE: usize = 100;
const FIELDS: &[(&str, usize)] = &[
    ("provider_id", 36),
    ("revision", 0),
    ("kind", 32),
    ("endpoint", 2048),
    ("model_name", 256),
    ("credential_ref", 75),
    ("request_profile_json", 4096),
    ("consent_origin", 2048),
    ("tested_at", 0),
    ("updated_at", 0),
    ("display_name", 160),
    ("test_checkpoint", 67),
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RecoveryKind {
    Provider,
    Credential,
}

/// No deserializer: a frontend cannot supply table names, rowids or epochs.
#[derive(Clone, PartialEq, Eq)]
pub struct RecoveryTarget {
    kind: RecoveryKind,
    rowid: i64,
    guard: Uuid,
    epoch: Epoch,
}
impl RecoveryTarget {
    pub fn kind(&self) -> RecoveryKind {
        self.kind
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Epoch {
    local: u64,
    external: i64,
}
fn epoch(conn: &Connection) -> Result<Epoch> {
    Ok(Epoch {
        local: conn.total_changes(),
        external: conn.query_row("PRAGMA main.data_version", [], |r| r.get(0))?,
    })
}
pub struct ProviderEntry {
    pub target: RecoveryTarget,
    pub name: Option<String>,
    pub record: Option<ProviderRecord>,
}
pub struct ProviderInventory {
    pub entries: Vec<ProviderEntry>,
    pub invalid_credentials: Vec<RecoveryTarget>,
    pub cleanup_pending: usize,
    pub total_providers: u64,
    pub truncated: bool,
}

// Bounds are enforced in SQL before bytes are copied into Rust. A wrong type or
// oversized cell becomes NULL and invalidates that row, never its neighbours.
fn projection() -> String {
    FIELDS.iter().map(|(field, max)| {
        if *max == 0 {
            format!("CASE WHEN typeof({field})='integer' THEN {field} END")
        } else {
            format!("CASE WHEN typeof({field})='text' AND length(CAST({field} AS BLOB))<={max} THEN {field} END")
        }
    }).collect::<Vec<_>>().join(",")
}
fn text(row: &rusqlite::Row<'_>, index: usize) -> Option<String> {
    match row.get_ref(index).ok()? {
        ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok().map(str::to_owned),
        _ => None,
    }
}
fn integer(row: &rusqlite::Row<'_>, index: usize) -> Option<i64> {
    match row.get_ref(index).ok()? {
        ValueRef::Integer(value) => Some(value),
        _ => None,
    }
}
fn safe_name(name: Option<String>) -> Option<String> {
    name.filter(|v| !v.trim().is_empty() && v.trim() == v && !v.chars().any(char::is_control))
}
fn decode(row: &rusqlite::Row<'_>) -> Option<ProviderRecord> {
    // Index 0 is rowid. Null flags at 13/14 distinguish absent optional values
    // from malformed non-null cells replaced by our bounded projection.
    let id = text(row, 1)?;
    let provider_id = Uuid::parse_str(&id).ok()?;
    if provider_id.is_nil() || provider_id.to_string() != id {
        return None;
    }
    let reference = text(row, 6);
    if reference.is_none() && row.get::<_, bool>(13).ok()? {
        return None;
    }
    let consent = text(row, 8);
    if consent.is_none() && row.get::<_, bool>(14).ok()? {
        return None;
    }
    let revision = integer(row, 2)?;
    if revision <= 0 {
        return None;
    }
    let draft = ProviderDraft {
        provider_id,
        display_name: text(row, 11)?,
        kind: text(row, 3)?,
        endpoint: text(row, 4)?,
        model_name: text(row, 5)?,
        request_profile_json: text(row, 7)?,
        consent_origin: consent,
        credential_ref: reference
            .as_deref()
            .map(CredentialRef::parse)
            .transpose()
            .ok()?,
    };
    draft.validate().ok()?;
    Some(ProviderRecord {
        draft,
        revision,
        tested_at: integer(row, 9),
        updated_at: integer(row, 10)?,
        test_checkpoint: text(row, 12),
    })
}
struct DecodedEntry {
    rowid: i64,
    name: Option<String>,
    record: Option<ProviderRecord>,
}
fn read(conn: &Connection) -> Result<Vec<DecodedEntry>> {
    let mut statement = conn.prepare_cached(&format!("SELECT rowid,{},credential_ref IS NOT NULL,consent_origin IS NOT NULL FROM providers ORDER BY rowid LIMIT {PAGE}", projection()))?;
    let mut rows = statement.query([])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        result.push(DecodedEntry {
            rowid: row.get(0)?,
            name: safe_name(text(row, 11)),
            record: decode(row),
        });
    }
    Ok(result)
}
pub(super) fn records(conn: &Connection) -> Result<Vec<ProviderRecord>> {
    Ok(read(conn)?
        .into_iter()
        .filter_map(|entry| entry.record)
        .collect())
}

fn credentials(conn: &Connection) -> Result<Vec<(i64, Option<CredentialRef>)>> {
    let mut stmt=conn.prepare(&format!("SELECT rowid,CASE WHEN typeof(credential_ref)='text' AND length(CAST(credential_ref AS BLOB))=75 THEN credential_ref END,CASE WHEN typeof(provider_id)='text' AND length(CAST(provider_id AS BLOB))=36 THEN provider_id END FROM provider_credentials_pending ORDER BY rowid LIMIT {PAGE}"))?;
    let mut rows = stmt.query([])?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let reference = text(row, 1)
            .and_then(|account| CredentialRef::parse(&account).ok())
            .filter(|reference| Some(reference.provider_id().to_string()) == text(row, 2));
        result.push((row.get(0)?, reference));
    }
    Ok(result)
}
pub(super) fn pending(conn: &Connection) -> Result<Vec<CredentialRef>> {
    Ok(credentials(conn)?
        .into_iter()
        .filter_map(|(_, reference)| reference)
        .collect())
}

impl Store {
    pub fn provider_inventory(&self) -> Result<ProviderInventory> {
        if !self.conn.is_autocommit() {
            return Err(AppError::busy());
        }
        let before = epoch(&self.conn)?;
        let entries = read(&self.conn)?
            .into_iter()
            .map(
                |DecodedEntry {
                     rowid,
                     name,
                     record,
                 }| ProviderEntry {
                    target: RecoveryTarget {
                        kind: RecoveryKind::Provider,
                        rowid,
                        guard: self.provider_recovery_guard,
                        epoch: before,
                    },
                    name,
                    record,
                },
            )
            .collect::<Vec<_>>();
        let pending = credentials(&self.conn)?;
        let total_providers = self
            .conn
            .query_row("SELECT count(*) FROM providers", [], |row| {
                row.get::<_, u64>(0)
            })?;
        let total_pending = self.conn.query_row(
            "SELECT count(*) FROM provider_credentials_pending",
            [],
            |row| row.get::<_, u64>(0),
        )?;
        if epoch(&self.conn)? != before {
            return Err(AppError::conflict());
        }
        Ok(ProviderInventory {
            truncated: total_providers > entries.len() as u64
                || total_pending > pending.len() as u64,
            total_providers,
            entries,
            cleanup_pending: pending.iter().filter(|(_, r)| r.is_some()).count(),
            invalid_credentials: pending
                .into_iter()
                .filter(|(_, r)| r.is_none())
                .map(|(rowid, _)| RecoveryTarget {
                    kind: RecoveryKind::Credential,
                    rowid,
                    guard: self.provider_recovery_guard,
                    epoch: before,
                })
                .collect(),
        })
    }

    /// Called only with a native recovery target that was shown for review.
    /// IMMEDIATE excludes external writers before the connection-local epoch
    /// is compared; even unrelated writes invalidate the old review.
    pub fn discard_provider_entry(
        &mut self,
        expected: i64,
        target: RecoveryTarget,
        now: i64,
    ) -> Result<()> {
        if target.guard != self.provider_recovery_guard {
            return Err(AppError::conflict());
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if epoch(&tx)? != target.epoch || Self::read_settings(&tx)?.revision != expected {
            return Err(AppError::conflict());
        }
        match target.kind {
            RecoveryKind::Provider => {
                let count: usize = tx.query_row(
                    "SELECT count(*) FROM provider_credentials_pending",
                    [],
                    |r| r.get(0),
                )?;
                let has_key: bool = tx
                    .query_row(
                        "SELECT credential_ref IS NOT NULL FROM providers WHERE rowid=?1",
                        [target.rowid],
                        |r| r.get(0),
                    )
                    .optional()?
                    .ok_or_else(AppError::conflict)?;
                if has_key && count >= MAX_PENDING_CREDENTIALS {
                    return Err(AppError::new(
                        "CredentialCleanupRequired",
                        "请先处理待清理的凭据记录。",
                        true,
                    ));
                }
                // Copy metadata in SQL, preserving wrong types/bytes too. Only
                // canonical matching references are ever sent to the vault.
                tx.execute("INSERT OR IGNORE INTO provider_credentials_pending(credential_ref,provider_id,purpose,created_at) SELECT credential_ref,COALESCE(provider_id,''),'cleanup',?2 FROM providers WHERE rowid=?1 AND credential_ref IS NOT NULL",params![target.rowid,now])?;
                tx.execute("UPDATE settings SET polish_enabled=0,active_provider_id=NULL WHERE active_provider_id IS (SELECT provider_id FROM providers WHERE rowid=?1)",[target.rowid])?;
                if tx.execute("DELETE FROM providers WHERE rowid=?1", [target.rowid])? != 1 {
                    return Err(AppError::conflict());
                }
            }
            RecoveryKind::Credential => {
                if !credentials(&tx)?
                    .iter()
                    .any(|(id, reference)| *id == target.rowid && reference.is_none())
                {
                    return Err(AppError::conflict());
                }
                if tx.execute(
                    "DELETE FROM provider_credentials_pending WHERE rowid=?1",
                    [target.rowid],
                )? != 1
                {
                    return Err(AppError::conflict());
                }
            }
        }
        tx.execute(
            "UPDATE settings SET revision=revision+1,updated_at=?1 WHERE id=1",
            [now],
        )?;
        tx.commit()?;
        self.refresh_processing_policy();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert(store: &Store, id: &str, reference: Option<&str>) {
        store.conn.execute("INSERT INTO providers(provider_id,kind,endpoint,model_name,request_profile_json,display_name,credential_ref,updated_at) VALUES(?1,'ollama','http://127.0.0.1:11434/api/chat','fixture','{\"kind\":\"ollama\"}','Local fixture',?2,1)",params![id,reference]).unwrap();
    }
    fn invalid(store: &Store) -> RecoveryTarget {
        store
            .provider_inventory()
            .unwrap()
            .entries
            .into_iter()
            .find(|e| e.record.is_none())
            .unwrap()
            .target
    }
    fn revision(store: &Store) -> i64 {
        store.settings().unwrap().revision
    }

    #[test]
    fn malformed_rows_are_isolated_and_recovery_preserves_unknown_key_debt() {
        let mut store = Store::memory().unwrap();
        let healthy = Uuid::new_v4().to_string();
        insert(&store, &healthy, None);
        insert(&store, "broken-id", Some("unrecognized-reference"));
        let huge = Uuid::new_v4().to_string();
        insert(&store, &huge, None);
        store
            .conn
            .execute(
                "UPDATE providers SET model_name=?1 WHERE provider_id=?2",
                params!["x".repeat(200_000), huge],
            )
            .unwrap();
        let utf8 = Uuid::new_v4().to_string();
        insert(&store, &utf8, None);
        store
            .conn
            .execute(
                "UPDATE providers SET display_name=CAST(x'ff' AS TEXT) WHERE provider_id=?1",
                [utf8],
            )
            .unwrap();
        assert_eq!(store.providers().unwrap().len(), 1);
        let inventory = store.provider_inventory().unwrap();
        assert_eq!(inventory.total_providers, 4);
        assert_eq!(
            inventory
                .entries
                .iter()
                .filter(|e| e.record.is_none())
                .count(),
            3
        );
        assert_eq!(inventory.entries[3].name, None);
        store.conn.execute("UPDATE settings SET active_provider_id='broken-id',polish_enabled=1,custom_prompt='keep fixture'",[]).unwrap();
        let target = invalid(&store);
        store
            .discard_provider_entry(revision(&store), target, 2)
            .unwrap();
        assert!(store.settings().unwrap().active_provider_id.is_none());
        assert!(!store.settings().unwrap().polish_enabled);
        assert_eq!(store.settings().unwrap().custom_prompt, "keep fixture");
        assert_eq!(
            store.providers().unwrap()[0].draft.provider_id.to_string(),
            healthy
        );
        assert!(store.pending_provider_credentials().unwrap().is_empty());
        let inventory = store.provider_inventory().unwrap();
        assert_eq!(inventory.invalid_credentials.len(), 1);
        let debt = inventory.invalid_credentials.into_iter().next().unwrap();
        store
            .discard_provider_entry(revision(&store), debt, 3)
            .unwrap();
        assert!(store
            .provider_inventory()
            .unwrap()
            .invalid_credentials
            .is_empty());
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM usage_events", [], |r| r
                    .get::<_, usize>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn reviews_reject_local_external_and_reopen_changes_and_failed_delete_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.sqlite3");
        let mut store = Store::open(&path).unwrap();
        insert(&store, "broken-id", None);
        let target = invalid(&store);
        store
            .conn
            .execute("UPDATE providers SET model_name='changed'", [])
            .unwrap();
        assert_eq!(
            store
                .discard_provider_entry(revision(&store), target, 2)
                .unwrap_err()
                .code,
            "RevisionConflict"
        );
        let target = invalid(&store);
        let other = Connection::open(&path).unwrap();
        other
            .execute("UPDATE providers SET model_name='external'", [])
            .unwrap();
        assert_eq!(
            store
                .discard_provider_entry(revision(&store), target, 2)
                .unwrap_err()
                .code,
            "RevisionConflict"
        );
        let target = invalid(&store);
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .discard_provider_entry(revision(&store), target, 2)
                .unwrap_err()
                .code,
            "RevisionConflict"
        );
        store
            .conn
            .execute(
                "UPDATE settings SET active_provider_id='broken-id',polish_enabled=1",
                [],
            )
            .unwrap();
        store.conn.execute_batch("CREATE TRIGGER reject_recovery BEFORE DELETE ON providers BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        let target = invalid(&store);
        let before = revision(&store);
        assert!(store.discard_provider_entry(before, target, 3).is_err());
        assert_eq!(revision(&store), before);
        assert!(store.settings().unwrap().polish_enabled);
        assert_eq!(store.provider_inventory().unwrap().total_providers, 1);
    }

    #[test]
    fn only_matching_canonical_key_accounts_are_cleanup_candidates() {
        let mut store = Store::memory().unwrap();
        let id = Uuid::new_v4();
        let reference = CredentialRef::new(id).unwrap();
        insert(&store, &id.to_string(), Some(&reference.account()));
        store
            .conn
            .execute("UPDATE providers SET endpoint='bad-endpoint'", [])
            .unwrap();
        let target = invalid(&store);
        store
            .discard_provider_entry(revision(&store), target, 2)
            .unwrap();
        assert_eq!(
            store.pending_provider_credentials().unwrap(),
            vec![reference.clone()]
        );
        store
            .conn
            .execute(
                "UPDATE provider_credentials_pending SET provider_id=?1",
                [Uuid::new_v4().to_string()],
            )
            .unwrap();
        assert!(store.pending_provider_credentials().unwrap().is_empty());
        assert_eq!(
            store
                .provider_inventory()
                .unwrap()
                .invalid_credentials
                .len(),
            1
        );
    }

    #[test]
    fn unreadable_rows_still_count_toward_the_twenty_service_limit() {
        let mut store = Store::memory().unwrap();
        for i in 0..20 {
            insert(&store, &format!("broken-{i}"), None);
        }
        assert!(store.providers().unwrap().is_empty());
        let draft = ProviderDraft {
            provider_id: Uuid::new_v4(),
            display_name: "new".into(),
            kind: "ollama".into(),
            endpoint: "http://127.0.0.1:11434/api/chat".into(),
            model_name: "fixture".into(),
            request_profile_json: "{\"kind\":\"ollama\"}".into(),
            consent_origin: None,
            credential_ref: None,
        };
        assert!(store
            .save_provider(revision(&store), None, draft, 2)
            .is_err());
        assert_eq!(store.provider_inventory().unwrap().total_providers, 20);
    }
}
