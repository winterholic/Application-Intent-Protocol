//! The registry of wire codes against the sources that can emit them: no code without an entry, no entry that
//! nothing emits, and the generated document equals the registry.
use spike_v6_transport::wire_codes::{self, Class, Retry};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Uppercase string literals of the read, write, worker and transport layers that are not wire codes.
const NOT_CODES: &[&str] = &["DESC", "HOME", "PATH", "PLAIN", "PRIVATE_VALUE", "PYTHONPATH", "ROLLBACK", "CARGO_MANIFEST_DIR"];

fn emitted() -> BTreeSet<String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found = BTreeSet::new();
    for layer in ["spike-v2-read", "spike-v3-write", "spike-v4-worker", "spike-v6-transport"] {
        for entry in std::fs::read_dir(root.join(layer).join("src")).expect("src dir") {
            let path = entry.expect("entry").path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // test modules and the registry itself name codes as data
            if !name.ends_with(".rs") || name.ends_with("_tests.rs") || name == "wire_codes.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("source");
            for literal in text.split('"').skip(1).step_by(2) {
                let code_like = literal.len() >= 4
                    && literal.starts_with(|c: char| c.is_ascii_uppercase())
                    && literal.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
                // methods and environment variables are not codes
                if code_like
                    && !literal.starts_with("AIP_")
                    && !["GET", "POST", "HEAD", "PUT", "PATCH", "DELETE", "NULL", "TRUE", "FALSE", "NONE", "OPTIONS"].contains(&literal)
                {
                    found.insert(literal.to_string());
                }
            }
        }
    }
    found.retain(|c| !NOT_CODES.contains(&c.as_str()));
    found
}

#[test]
fn every_emitted_code_has_an_entry() {
    let found = emitted();
    assert!(found.len() > 60, "the scan found only {} codes; is it looking at the right files?", found.len());
    let registered: BTreeSet<String> = wire_codes::all().iter().map(|c| c.code.to_string()).collect();
    let missing: Vec<&String> = found.iter().filter(|c| !registered.contains(*c)).collect();
    assert!(missing.is_empty(), "codes in the sources without an entry in src/wire_codes.rs: {missing:?}");
}

#[test]
fn removing_an_entry_is_detected() {
    // negative control: the completeness check must fail when an entry is missing
    let found = emitted();
    let reduced: BTreeSet<String> = wire_codes::all().iter().filter(|c| c.code != "COMMIT_UNKNOWN").map(|c| c.code.to_string()).collect();
    let missing: Vec<&String> = found.iter().filter(|c| !reduced.contains(*c)).collect();
    assert_eq!(missing, vec!["COMMIT_UNKNOWN"]);
}

#[test]
fn no_entry_is_dead_and_every_entry_is_complete() {
    let found = emitted();
    let mut seen = BTreeSet::new();
    for c in wire_codes::all() {
        assert!(seen.insert(c.code), "{} is registered twice", c.code);
        assert!(found.contains(c.code), "{} is registered but nothing emits it", c.code);
        assert!(!c.explain.is_empty() && !c.fix.is_empty(), "{} needs an explanation and a fix", c.code);
        // a status is the only kind of entry that is not a failure
        assert_eq!(c.class == Class::Status, c.retry == Retry::NotAFailure, "{}: class and retry disagree", c.code);
    }
}

#[test]
fn the_retry_of_the_codes_that_decide_a_write_is_the_documented_one() {
    let retry = |code: &str| wire_codes::lookup(code).expect("registered").retry;
    // an unknown outcome is never retried blind; a refused request is not retried unchanged
    assert_eq!(retry("COMMIT_UNKNOWN"), Retry::Check);
    assert_eq!(retry("EFFECT_UNKNOWN"), Retry::Check);
    assert_eq!(retry("IDEMPOTENCY_MISMATCH"), Retry::Never);
    assert_eq!(retry("DB_UNAVAILABLE"), Retry::Later);
    assert_eq!(retry("TOKEN_EXPIRED"), Retry::Reauthenticate);
    assert_eq!(retry("BAD_VALUE"), Retry::Never);
}

#[test]
fn spec_wire_errors_is_generated_from_the_registry() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/wire-errors.md");
    let generated = wire_codes::render_markdown();
    if std::env::var("AIP_UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &generated).expect("write spec/wire-errors.md");
    }
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(on_disk == generated, "spec/wire-errors.md differs from the registry (AIP_UPDATE_GOLDEN=1 regenerates it)");
}
