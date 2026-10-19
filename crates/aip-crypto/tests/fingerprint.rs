#![allow(clippy::unwrap_used)]
use aip_crypto::{
    cell::Error,
    fingerprint::{Context, Keys},
};

fn context() -> Context<'static> {
    Context { schema: "aip_fixture", principal: "test:alice", request_key: "request-1", resource: "Secret", field: "value" }
}

#[test]
fn stored_key_identity_survives_active_key_rotation_and_missing_keys_fail_closed() {
    let old = Keys::new("fp1", vec![("fp1", [1; 32])]).unwrap();
    let stored = old.idem(&context(), br#""fixture value""#).unwrap();
    let rotated = Keys::new("fp2", vec![("fp2", [2; 32]), ("fp1", [1; 32])]).unwrap();
    assert_ne!(stored, rotated.idem(&context(), br#""fixture value""#).unwrap());
    assert_eq!(rotated.verify_idem(&context(), br#""fixture value""#, &stored), Ok(true));
    assert_eq!(rotated.verify_idem(&context(), br#""changed value""#, &stored), Ok(false));
    let removed = Keys::new("fp2", vec![("fp2", [2; 32])]).unwrap();
    assert_eq!(removed.verify_idem(&context(), br#""fixture value""#, &stored), Err(Error::Unavailable));
}

#[test]
fn fingerprint_binds_every_request_identity_and_unambiguous_component_boundary() {
    let keys = Keys::new("fp1", vec![("fp1", [1; 32])]).unwrap();
    let stored = keys.idem(&context(), b"a|b").unwrap();
    for changed in [
        Context { schema: "aip_other", ..context() },
        Context { principal: "test:bob", ..context() },
        Context { request_key: "request-2", ..context() },
        Context { resource: "Other", ..context() },
        Context { field: "other", ..context() },
    ] {
        assert_eq!(keys.verify_idem(&changed, b"a|b", &stored), Ok(false));
    }
    let a = Context { resource: "A|B", field: "C", ..context() };
    let b = Context { resource: "A", field: "B|C", ..context() };
    assert_ne!(keys.idem(&a, b"value").unwrap(), keys.idem(&b, b"value").unwrap());
    for malformed in ["idem_v2:fp1:00", "audit_v2:fp1:deadbeef", "plaintext", "idem_v2:fp1:00:extra"] {
        assert_eq!(keys.verify_idem(&context(), b"a|b", malformed), Err(Error::Corrupted));
    }
}

#[test]
fn audit_and_blind_index_domains_are_separate_and_retained_tags_keep_exact_byte_equality() {
    use aip_crypto::{cell, fingerprint::BlindIndexContext};
    let keys = Keys::new("fp2", vec![("fp1", [1; 32]), ("fp2", [2; 32])]).unwrap();
    let ctx = cell::Context { schema: "aip_fixture", resource: "Secret", field: "value", row_id: "42", scope_kind: "ref:Project", scope_id: "7" };
    let marker = keys.audit(&ctx, b"CaseSensitive").unwrap();
    assert_eq!(keys.verify_audit(&ctx, b"CaseSensitive", &marker), Ok(true));
    assert_eq!(keys.verify_audit(&ctx, b"casesensitive", &marker), Ok(false));
    assert_eq!(keys.verify_audit(&cell::Context { row_id: "43", ..ctx.clone() }, b"CaseSensitive", &marker), Ok(false));
    assert_eq!(keys.verify_idem(&context(), b"CaseSensitive", &marker), Err(Error::Corrupted));
    let bidx = BlindIndexContext { schema: ctx.schema, resource: ctx.resource, field: ctx.field, scope_kind: ctx.scope_kind, scope_id: ctx.scope_id };
    let tags = keys.blind_tags(&bidx, b"CaseSensitive").unwrap();
    assert_eq!(tags.len(), 2);
    assert!(tags[0].starts_with("bidx_v1:fp1:") && tags[1].starts_with("bidx_v1:fp2:"));
    assert!(!tags.iter().any(|tag| tag.contains('=')));
    let old = Keys::new("fp1", vec![("fp1", [1; 32])]).unwrap();
    assert_eq!(old.blind_tags(&bidx, b"CaseSensitive").unwrap()[0], tags[0]);
    assert_ne!(tags, keys.blind_tags(&bidx, b"casesensitive").unwrap());
    assert_ne!(tags, keys.blind_tags(&BlindIndexContext { scope_id: "8", ..bidx }, b"CaseSensitive").unwrap());
}
