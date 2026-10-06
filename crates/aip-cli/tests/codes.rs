//! The code registry (`aip-ir/src/codes.rs`) against its consumers: the generated
//! `spec/diagnostics.md`, the conformance expectations, the public contract and the CLI.
//! Regenerate the document on purpose with `AIP_UPDATE_GOLDEN=1 cargo test -p aip-cli --test codes`.

use aip_ir::codes;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn aip(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aip")).args(args).output().expect("run aip")
}

#[test]
fn spec_diagnostics_is_generated_from_the_registry() {
    let path = root().join("spec/diagnostics.md");
    let generated = String::from_utf8(aip(&["diagnostics", "--markdown"]).stdout).expect("utf8");
    assert_eq!(generated, codes::render_markdown(), "the CLI prints the registry rendering");
    if std::env::var("AIP_UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &generated).expect("write spec/diagnostics.md");
    }
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(on_disk == generated, "spec/diagnostics.md differs from the registry (AIP_UPDATE_GOLDEN=1 regenerates it)");
}

#[test]
fn conformance_codes_are_registered() {
    let dir = root().join("conformance/sema");
    let mut seen = BTreeSet::new();
    for e in std::fs::read_dir(&dir).expect("conformance dir") {
        let p = e.expect("entry").path();
        if p.extension().is_some_and(|x| x == "expect") {
            for code in std::fs::read_to_string(&p).expect("expect file").split_whitespace() {
                assert!(codes::lookup(code).is_some(), "{}: {code} is not in the registry", p.display());
                seen.insert(code.to_string());
            }
        }
    }
    assert!(seen.len() > 10, "expected the conformance suite to name many codes, found {seen:?}");
}

fn contract_codes(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(m) => {
            if let Some(Value::Array(errors)) = m.get("errors") {
                for e in errors {
                    if let Some(c) = e.get("code").and_then(Value::as_str) {
                        out.insert(c.to_string());
                    }
                }
            }
            m.values().for_each(|x| contract_codes(x, out));
        }
        Value::Array(a) => a.iter().for_each(|x| contract_codes(x, out)),
        _ => {}
    }
}

#[test]
fn contract_error_codes_are_registered() {
    for app in ["ariari", "shop", "saas", "cms"] {
        let src = root().join(format!("examples/{app}/app.aip"));
        let out = aip(&["contract", &src.to_string_lossy()]);
        assert!(out.status.success(), "aip contract {app} failed");
        let v: Value = serde_json::from_slice(&out.stdout).expect("contract json");
        let mut found = BTreeSet::new();
        contract_codes(&v, &mut found);
        assert!(found.len() > 5, "{app}: found only {found:?}");
        for c in &found {
            let info = codes::lookup(c).unwrap_or_else(|| panic!("{app}: contract lists {c}, which is not in the registry"));
            assert!(info.http_status.is_some(), "{app}: {c} is listed as an intent error, so it must be a runtime code");
        }
    }
}

#[test]
fn registry_examples_parse() {
    for c in codes::all().iter().filter(|c| !c.example.is_empty()) {
        if let Err(d) = aip_syntax::parse_file(c.example) {
            panic!("example of {} does not parse: {}", c.code, d.render("example"));
        }
    }
}

#[test]
fn explain_code_prints_the_entry() {
    let out = aip(&["explain-code", "AIP.CONFLICT.STALE_VERSION"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).expect("utf8");
    assert!(text.contains("http status: 409") && text.contains("retryable with the same idempotency key: no"), "{text}");

    let out = aip(&["explain-code", "--json", "aip-e103"]);
    assert!(out.status.success(), "lookup is case-insensitive");
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["code"], "AIP-E103");
    assert_eq!(v["severity"], "error");

    assert!(!aip(&["explain-code", "NOT_A_CODE"]).status.success());
}

#[test]
fn check_json_only_adds_explain() {
    let dir = std::env::temp_dir().join(format!("aip-codes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let file = dir.join("t.aip");
    let prelude = std::fs::read_to_string(root().join("conformance/sema/_prelude.aip")).expect("prelude");
    let body = std::fs::read_to_string(root().join("conformance/sema/unknown_field.aip")).expect("case");
    std::fs::write(&file, format!("{prelude}\n{body}")).expect("write");
    let out = aip(&["check", &file.to_string_lossy(), "--json"]);
    std::fs::remove_dir_all(&dir).ok();
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    let d = &v[0];
    assert_eq!(d["code"], "AIP-E103");
    assert_eq!(d["explain"], "aip explain-code AIP-E103");
    // serde_json::Value sorts keys, so the order is checked on the raw text
    let text = String::from_utf8(out.stdout).expect("utf8");
    let pos: Vec<usize> =
        ["\"severity\"", "\"code\"", "\"message\"", "\"span\"", "\"explain\""].iter().map(|k| text.find(k).expect("field")).collect();
    assert!(pos.windows(2).all(|w| w[0] < w[1]), "existing fields keep their order, explain comes last: {text}");
}

/// A semantic rule is found on the IR, not on the AST, and every command that needs a program goes through the
/// same pipeline: all of them refuse a program that breaks one and name the registered code.
#[test]
fn semantic_rule_stops_every_command_that_needs_a_program() {
    let dir = std::env::temp_dir().join(format!("aip-sem-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let file = dir.join("t.aip");
    let prelude = std::fs::read_to_string(root().join("conformance/sema/_prelude.aip")).expect("prelude");
    let body = std::fs::read_to_string(root().join("conformance/sema/missing_allow.aip")).expect("case");
    std::fs::write(&file, format!("{prelude}\n{body}")).expect("write");
    let f = file.to_string_lossy().to_string();
    let out_ts = dir.join("client.ts").to_string_lossy().to_string();

    let check = aip(&["check", &f, "--json"]);
    let v: Value = serde_json::from_slice(&check.stdout).expect("json");
    assert_eq!(v[0]["code"], "AIP-E301");
    assert_eq!(v[0]["span"]["line"], 25, "the position comes from the source map");
    assert!(v[0]["help"].is_string(), "the help text of the rule survives the move to the IR");
    assert!(!check.status.success());

    for args in [vec!["ddl", &f], vec!["ir", &f], vec!["core", &f], vec!["explain", &f, "Rename"], vec!["contract", &f], vec!["gen-ts", &f, &out_ts]]
    {
        let out = aip(&args);
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(!out.status.success(), "{args:?} accepted a command without `allow`");
        assert!(err.contains("AIP-E301"), "{args:?}: {err}");
    }
    std::fs::remove_dir_all(&dir).ok();
}
