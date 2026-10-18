//! The registry of wire codes against the sources that can emit them: no code without an entry, no entry that
//! nothing emits, no code without the `WIRE.` area, and the generated document equals the registry.
use spike_v6_transport::wire_codes::{self, Retry};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Uppercase string literals of the read, write, worker and transport layers that are not wire codes.
const NOT_CODES: &[&str] = &[
    "DESC",
    "HOME",
    "PATH",
    "PLAIN",
    "PRIVATE_VALUE",
    "PYTHONPATH",
    "ROLLBACK",
    "CARGO_MANIFEST_DIR",
    // The value `JSON` is the name of a MySQL column type that spike-v2-read/src/mysql_rows.rs puts into a
    // `WIRE.INTERNAL` message (`malformed("JSON")`). It is never a code.
    "JSON",
    // `EVAL_RO`, `EXISTS` and `PING` are Redis commands that spike-v4-worker/src/capability.rs sends to the store with
    // `redis::cmd(..)`. Only these three are listed: every other bare uppercase literal, including other Redis
    // commands, is still reported by `no_code_is_written_without_its_area`.
    "EVAL_RO",
    "EXISTS",
    "PING",
];
/// Job status values: a state of a job, not an error code, so they keep their bare names. `RETRY`, `PENDING` and
/// `RUNNING` are the attempt states that the job metrics and the OTLP span status mapping name.
const JOB_STATUSES: &[&str] = &["CANCELLED", "CANCEL_REQUESTED", "FAILED", "PENDING", "RETRY", "RUNNING", "SUCCEEDED"];

/// `(codes with the WIRE area, uppercase literals without an area)` of the layers that emit wire codes.
fn scan() -> (BTreeSet<String>, BTreeSet<String>) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found = BTreeSet::new();
    let mut bare = BTreeSet::new();
    for layer in ["spike-v2-read", "spike-v3-write", "spike-v4-worker", "spike-v6-transport"] {
        for entry in std::fs::read_dir(root.join(layer).join("src")).expect("src dir") {
            let path = entry.expect("entry").path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // test modules and the registry itself name codes as data
            if !name.ends_with(".rs") || name.ends_with("_tests.rs") || name == "wire_codes.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("source");
            scan_literals(&text, &mut found, &mut bare);
        }
    }
    (found, strays(bare))
}

/// Sorts the string literals of one source text into `WIRE.` codes and uppercase literals without an area.
fn scan_literals(text: &str, found: &mut BTreeSet<String>, bare: &mut BTreeSet<String>) {
    for literal in text.split('"').skip(1).step_by(2) {
        let code_like = |l: &str| {
            l.len() >= 4
                && l.starts_with(|c: char| c.is_ascii_uppercase())
                && l.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        };
        if literal.strip_prefix("WIRE.").is_some_and(code_like) {
            found.insert(literal.to_string());
        // methods and environment variables are not codes
        } else if code_like(literal)
            && !literal.starts_with("AIP_")
            && !["GET", "POST", "HEAD", "PUT", "PATCH", "DELETE", "NULL", "TRUE", "FALSE", "NONE", "OPTIONS"].contains(&literal)
        {
            bare.insert(literal.to_string());
        }
    }
}

/// The bare literals that are neither listed non-codes nor job statuses.
fn strays(mut bare: BTreeSet<String>) -> BTreeSet<String> {
    bare.retain(|c| !NOT_CODES.contains(&c.as_str()) && !JOB_STATUSES.contains(&c.as_str()));
    bare
}

fn emitted() -> BTreeSet<String> {
    scan().0
}

/// A code without its area would be the old scheme again (`WIRE.BAD_VALUE` instead of `WIRE.BAD_VALUE`).
#[test]
fn no_code_is_written_without_its_area() {
    let (_, bare) = scan();
    assert!(bare.is_empty(), "uppercase literals without the WIRE. area in the wire layers: {bare:?}");
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
    let reduced: BTreeSet<String> = wire_codes::all().iter().filter(|c| c.code != "WIRE.COMMIT_UNKNOWN").map(|c| c.code.to_string()).collect();
    let missing: Vec<&String> = found.iter().filter(|c| !reduced.contains(*c)).collect();
    assert_eq!(missing, vec!["WIRE.COMMIT_UNKNOWN"]);
}

#[test]
fn no_entry_is_dead_and_every_entry_is_complete() {
    let found = emitted();
    let mut seen = BTreeSet::new();
    for c in wire_codes::all() {
        assert!(seen.insert(c.code), "{} is registered twice", c.code);
        assert!(found.contains(c.code), "{} is registered but nothing emits it", c.code);
        assert!(!c.explain.is_empty() && !c.fix.is_empty(), "{} needs an explanation and a fix", c.code);
        assert!(c.code.starts_with("WIRE."), "{} is not in the WIRE area", c.code);
    }
}

#[test]
fn the_retry_of_the_codes_that_decide_a_write_is_the_documented_one() {
    let retry = |code: &str| wire_codes::lookup(code).expect("registered").retry;
    // an unknown outcome is never retried blind; a refused request is not retried unchanged
    assert_eq!(retry("WIRE.COMMIT_UNKNOWN"), Retry::Check);
    assert_eq!(retry("WIRE.EFFECT_UNKNOWN"), Retry::Check);
    assert_eq!(retry("WIRE.IDEMPOTENCY_MISMATCH"), Retry::Never);
    assert_eq!(retry("WIRE.DB_UNAVAILABLE"), Retry::Later);
    assert_eq!(retry("WIRE.TOKEN_EXPIRED"), Retry::Reauthenticate);
    assert_eq!(retry("WIRE.BAD_VALUE"), Retry::Never);
}

/// The managed Mongo/Redis codes carry a class and a retry that keep the operator in charge of configuration faults.
#[test]
fn the_managed_capability_codes_keep_their_class_and_retry() {
    use wire_codes::Class;
    let entry = |code: &str| wire_codes::lookup(code).unwrap_or_else(|| panic!("{code} is registered"));
    // refused although declared: the same call fails the same way
    assert_eq!((entry("WIRE.CAPABILITY_DENIED").class, entry("WIRE.CAPABILITY_DENIED").retry), (Class::Server, Retry::Never));
    // not declared at all stays a separate code from the one above
    assert_eq!((entry("WIRE.ACCESS_NOT_DECLARED").class, entry("WIRE.ACCESS_NOT_DECLARED").retry), (Class::Server, Retry::Never));
    assert!(entry("WIRE.CAPABILITY_DENIED").explain.contains("WIRE.ACCESS_NOT_DECLARED"));
    // an out-of-range setting is never cured by sending the request again, and it is not BAD_CONFIGURATION
    assert_eq!((entry("WIRE.BAD_CONFIG").class, entry("WIRE.BAD_CONFIG").retry), (Class::Server, Retry::Never));
    assert!(entry("WIRE.BAD_CONFIG").explain.contains("WIRE.BAD_CONFIGURATION"));
    assert_eq!((entry("WIRE.BAD_CONFIGURATION").class, entry("WIRE.BAD_CONFIGURATION").retry), (Class::Server, Retry::Never));
    // an adapter fault mixes outages with missing settings: retry is `later`, but the fix text must not promise success
    let unavailable = entry("WIRE.ADAPTER_UNAVAILABLE");
    assert_eq!((unavailable.class, unavailable.retry), (Class::Availability, Retry::Later));
    assert!(unavailable.explain.contains("not configured") && unavailable.explain.contains("timed out"), "every cause group is named");
    assert!(unavailable.fix.contains("only when the cause was an outage or a timeout"), "the retry condition is conservative");
    assert!(unavailable.fix.contains("fails the same way until the operator fixes it"), "a configuration fault is not auto-retried");
}

/// The scan must keep catching what it exists for: a code that is not registered and a code without its area.
#[test]
fn an_injected_code_or_bare_literal_is_detected() {
    let (mut found, mut bare) = (BTreeSet::new(), BTreeSet::new());
    scan_literals(
        r#"reject("WIRE.GHOST_CODE"); reject("GHOST_CODE"); redis::cmd("HGETALL"); redis::cmd("EXISTS"); malformed("JSON");"#,
        &mut found,
        &mut bare,
    );
    let registered: BTreeSet<String> = wire_codes::all().iter().map(|c| c.code.to_string()).collect();
    assert!(found.iter().any(|c| c == "WIRE.GHOST_CODE") && !registered.contains("WIRE.GHOST_CODE"), "an unregistered WIRE code is reported");
    let stray = strays(bare);
    // only the four listed literals are exempt: an unlisted Redis command and a made-up bare code are still reported
    assert_eq!(stray, ["GHOST_CODE", "HGETALL"].map(String::from).into_iter().collect::<BTreeSet<_>>());
    for exempt in ["EVAL_RO", "EXISTS", "PING", "JSON"] {
        assert!(NOT_CODES.contains(&exempt), "{exempt} is listed");
    }
}

#[test]
fn value_too_long_documents_every_read_filter_operator_that_emits_it() {
    let code = wire_codes::lookup("WIRE.VALUE_TOO_LONG").expect("registered");
    assert!(code.explain.contains("`like`"), "like emits WIRE.VALUE_TOO_LONG and belongs in its explanation");
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
