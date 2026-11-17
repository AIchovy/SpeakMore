use super::*;
use fixtures::{payload, sign, token, trust};

#[test]
fn independently_signed_node_fixture_interoperates_with_native_verification() {
    let fixture: serde_json::Value =
        serde_json::from_slice(include_bytes!("../testdata/node-interoperability.json")).unwrap();
    let trust = TrustStore::from_json(&serde_json::to_vec(&serde_json::json!({
        "schema":1, "keys":[{"key_id":"node-interop-fixture","public_key":fixture["public_key"]}]
    })).unwrap()).unwrap();
    let token = fixture["token"].as_str().unwrap();
    let verified = trust.verify(token).unwrap();
    assert_eq!(
        verified.license_id(),
        "aacb01a6-b599-4898-92eb-442aef3dc601"
    );
    assert_eq!(verified.issued_at(), 1789400000);
    assert_eq!(
        TrustStore::bundled().unwrap().verify(token).unwrap_err(),
        Failure::UnknownKey
    );
}

#[test]
fn original_bytes_and_offline_permanent_claims_are_verified_without_reserialization() {
    let trust = trust();
    // Whitespace and field order are signed as given, not normalized before verification.
    let reordered = payload().replace('{', "{\n ").replace(',', ",\n ");
    let token = sign(&reordered, SIGNING_DOMAIN);
    let verified = trust.verify(&format!("\n{token}\r\n")).unwrap();
    assert_eq!(verified.encoded_token(), token);
    assert_eq!(verified.major_version(), 1);
    assert_eq!(
        verified.license_id(),
        "627b5d45-b337-4873-b064-1dc7e4550980"
    );
    assert!(!format!("{verified:?}").contains(verified.license_id()));
    assert!(!format!("{verified:?}").contains(&token));
    // issued_at is metadata, never an expiration or a requirement to contact a server.
    for issued in [0, i64::MAX] {
        let future = payload().replace("1789400000", &issued.to_string());
        assert_eq!(
            trust
                .verify(&sign(&future, SIGNING_DOMAIN))
                .unwrap()
                .issued_at(),
            issued
        );
    }
}

#[test]
fn signatures_are_domain_separated_and_tampering_cannot_change_entitlements() {
    let trust = trust();
    for domain in [
        b"".as_slice(),
        b"SpeakMore-License-v1".as_slice(),
        b"SpeakMore-Model-v1\0".as_slice(),
    ] {
        assert_eq!(
            trust.verify(&sign(&payload(), domain)).unwrap_err(),
            Failure::InvalidSignature
        );
    }
    let token = token();
    let (_, signature) = token.split_once('.').unwrap();
    let tampered = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload().replace("1789400000", "1789400001")),
        signature
    );
    assert_eq!(
        trust.verify(&tampered).unwrap_err(),
        Failure::InvalidSignature
    );
    assert_eq!(
        TrustStore::bundled().unwrap().verify(&token).unwrap_err(),
        Failure::UnknownKey
    );
}

#[test]
fn strict_json_and_token_envelope_reject_duplicate_unknown_or_ambiguous_fields() {
    let trust = trust();
    for payload in [
        payload().replace("\"schema\":1", "\"schema\":1,\"schema\":1"),
        payload().replace("\"schema\":1", r#""schema":1,"\u0073chema":1"#),
        payload().replace("\"schema\":1", "\"schema\":1.0"),
        payload().replace("\"schema\":1", "\"schema\":true"),
        payload().replace("\"issued_at\":1789400000", "\"issued_at\":\"1789400000\""),
        payload().replace("\"edition\":\"pro\",", ""),
        payload().replace(
            "\"edition\":\"pro\"",
            "\"edition\":\"pro\",\"expires_at\":0",
        ),
        format!("{}{{}}", payload()),
        "[true]".into(),
    ] {
        assert_eq!(
            trust.verify(&sign(&payload, SIGNING_DOMAIN)).unwrap_err(),
            Failure::InvalidToken
        );
    }
    let token = token();
    let (payload, signature) = token.split_once('.').unwrap();
    for value in [
        "".into(),
        "a.b.c".into(),
        format!("{payload}=.{signature}"),
        format!("{payload}.{signature}="),
        format!("{payload}.AA"),
        format!("{payload}.{}", URL_SAFE_NO_PAD.encode([0u8; 65])),
        " ".repeat(MAX_TOKEN_BYTES + 1),
        format!("{payload}.\n{signature}"),
    ] {
        assert_eq!(trust.verify(&value).unwrap_err(), Failure::InvalidToken);
    }
}

#[test]
fn authenticated_but_incompatible_claims_return_specific_reasons() {
    let trust = trust();
    for (from, to, expected) in [
        ("\"schema\":1", "\"schema\":2", Failure::UnsupportedSchema),
        (
            "\"product\":\"speakmore\"",
            "\"product\":\"other\"",
            Failure::WrongProduct,
        ),
        (
            "\"edition\":\"pro\"",
            "\"edition\":\"free\"",
            Failure::WrongEdition,
        ),
        (
            "\"major_version\":1",
            "\"major_version\":2",
            Failure::UnsupportedMajor,
        ),
        ("1789400000", "-1", Failure::InvalidClaims),
        (
            "627b5d45-b337-4873-b064-1dc7e4550980",
            "00000000-0000-0000-0000-000000000000",
            Failure::InvalidClaims,
        ),
        ("fixture-license-key", "unknown-key", Failure::UnknownKey),
    ] {
        assert_eq!(
            trust
                .verify(&sign(&payload().replace(from, to), SIGNING_DOMAIN))
                .unwrap_err(),
            expected
        );
    }
}

#[test]
fn trust_roots_require_bounded_unique_valid_keys_and_accept_rotation() {
    use ed25519_dalek::SigningKey;
    let public =
        URL_SAFE_NO_PAD.encode(SigningKey::from_bytes(&[39; 32]).verifying_key().as_bytes());
    let second =
        URL_SAFE_NO_PAD.encode(SigningKey::from_bytes(&[40; 32]).verifying_key().as_bytes());
    let key = serde_json::json!({"key_id":fixtures::KEY_ID,"public_key":public});
    let make = |keys| serde_json::to_vec(&serde_json::json!({"schema":1,"keys":keys})).unwrap();
    let rotated = TrustStore::from_json(&make(vec![
        key.clone(),
        serde_json::json!({"key_id":"next-key","public_key":second}),
    ]))
    .unwrap();
    assert!(rotated.verify(&token()).is_ok());
    for keys in [
        vec![],
        vec![key.clone(), key.clone()],
        vec![key.clone(); 17],
        vec![serde_json::json!({"key_id":"invalid id","public_key":public})],
        vec![
            serde_json::json!({"key_id":"weak-key","public_key":URL_SAFE_NO_PAD.encode({let mut identity=[0u8;32];identity[0]=1;identity})}),
        ],
    ] {
        assert!(matches!(
            TrustStore::from_json(&make(keys)),
            Err(Failure::InvalidTrust)
        ));
    }
    assert!(matches!(
        TrustStore::from_json(&vec![b' '; 16_385]),
        Err(Failure::InvalidTrust)
    ));
}
