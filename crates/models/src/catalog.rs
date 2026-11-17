use crate::{ModelError as E, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use speakmore_protocol::{
    auxiliary_names, is_sensevoice_profile, model_artifact_matches_profile, supports_model_profile,
    PARAKEET_PROFILE, QWEN_ASR_PROFILE, SENSEVOICE_TILED_PROFILE,
};
use std::collections::HashSet;

const MAX_PAYLOAD: usize = 512 * 1024;
const MAX_BUNDLE: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub key_id: String,
    pub payload: String,
    pub signature: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustKey {
    pub key_id: String,
    pub public_key: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub role: String,
    pub filename: String,
    pub revision: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub url: String,
    pub license: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub model_id: String,
    /// None preserves the original installation identity and digest. New releases
    /// of the same engine/model use a human-readable label and a digest identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub display_name: String,
    pub family: String,
    pub engine_id: String,
    pub engine_profile: String,
    pub quantization: String,
    pub languages: Vec<String>,
    pub revision: String,
    pub minimum_worker_build: String,
    pub platforms: Vec<String>,
    pub capabilities: Capabilities,
    pub artifacts: Vec<Artifact>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub dictionary_prompt: bool,
    pub max_duration_ms: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub schema_version: u32,
    pub models: Vec<Model>,
}

// Construction requires verification against a separately trusted public key.
// No unsigned deserializer or arbitrary user-provided URLs reach ModelManager.
#[derive(Clone)]
pub struct TrustedCatalog {
    models: Vec<Model>,
}
impl TrustedCatalog {
    pub fn verify(envelope: &[u8], trust: &TrustKey) -> Result<Self> {
        if envelope.len() > MAX_PAYLOAD * 2 {
            return Err(E::UntrustedCatalog);
        }
        let envelope: Envelope =
            serde_json::from_slice(envelope).map_err(|_| E::UntrustedCatalog)?;
        if envelope.key_id != trust.key_id {
            return Err(E::UntrustedCatalog);
        }
        let payload = STANDARD
            .decode(&envelope.payload)
            .map_err(|_| E::UntrustedCatalog)?;
        if payload.len() > MAX_PAYLOAD {
            return Err(E::UntrustedCatalog);
        }
        let public: [u8; 32] = STANDARD
            .decode(&trust.public_key)
            .map_err(|_| E::UntrustedCatalog)?
            .try_into()
            .map_err(|_| E::UntrustedCatalog)?;
        let signature = Signature::from_slice(
            &STANDARD
                .decode(&envelope.signature)
                .map_err(|_| E::UntrustedCatalog)?,
        )
        .map_err(|_| E::UntrustedCatalog)?;
        VerifyingKey::from_bytes(&public)
            .map_err(|_| E::UntrustedCatalog)?
            .verify_strict(&payload, &signature)
            .map_err(|_| E::UntrustedCatalog)?;
        let parsed: Payload = serde_json::from_slice(&payload).map_err(|_| E::InvalidCatalog)?;
        validate(&parsed)?;
        Ok(Self {
            models: parsed.models,
        })
    }
    pub fn models(&self) -> &[Model] {
        &self.models
    }
    pub fn model(&self, id: &str) -> Result<&Model> {
        self.models
            .iter()
            .find(|model| model.installation_id() == id)
            .ok_or(E::UnsupportedModel)
    }
    /// Published installation identities are immutable. Retain old signed
    /// entries so an application upgrade can still verify, load and remove them.
    pub fn preserves(&self, previous: &Self) -> Result<()> {
        for old in previous.models() {
            let retained = self.model(&old.installation_id())?;
            if retained.bundle_digest() != old.bundle_digest() {
                return Err(E::InvalidCatalog);
            }
        }
        Ok(())
    }
}
impl Model {
    pub fn label(&self) -> String {
        self.version.as_ref().map_or_else(
            || self.display_name.clone(),
            |version| format!("{} · {version}", self.display_name),
        )
    }
    pub fn installation_id(&self) -> String {
        if self.version.is_some() {
            format!("{}--{}", self.model_id, self.bundle_digest())
        } else {
            self.model_id.clone()
        }
    }
    pub fn size_bytes(&self) -> u64 {
        self.artifacts.iter().map(|file| file.size_bytes).sum()
    }
    pub fn bundle_digest(&self) -> String {
        digest(&serde_json::to_vec(self).expect("model serialization is infallible"))
    }
    pub fn supported(&self, platform: &str, build: &str) -> bool {
        // Extend only with a packaged and tested native adapter. Catalog data cannot add code.
        let adapter = match (
            self.family.as_str(),
            self.engine_id.as_str(),
            self.engine_profile.as_str(),
        ) {
            ("whisper", "whisper.cpp", "whisper-v1") => matches!(
                self.model_id.as_str(),
                "whisper-small-q5_1" | "whisper-large-v3-turbo-q5_0"
            ),
            ("sensevoice", "sherpa-onnx", profile) if is_sensevoice_profile(profile) => {
                self.model_id == "sensevoice-small-int8" && platform == "macos-aarch64"
            }
            ("parakeet", "sherpa-onnx", PARAKEET_PROFILE) => {
                self.model_id == "parakeet-tdt-v3-int8" && platform == "macos-aarch64"
            }
            ("qwen-asr", "sherpa-onnx", QWEN_ASR_PROFILE) => {
                self.model_id == "qwen3-asr-0.6b-int8" && platform == "macos-aarch64"
            }
            _ => false,
        };
        adapter
            && supports_model_profile(&self.model_id, &self.engine_id, &self.engine_profile)
            && self.platforms.iter().any(|p| p == platform)
            && semver::Version::parse(build)
                .ok()
                .zip(semver::Version::parse(&self.minimum_worker_build).ok())
                .is_some_and(|(current, minimum)| current >= minimum)
    }
}
pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn safe_name(value: &str) -> bool {
    let stem = value.split('.').next().unwrap_or("").to_ascii_lowercase();
    !value.is_empty()
        && value.len() <= 120
        && value.as_bytes()[0].is_ascii_lowercase()
        && !value.ends_with('.')
        && !value.contains("..")
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b))
        && ![
            "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
            "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
        ]
        .contains(&stem.as_str())
}
fn hash(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && value.bytes().any(|b| b != b'0')
}
fn labels(values: &[String]) -> bool {
    !values.is_empty()
        && values.len() <= 32
        && values.iter().all(|s| safe_name(s))
        && values.iter().collect::<HashSet<_>>().len() == values.len()
}
pub fn validate(payload: &Payload) -> Result<()> {
    if payload.schema_version != 1 || payload.models.is_empty() || payload.models.len() > 32 {
        return Err(E::InvalidCatalog);
    }
    let mut ids = HashSet::new();
    let mut version_labels = HashSet::new();
    for model in &payload.models {
        if !safe_name(&model.model_id)
            || model.installation_id().len() > 120
            || !ids.insert(model.installation_id())
            || !version_labels.insert((&model.model_id, &model.version))
            || model.version.as_ref().is_some_and(|v| {
                v.is_empty() || v.len() > 40 || v.trim() != v || v.chars().any(char::is_control)
            })
            || model.display_name.is_empty()
            || model.display_name.len() > 100
            || model.display_name.chars().any(char::is_control)
            || !safe_name(&model.family)
            || !safe_name(&model.engine_id)
            || !safe_name(&model.engine_profile)
            || !safe_name(&model.quantization)
            || !hash(&model.revision, 40)
            || semver::Version::parse(&model.minimum_worker_build).is_err()
            || !labels(&model.languages)
            || !labels(&model.platforms)
            || model.capabilities.max_duration_ms != 300_000
            || model.artifacts.is_empty()
            || model.artifacts.len() > 16
        {
            return Err(E::InvalidCatalog);
        }
        let mut names = HashSet::new();
        let mut roles = HashSet::new();
        let mut total = 0_u64;
        for file in &model.artifacts {
            let url = reqwest::Url::parse(&file.url).map_err(|_| E::InvalidCatalog)?;
            if !safe_name(&file.filename)
                || !safe_name(&file.role)
                || !names.insert(&file.filename)
                || !roles.insert(file.role.as_str())
                || !hash(&file.sha256, 64)
                || !hash(&file.revision, 40)
                || file.size_bytes == 0
                || file.size_bytes > MAX_BUNDLE
                || file.license.is_empty()
                || file.license.len() > 100
                || file.license.chars().any(char::is_control)
                || file.url.len() > 2048
                || url.scheme() != "https"
                || url.host_str() != Some("huggingface.co")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.port().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || !url.path().contains(&format!("/resolve/{}/", file.revision))
                || !url.path().ends_with(&format!("/{}", file.filename))
            {
                return Err(E::InvalidCatalog);
            }
            total = total
                .checked_add(file.size_bytes)
                .ok_or(E::InvalidCatalog)?;
        }
        if total > MAX_BUNDLE {
            return Err(E::InvalidCatalog);
        }
        match (
            model.family.as_str(),
            model.engine_id.as_str(),
            model.engine_profile.as_str(),
        ) {
            ("whisper", "whisper.cpp", "whisper-v1")
                if roles == HashSet::from(["model", "vad"])
                    && model.capabilities.dictionary_prompt => {}
            ("sensevoice", "sherpa-onnx", profile)
                if is_sensevoice_profile(profile)
                    && roles == HashSet::from(["model", "tokens", "vad"])
                    && !model.capabilities.dictionary_prompt
                    && (profile != SENSEVOICE_TILED_PROFILE || model.version.is_some()) => {}
            ("parakeet", "sherpa-onnx", PARAKEET_PROFILE)
                if roles == HashSet::from(["model", "tokens", "vad", "decoder", "joiner"])
                    && !model.capabilities.dictionary_prompt => {}
            ("qwen-asr", "sherpa-onnx", QWEN_ASR_PROFILE)
                if roles
                    == HashSet::from([
                        "model",
                        "vad",
                        "frontend",
                        "decoder",
                        "vocab",
                        "merges",
                        "tokenizer-config",
                    ])
                    && !model.capabilities.dictionary_prompt => {}
            _ => return Err(E::InvalidCatalog),
        }
        if matches!(
            model.engine_profile.as_str(),
            PARAKEET_PROFILE | QWEN_ASR_PROFILE
        ) {
            if !supports_model_profile(&model.model_id, &model.engine_id, &model.engine_profile)
                || !model
                    .artifacts
                    .iter()
                    .any(|a| a.role == "model" && a.filename == "encoder.int8.onnx")
                || auxiliary_names(&model.engine_profile)
                    .iter()
                    .any(|(role, name)| {
                        !model
                            .artifacts
                            .iter()
                            .any(|a| a.role == *role && a.filename == *name)
                    })
            {
                return Err(E::InvalidCatalog);
            }
        }
        let artifact = model
            .artifacts
            .iter()
            .find(|file| file.role == "model")
            .unwrap();
        if !model_artifact_matches_profile(
            &model.engine_profile,
            &artifact.sha256,
            artifact.size_bytes,
        ) {
            return Err(E::InvalidCatalog);
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    #[test]
    fn tiled_graph_is_a_versioned_signed_candidate_and_cannot_relabel_legacy_weights() {
        let mut payload: Payload = serde_json::from_slice(include_bytes!(
            "../../../resources/model-catalog.payload.json"
        ))
        .unwrap();
        let (baseline_bytes, key) = signed(&payload);
        let baseline = TrustedCatalog::verify(&baseline_bytes, &key).unwrap();
        let mut candidate = payload
            .models
            .iter()
            .find(|m| m.family == "sensevoice")
            .unwrap()
            .clone();
        let old_id = candidate.installation_id();
        candidate.version = Some("tiled256-v1".into());
        candidate.engine_profile = SENSEVOICE_TILED_PROFILE.into();
        let artifact = candidate
            .artifacts
            .iter_mut()
            .find(|a| a.role == "model")
            .unwrap();
        artifact.sha256 = speakmore_protocol::SENSEVOICE_TILED_SHA256.into();
        artifact.size_bytes = speakmore_protocol::SENSEVOICE_TILED_BYTES;
        // Synthetic signed metadata; this fixture is never a download source.
        let new_id = candidate.installation_id();
        assert_ne!(new_id, old_id);
        assert!(candidate.supported("macos-aarch64", "0.1.0"));
        assert!(!candidate.supported("macos-x86_64", "0.1.0"));
        assert!(!candidate.supported("windows-x86_64", "0.1.0"));
        payload.models.push(candidate);
        let (encoded, key) = signed(&payload);
        let catalog = TrustedCatalog::verify(&encoded, &key).unwrap();
        catalog.preserves(&baseline).unwrap();
        assert_eq!(
            catalog.model(&old_id).unwrap().engine_profile,
            "sensevoice-v1"
        );
        assert_eq!(
            catalog.model(&new_id).unwrap().engine_profile,
            SENSEVOICE_TILED_PROFILE
        );
        let index = payload.models.len() - 1;
        for which in 0..4 {
            let mut invalid: Payload =
                serde_json::from_slice(&serde_json::to_vec(&payload).unwrap()).unwrap();
            let model = &mut invalid.models[index];
            match which {
                0 => model.version = None,
                1 => model.artifacts[0].sha256 = "4".repeat(64),
                2 => model.artifacts[0].size_bytes -= 1,
                _ => model.capabilities.dictionary_prompt = true,
            }
            let (encoded, key) = signed(&invalid);
            assert!(
                matches!(
                    TrustedCatalog::verify(&encoded, &key),
                    Err(E::InvalidCatalog)
                ),
                "case {which}"
            );
        }
    }
    pub fn fixture(bytes: &[u8]) -> Payload {
        let lock: serde_json::Value =
            serde_json::from_str(include_str!("../../../resources/model-sources.lock.json"))
                .unwrap();
        let source = &lock["models"][0];
        let revision = source["revision"].as_str().unwrap().to_string();
        Payload {
            schema_version: 1,
            models: vec![Model {
                model_id: "whisper-small-q5_1".into(),
                version: None,
                display_name: "Whisper small".into(),
                family: "whisper".into(),
                engine_id: "whisper.cpp".into(),
                engine_profile: "whisper-v1".into(),
                quantization: "q5_1".into(),
                languages: vec!["multilingual".into()],
                revision: revision.clone(),
                minimum_worker_build: "0.1.0".into(),
                platforms: vec!["macos-aarch64".into()],
                capabilities: Capabilities {
                    dictionary_prompt: true,
                    max_duration_ms: 300_000,
                },
                artifacts: ["model", "vad"]
                    .into_iter()
                    .map(|role| Artifact {
                        role: role.into(),
                        filename: format!("{role}.bin"),
                        revision: revision.clone(),
                        size_bytes: bytes.len() as u64,
                        sha256: digest(bytes),
                        url: format!(
                            "https://huggingface.co/test/model/resolve/{revision}/{role}.bin"
                        ),
                        license: "MIT".into(),
                    })
                    .collect(),
            }],
        }
    }
    pub fn signed(payload: &Payload) -> (Vec<u8>, TrustKey) {
        let key = SigningKey::from_bytes(&[37; 32]);
        let raw = serde_json::to_vec(payload).unwrap();
        let envelope = Envelope {
            key_id: "test".into(),
            payload: STANDARD.encode(&raw),
            signature: STANDARD.encode(key.sign(&raw).to_bytes()),
        };
        (
            serde_json::to_vec(&envelope).unwrap(),
            TrustKey {
                key_id: "test".into(),
                public_key: STANDARD.encode(key.verifying_key().to_bytes()),
            },
        )
    }
    pub fn trusted(bytes: &[u8]) -> TrustedCatalog {
        let (envelope, key) = signed(&fixture(bytes));
        TrustedCatalog::verify(&envelope, &key).unwrap()
    }
    #[test]
    fn releases_keep_legacy_digest_and_require_immutable_retained_entries() {
        let trust: TrustKey =
            serde_json::from_slice(include_bytes!("../../../resources/model-trust.json")).unwrap();
        let old = TrustedCatalog::verify(
            include_bytes!("../../../resources/model-catalog.baseline.signed.json"),
            &trust,
        )
        .unwrap();
        let raw: Envelope = serde_json::from_slice(include_bytes!(
            "../../../resources/model-catalog.baseline.signed.json"
        ))
        .unwrap();
        let payload: serde_json::Value =
            serde_json::from_slice(&STANDARD.decode(raw.payload).unwrap()).unwrap();
        // Existing digests are computed from the original field order, with no
        // new null field introduced by the optional version label.
        for (index, model) in old.models().iter().enumerate() {
            assert_eq!(
                model.bundle_digest(),
                [
                    "a00d240d3cffc129c490c34216d64dcc1cad79839fc067da2babcb07a00c29eb",
                    "746b8e700383f695a71ba487aa892a359da62bb8e55020168d28fc97f72bea38",
                    "0c5dc61147dc80d47aca95c8e78de5d63d3a09c49c0f120eed2f5268c8b680f3",
                ][index],
                "published installation path must not move"
            );
            let roundtrip: Model =
                serde_json::from_value(payload["models"][index].clone()).unwrap();
            assert_eq!(model.bundle_digest(), roundtrip.bundle_digest());
            assert!(!String::from_utf8(serde_json::to_vec(model).unwrap())
                .unwrap()
                .contains("\"version\""));
            assert_eq!(model.installation_id(), model.model_id);
        }
        let mut p = fixture(b"old");
        let baseline = trusted(b"old");
        let mut newer = fixture(b"new").models.remove(0);
        newer.version = Some("2026.10".into());
        let id = newer.installation_id();
        assert_ne!(id, newer.model_id);
        assert!(id.ends_with(&newer.bundle_digest()));
        p.models.push(newer.clone());
        let (bytes, key) = signed(&p);
        let release = TrustedCatalog::verify(&bytes, &key).unwrap();
        release.preserves(&baseline).unwrap();
        assert_eq!(
            release.model(&id).unwrap().version.as_deref(),
            Some("2026.10")
        );
        assert_eq!(release.model("whisper-small-q5_1").unwrap().version, None);
        // A new release label cannot overwrite the old logical ID or hide a
        // changed legacy entry, even if all entries are correctly signed.
        p.models.remove(0);
        let (bytes, key) = signed(&p);
        assert!(TrustedCatalog::verify(&bytes, &key)
            .unwrap()
            .preserves(&baseline)
            .is_err());
        let mut changed = fixture(b"changed");
        let (bytes, key) = signed(&changed);
        assert!(TrustedCatalog::verify(&bytes, &key)
            .unwrap()
            .preserves(&baseline)
            .is_err());
        changed.models = vec![newer.clone(), newer];
        changed.models[1].artifacts[0].sha256 = "a".repeat(64);
        assert!(
            validate(&changed).is_err(),
            "same displayed version must not identify two bundles"
        );
        for label in ["", " bad", "bad\n", &"x".repeat(41)] {
            let mut p = fixture(b"x");
            p.models[0].version = Some(label.into());
            assert!(validate(&p).is_err());
        }
    }
    #[test]
    fn raw_signature_is_required_and_bounded() {
        let (encoded, key) = signed(&fixture(b"weights"));
        assert!(TrustedCatalog::verify(&encoded, &key).is_ok());
        let mut envelope: Envelope = serde_json::from_slice(&encoded).unwrap();
        envelope.payload =
            STANDARD.encode(serde_json::to_vec_pretty(&fixture(b"weights")).unwrap());
        assert!(matches!(
            TrustedCatalog::verify(&serde_json::to_vec(&envelope).unwrap(), &key),
            Err(E::UntrustedCatalog)
        ));
        assert!(TrustedCatalog::verify(&vec![b' '; MAX_PAYLOAD * 2 + 1], &key).is_err());
        assert!(TrustedCatalog::verify(
            &encoded,
            &TrustKey {
                key_id: "other".into(),
                ..key
            }
        )
        .is_err());
    }
    #[test]
    fn reject_unsafe_paths_unsigned_hosts_floating_revisions_and_capability_mismatches() {
        for filename in [
            "../model.bin",
            "/model.bin",
            "Model.bin",
            "nul.bin",
            "model..bin",
            "x\\y",
            "model.bin.",
        ] {
            let mut p = fixture(b"x");
            p.models[0].artifacts[0].filename = filename.into();
            assert!(validate(&p).is_err());
        }
        for url in [
            "http://huggingface.co/x",
            "https://evil.example/x",
            "https://huggingface.co/test/model/resolve/main/model.bin",
        ] {
            let mut p = fixture(b"x");
            p.models[0].artifacts[0].url = url.into();
            assert!(validate(&p).is_err());
        }
        let mut p = fixture(b"x");
        p.models[0].artifacts[0].size_bytes = u64::MAX;
        assert!(validate(&p).is_err());
        let mut p = fixture(b"x");
        p.models[0].artifacts.pop();
        assert!(validate(&p).is_err());
        let mut p = fixture(b"x");
        p.models[0].capabilities.dictionary_prompt = false;
        assert!(validate(&p).is_err());
        let mut p = fixture(b"x");
        p.models.push(p.models[0].clone());
        assert!(validate(&p).is_err());
        assert!(!fixture(b"x").models[0].supported("macos-x86_64", "0.1.0"));
        assert!(!fixture(b"x").models[0].supported("macos-aarch64", "0.0.9"));
    }
}
