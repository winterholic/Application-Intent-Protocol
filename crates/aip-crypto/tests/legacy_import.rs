#![cfg(feature = "legacy-import")]
#![allow(clippy::unwrap_used)]
use aes_gcm::{
    Aes256Gcm,
    aead::{Aead, KeyInit, Nonce, Payload},
};
use aip_crypto::{
    cell::{Context, DataKey, Error},
    legacy_import::LegacyKey,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};

fn context() -> Context<'static> {
    Context { schema: "fixture", resource: "Secret", field: "value", row_id: "42", scope_kind: "ref:Project", scope_id: "7" }
}
fn fixture() -> String {
    let cipher = Aes256Gcm::new_from_slice(&[3; 32]).unwrap();
    let nonce = Nonce::<Aes256Gcm>::try_from(&[4_u8; 12][..]).unwrap();
    let body = cipher.encrypt(&nonce, Payload { msg: b"old fixture", aad: b"aip.enc.v1|Secret.value|42" }).unwrap();
    format!("v1:old:{}:{}", STANDARD.encode(nonce), STANDARD.encode(body))
}
#[test]
fn explicit_import_authenticates_legacy_identity_and_reseals_for_destination() {
    let legacy = LegacyKey::new("old", [3; 32]).unwrap();
    let destination = DataKey::new("new-dek", 1, [5; 32]).unwrap();
    let old = fixture();
    assert_eq!(destination.open(&context(), &old), Err(Error::Corrupted));
    let imported = legacy.import_cell("Secret.value", "42", &old, &destination, &context()).unwrap();
    assert!(imported.starts_with("v2:r:new-dek:1:"));
    assert_eq!(destination.open(&context(), &imported).unwrap().as_slice(), b"old fixture");
    assert!(legacy.import_cell("Secret.other", "42", &old, &destination, &context()).is_err());
    assert!(legacy.import_cell("Secret.value", "43", &old, &destination, &context()).is_err());
    assert!(LegacyKey::new("old", [8; 32]).unwrap().import_cell("Secret.value", "42", &old, &destination, &context()).is_err());
    for malformed in [
        "plaintext".to_string(),
        old.replace("v1:old:", "v1:other:"),
        format!("{old}:extra"),
        "v1:old:AAAA:AAAA".into(),
        "x".repeat(2 * aip_crypto::cell::MAX_CELL_BYTES),
    ] {
        assert_eq!(legacy.import_cell("Secret.value", "42", &malformed, &destination, &context()), Err(Error::Corrupted));
    }
}
