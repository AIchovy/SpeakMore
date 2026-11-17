//! Development-only catalog, restricted to a separate application identity.
//! No runtime path, URL, key or catalog override is accepted from IPC or env.
use speakmore_core::{error::Result, storage::Store, types::SettingsPatch};
use speakmore_models::catalog::{TrustKey, TrustedCatalog};

pub(crate) const IDENTIFIER: &str = "com.speakmore.modelcheck.20260916";

pub(crate) fn catalog(baseline: &TrustedCatalog) -> speakmore_models::Result<TrustedCatalog> {
    let trust: TrustKey =
        serde_json::from_slice(include_bytes!("../fixtures/model-check/trust.json"))
            .map_err(|_| speakmore_models::ModelError::UntrustedCatalog)?;
    let candidate = TrustedCatalog::verify(
        include_bytes!("../fixtures/model-check/catalog.signed.json"),
        &trust,
    )?;
    candidate.preserves(baseline)?;
    Ok(candidate)
}

pub(crate) fn prepare(store: &mut Store) -> Result<()> {
    // Only this development identity calls here. Never register its recording
    // shortcuts on launch, and keep delivery local/manual for UI checks.
    let before = store.shortcuts()?;
    let mut next = before.clone();
    next.push_to_talk.enabled = false;
    next.cancel.enabled = false;
    if before != next {
        store.update_shortcuts(&before, &next, crate::now())?;
    }
    let settings = store.settings()?;
    if settings.delivery_mode != "manual"
        || settings.polish_enabled
        || settings.history_enabled
        || settings.processing_policy != "local_only"
    {
        store.update_settings(
            settings.revision,
            SettingsPatch {
                delivery_mode: Some("manual".into()),
                polish_enabled: Some(false),
                history_enabled: Some(false),
                processing_policy: Some("local_only".into()),
                ..Default::default()
            },
            crate::now(),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separate_catalog_retains_production_and_pins_one_candidate() {
        let trust: TrustKey =
            serde_json::from_slice(include_bytes!("../../resources/model-trust.json")).unwrap();
        let baseline = TrustedCatalog::verify(
            include_bytes!("../../resources/model-catalog.signed.json"),
            &trust,
        )
        .unwrap();
        let next = catalog(&baseline).unwrap();
        assert_eq!(next.models().len(), baseline.models().len() + 1);
        let model = next.models().last().unwrap();
        assert_eq!(model.version.as_deref(), Some("tiled256-v1"));
        assert_eq!(
            model.engine_profile,
            speakmore_protocol::SENSEVOICE_TILED_PROFILE
        );
        let graph = model.artifacts.iter().find(|a| a.role == "model").unwrap();
        assert_eq!(graph.sha256, speakmore_protocol::SENSEVOICE_TILED_SHA256);
        assert_eq!(graph.size_bytes, speakmore_protocol::SENSEVOICE_TILED_BYTES);
        assert_eq!(model.installation_id(), "sensevoice-small-int8--449f53e709f54de54d48c2bbfa225bd5d2f69e9ad620f78c946cd69de9dc16d2");
        // The public development signer must not be trusted by normal builds.
        assert!(TrustedCatalog::verify(
            include_bytes!("../fixtures/model-check/catalog.signed.json"),
            &trust
        )
        .is_err());
    }

    #[test]
    fn preparation_is_idempotent_and_keeps_an_empty_local_database() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = Store::open(&temp.path().join("check.sqlite3")).unwrap();
        prepare(&mut store).unwrap();
        let revision = store.settings().unwrap().revision;
        prepare(&mut store).unwrap();
        assert_eq!(store.settings().unwrap().revision, revision);
        assert_eq!(store.settings().unwrap().delivery_mode, "manual");
        let shortcuts = store.shortcuts().unwrap();
        assert!(!shortcuts.push_to_talk.enabled && !shortcuts.cancel.enabled);
        assert!(store.settings().unwrap().active_model_id.is_none());
    }
}
