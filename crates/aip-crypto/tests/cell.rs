#![allow(clippy::unwrap_used)]
use aip_crypto::cell::{Context, DataKey, Error};

fn context() -> Context<'static> {
    Context { schema: "aip_fixture", resource: "Secret", field: "value", row_id: "42", scope_kind: "ref:Project", scope_id: "7" }
}

#[test]
fn cell_roundtrip_is_randomized_and_every_context_element_is_authenticated() {
    let key = DataKey::new("dek-fixture", 1, [7; 32]).unwrap();
    let ctx = context();
    let first = key.seal(&ctx, b"fixture plaintext").unwrap();
    let second = key.seal(&ctx, b"fixture plaintext").unwrap();
    assert_ne!(first, second);
    assert!(first.starts_with("v2:r:dek-fixture:1:"));
    assert_eq!(key.open(&ctx, &first).unwrap().as_slice(), b"fixture plaintext");
    for changed in [
        Context { schema: "aip_other", ..context() },
        Context { resource: "Other", ..context() },
        Context { field: "other", ..context() },
        Context { row_id: "43", ..context() },
        Context { scope_kind: "ref:Other", ..context() },
        Context { scope_id: "8", ..context() },
    ] {
        assert_eq!(key.open(&changed, &first), Err(Error::Corrupted));
    }
    assert_eq!(DataKey::new("other-key-id", 1, [7; 32]).unwrap().open(&ctx, &first), Err(Error::Corrupted));
    assert_eq!(DataKey::new("dek-fixture", 2, [7; 32]).unwrap().open(&ctx, &first), Err(Error::Corrupted));
    assert_eq!(DataKey::new("dek-fixture", 1, [8; 32]).unwrap().open(&ctx, &first), Err(Error::Corrupted));
}

#[test]
fn malformed_or_legacy_frames_never_fall_back_to_plaintext_or_poc_decryption() {
    let key = DataKey::new("dek-fixture", 1, [7; 32]).unwrap();
    for frame in ["plaintext", "v1:key:AAAA:AAAA", "v2:r:dek-fixture:0:AAAA:AAAA", "v2:r:dek-fixture:1:AAAA:AAAA", "v2:r:dek-fixture:01:AAAA:AAAA"] {
        assert_eq!(key.open(&context(), frame), Err(Error::Corrupted));
    }
    let frame = key.seal(&context(), b"fixture").unwrap();
    let mut altered = frame.into_bytes();
    let last = altered.len() - 3;
    altered[last] = if altered[last] == b'A' { b'B' } else { b'A' };
    assert_eq!(key.open(&context(), std::str::from_utf8(&altered).unwrap()), Err(Error::Corrupted));
}

#[test]
fn cell_and_context_limits_reject_before_unbounded_allocation_and_authenticate_header_changes() {
    let key = DataKey::new("dek-fixture", 1, [7; 32]).unwrap();
    let frame = key.seal(&context(), &vec![1; aip_crypto::cell::MAX_CELL_BYTES]).unwrap();
    assert_eq!(key.open(&context(), &frame).unwrap().len(), aip_crypto::cell::MAX_CELL_BYTES);
    assert_eq!(key.seal(&context(), &vec![1; aip_crypto::cell::MAX_CELL_BYTES + 1]), Err(Error::InvalidInput));
    assert_eq!(key.open(&context(), &"x".repeat(2 * aip_crypto::cell::MAX_CELL_BYTES)), Err(Error::Corrupted));
    let too_long = "x".repeat(257);
    assert_eq!(key.seal(&Context { schema: &too_long, ..context() }, b"fixture"), Err(Error::InvalidInput));
    for id in ["", "key:injected", "한글", "key with space"] {
        assert!(DataKey::new(id, 1, [7; 32]).is_err());
    }
    assert!(DataKey::new("key", 0, [7; 32]).is_err());
    let frame = key.seal(&context(), b"fixture").unwrap();
    let renamed = frame.replacen(":dek-fixture:", ":renamed:", 1);
    assert_eq!(DataKey::new("renamed", 1, [7; 32]).unwrap().open(&context(), &renamed), Err(Error::Corrupted));
    let new_version = frame.replacen(":1:", ":2:", 1);
    assert_eq!(DataKey::new("dek-fixture", 2, [7; 32]).unwrap().open(&context(), &new_version), Err(Error::Corrupted));
}
