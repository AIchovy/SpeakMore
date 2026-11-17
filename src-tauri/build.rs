use speakmore_models::catalog::{TrustKey, TrustedCatalog};
fn main() {
    for name in [
        "model-trust.json",
        "model-catalog.signed.json",
        "model-catalog.payload.json",
        "model-catalog.baseline.signed.json",
    ] {
        println!("cargo:rerun-if-changed=../resources/{name}");
    }
    let trust: TrustKey = serde_json::from_slice(include_bytes!("../resources/model-trust.json"))
        .expect("model trust key must be valid");
    let catalog = TrustedCatalog::verify(include_bytes!("../resources/model-catalog.signed.json"), &trust)
        .expect("model catalog must have a trusted signature, fixed revisions, hashes and required artifacts");
    let baseline = TrustedCatalog::verify(
        include_bytes!("../resources/model-catalog.baseline.signed.json"),
        &trust,
    )
    .expect("published model baseline must retain its trusted signature");
    catalog.preserves(&baseline).expect(
        "retain every published model entry unchanged; add a labeled version for new artifacts",
    );
    let source: speakmore_models::catalog::Payload =
        serde_json::from_slice(include_bytes!("../resources/model-catalog.payload.json"))
            .expect("model source must be valid");
    assert_eq!(
        serde_json::to_vec(catalog.models()).unwrap(),
        serde_json::to_vec(&source.models).unwrap(),
        "model source differs from signed payload"
    );
    tauri_build::build()
}
