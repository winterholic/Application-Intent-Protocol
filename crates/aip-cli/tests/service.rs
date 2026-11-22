#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::Value;
use std::{collections::BTreeSet, process::Command};

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn product_cli_checks_the_caller_definition() {
    let definition = concat!(env!("CARGO_MANIFEST_DIR"), "/../../prototype/example/app.aip");
    let output = Command::new(env!("CARGO_BIN_EXE_aip")).args(["service", "check", definition]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["ok"], true);
    assert_eq!(result["executionDigest"].as_str().unwrap().len(), 64);
}

/// The part of the `service check` JSON a tool may rely on, checked as a minimum: the keys below must exist with these types, and
/// any other key is ignored. Adding an optional field must not break this (G3 of `plan-docs/hardening-1.0/diagnostics-contract.md`);
/// removing or retyping one of these must.
mod contract {
    use serde_json::Value;

    fn string(v: &Value, key: &str) -> Result<String, String> {
        v[key].as_str().map(str::to_owned).ok_or(format!("`{key}` must be a string: {v}"))
    }

    fn optional_string(v: &Value, key: &str) -> Result<(), String> {
        match v.get(key) {
            Some(x) if !x.is_string() => Err(format!("`{key}`, when present, must be a string: {v}")),
            _ => Ok(()),
        }
    }

    pub fn success(v: &Value) -> Result<(), String> {
        if v["ok"] != true {
            return Err(format!("`ok` must be true: {v}"));
        }
        if v["executionDigest"].as_str().map(str::len) != Some(64) {
            return Err(format!("`executionDigest` must be 64 characters: {v}"));
        }
        if !v["workers"]["required"].is_boolean() || !v["workers"]["extensions"].is_array() {
            return Err(format!("`workers.required` and `workers.extensions` are required: {v}"));
        }
        for a in v["advisories"].as_array().ok_or("`advisories` must be an array")? {
            for key in ["code", "severity", "message", "anchor"] {
                string(a, key)?;
            }
            optional_string(a, "explain")?;
            for key in ["line", "column"] {
                if a.get(key).is_some_and(|x| x.as_u64().is_none_or(|n| n < 1)) {
                    return Err(format!("`{key}`, when present, must be an integer of at least 1: {a}"));
                }
            }
        }
        Ok(())
    }

    pub fn rejection(v: &Value) -> Result<(), String> {
        if v["ok"] != false || v["code"] != "DEF.INVALID_DEFINITION" {
            return Err(format!("a rejected definition has ok false and code DEF.INVALID_DEFINITION: {v}"));
        }
        let diagnostics = v["diagnostics"].as_array().filter(|d| !d.is_empty()).ok_or("`diagnostics` must be a non-empty array")?;
        for d in diagnostics {
            for key in ["code", "severity", "message"] {
                string(d, key)?;
            }
            for key in ["line", "column"] {
                d[key].as_u64().ok_or(format!("`{key}` must be an unsigned integer: {d}"))?;
            }
            for key in ["scope", "help", "explain", "anchor"] {
                optional_string(d, key)?;
            }
            if d.get("located").is_some_and(|x| !x.is_boolean()) || d.get("scopePath").is_some_and(|x| !x.is_array()) {
                return Err(format!("`located` is a boolean and `scopePath` an array: {d}"));
            }
        }
        Ok(())
    }

    /// Any failure that is not a rejected definition (`SOURCE_IO`, `UNSUPPORTED_FORM`).
    pub fn failure(v: &Value) -> Result<(), String> {
        if v["ok"] != false {
            return Err(format!("`ok` must be false: {v}"));
        }
        string(v, "code")?;
        string(v, "message")?;
        optional_string(v, "reason")
    }
}

fn aip(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aip")).args(args).output().unwrap()
}

fn check(path: &std::path::Path) -> std::process::Output {
    aip(&["service", "check", path.to_str().unwrap()])
}

fn recruitment() -> std::path::PathBuf {
    root().join("spikes/spike-v1-fixture/fixture/recruitment.aip")
}

#[test]
fn service_check_json_keeps_success_and_failure_diagnostic_contracts() {
    let definition = recruitment();
    let success = check(&definition);
    assert_eq!(success.status.code(), Some(0));
    assert!(success.stderr.is_empty());
    let checked: Value = serde_json::from_slice(&success.stdout).unwrap();
    contract::success(&checked).unwrap();
    let keys = |value: &Value| value.as_object().unwrap().keys().cloned().collect::<BTreeSet<_>>();
    let has = |value: &Value, names: &[&str]| names.iter().all(|name| keys(value).contains(*name));
    assert!(has(&checked, &["advisories", "executionDigest", "ok", "workers"]), "{checked}");
    assert_eq!(checked["ok"].as_bool(), Some(true));
    let advisories = checked["advisories"].as_array().unwrap();
    assert_eq!(advisories.len(), 1);
    assert!(has(&advisories[0], &["anchor", "code", "message", "severity"]), "{}", advisories[0]);
    assert_eq!(advisories[0]["severity"].as_str(), Some("warning"));
    assert_eq!(advisories[0]["code"].as_str(), Some("DEF.ADVISORY_PUBLIC_COUNT"));
    assert!(advisories[0]["anchor"].as_str().is_some_and(|v| v.starts_with("line:")));
    // G2: the position of an advisory the compiler reported, in the same coordinates as an error.
    assert_eq!(advisories[0]["anchor"].as_str(), Some("line:60:col:3"));
    assert_eq!((advisories[0]["line"].as_u64(), advisories[0]["column"].as_u64()), (Some(60), Some(3)));
    assert_eq!(advisories[0]["explain"].as_str(), Some("aip explain-code DEF.ADVISORY_PUBLIC_COUNT"));
    assert_eq!(checked["workers"]["required"].as_bool(), Some(true));

    let dir = tempfile::tempdir().unwrap();
    let source = std::fs::read_to_string(&definition).unwrap();
    let invalid = dir.path().join("invalid.aip");
    std::fs::write(&invalid, source.replacen("sort periodEnd, views, id", "sorts periodEnd, views, id", 1)).unwrap();
    let failure = check(&invalid);
    assert_eq!(failure.status.code(), Some(1));
    assert!(failure.stdout.is_empty());
    let rejected: Value = serde_json::from_slice(&failure.stderr).unwrap();
    contract::rejection(&rejected).unwrap();
    assert!(has(&rejected, &["code", "diagnostics", "ok"]), "{rejected}");
    let diagnostics = rejected["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    let diagnostic = &diagnostics[0];
    assert!(has(diagnostic, &["code", "column", "explain", "help", "line", "message", "scope", "severity"]), "{diagnostic}");
    assert_eq!(diagnostic["severity"].as_str(), Some("error"));
    assert_eq!(diagnostic["code"].as_str(), Some("DEF.PARSE_UNKNOWN_KEY"));
    assert_eq!(diagnostic["line"].as_u64(), Some(56));
    assert_eq!(diagnostic["column"].as_u64(), Some(5));
    assert_eq!(diagnostic["anchor"].as_str(), Some("line:56:col:5"));
    assert_eq!(diagnostic["located"], true);
    assert_eq!(diagnostic["scopePath"], serde_json::json!(["resource Recruitment", "expose read"]));
    for key in ["message", "scope", "help", "explain"] {
        assert!(diagnostic[key].is_string(), "{key} must remain a string: {diagnostic}");
    }
}

#[test]
fn a_definition_without_a_position_is_marked_and_keeps_the_zero_coordinates() {
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty.aip");
    std::fs::write(&empty, "").unwrap();
    let out = check(&empty);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let rejected: Value = serde_json::from_slice(&out.stderr).unwrap();
    contract::rejection(&rejected).unwrap();
    let d = &rejected["diagnostics"][0];
    assert_eq!(d["code"], "DEF.MISSING_ITEM");
    // The historical coordinates are not changed (an existing reader still gets 0 and 0) ...
    assert_eq!((d["line"].as_u64(), d["column"].as_u64()), (Some(0), Some(0)), "{d}");
    // ... and a new reader can tell that this is "no position" and not a diagnostic at the first character.
    assert_eq!(d["located"], false, "{d}");
    assert!(d.get("anchor").is_none(), "{d}");
}

#[test]
fn the_contract_check_ignores_fields_it_does_not_know_and_rejects_a_missing_or_retyped_one() {
    let dir = tempfile::tempdir().unwrap();
    let source = std::fs::read_to_string(recruitment()).unwrap();
    let invalid = dir.path().join("invalid.aip");
    std::fs::write(&invalid, source.replacen("sort periodEnd, views, id", "sorts periodEnd, views, id", 1)).unwrap();
    let rejected: Value = serde_json::from_slice(&check(&invalid).stderr).unwrap();
    let checked: Value = serde_json::from_slice(&check(&recruitment()).stdout).unwrap();

    // A later release may add a field anywhere; a reader that follows the contract does not care.
    let mut richer = rejected.clone();
    richer["schema"] = Value::from("aip-diagnostics/1");
    richer["diagnostics"][0]["endLine"] = Value::from(56);
    richer["diagnostics"][0]["related"] = serde_json::json!([{"anchor": "line:1:col:1"}]);
    contract::rejection(&richer).unwrap();
    let mut richer = checked.clone();
    richer["notes"] = serde_json::json!({});
    richer["advisories"][0]["endColumn"] = Value::from(9);
    contract::success(&richer).unwrap();

    // A removed or retyped field must break it, or the check proves nothing.
    let mut missing = rejected.clone();
    missing["diagnostics"][0].as_object_mut().unwrap().remove("line");
    assert!(contract::rejection(&missing).is_err());
    let mut retyped = rejected.clone();
    retyped["diagnostics"][0]["code"] = Value::from(7);
    assert!(contract::rejection(&retyped).is_err());
    let mut bad_help = rejected.clone();
    bad_help["diagnostics"][0]["help"] = Value::from(7);
    assert!(contract::rejection(&bad_help).is_err());
    let mut no_digest = checked.clone();
    no_digest.as_object_mut().unwrap().remove("executionDigest");
    assert!(contract::success(&no_digest).is_err());
}

#[test]
fn a_failure_that_is_not_a_rejected_definition_names_its_reason() {
    let dir = tempfile::tempdir().unwrap();
    let cases: [(&str, Vec<u8>, &str, &str); 4] = [
        ("missing.aip", vec![], "SOURCE_IO", "OPEN_FAILED"),
        ("not_utf8.aip", vec![0xff, 0xfe, 0xfd], "SOURCE_IO", "NOT_UTF8"),
        ("too_large.aip", vec![b' '; (1 << 20) + 1], "INPUT_TOO_LARGE", "OVER_LIMIT"),
        ("kind.txt", b"actor A".to_vec(), "UNSUPPORTED_FORM", ""),
    ];
    for (name, bytes, code, reason) in cases {
        let path = dir.path().join(name);
        if name != "missing.aip" {
            std::fs::write(&path, bytes).unwrap();
        }
        let out = check(&path);
        assert_eq!(out.status.code(), Some(1), "{name}");
        assert!(out.stdout.is_empty(), "{name}");
        let v: Value = serde_json::from_slice(&out.stderr).unwrap();
        contract::failure(&v).unwrap();
        assert_eq!(v["code"], code, "{name}: {v}");
        // `reason` is the stable way to tell the kinds of one code apart; the message only explains.
        assert_eq!(v["reason"].as_str().unwrap_or(""), reason, "{name}: {v}");
        assert!(v.get("diagnostics").is_none(), "{name}: {v}");
    }
}

#[test]
fn the_exit_code_of_check_says_accepted_rejected_or_misused() {
    assert_eq!(check(&recruitment()).status.code(), Some(0));
    let dir = tempfile::tempdir().unwrap();
    let wrong = dir.path().join("wrong.aipoc");
    std::fs::write(&wrong, "entity A {}").unwrap();
    let out = check(&wrong);
    assert_eq!(out.status.code(), Some(1));
    let v: Value = serde_json::from_slice(&out.stderr).unwrap();
    contract::rejection(&v).unwrap();
    assert_eq!(v["diagnostics"][0]["code"], "DEF.WRONG_LANGUAGE");
    // A command line the parser refuses is 2 and is not JSON.
    let usage = aip(&["service", "check", "--no-such-option"]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(serde_json::from_slice::<Value>(&usage.stderr).is_err());
}

#[test]
fn the_registry_listing_carries_severity_and_status_for_every_definition_code() {
    let out = aip(&["diagnostics", "--area", "def", "--json"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let listing: Value = serde_json::from_slice(&out.stdout).unwrap();
    let entries = listing.as_array().or_else(|| listing["codes"].as_array()).unwrap_or_else(|| panic!("unexpected listing: {listing}"));
    assert!(entries.len() >= 99);
    for e in entries {
        for key in ["code", "area", "kind", "title", "explain", "fix", "example", "severity", "status"] {
            assert!(e[key].is_string(), "{key}: {e}");
        }
        let want = if e["code"].as_str().unwrap().starts_with("DEF.ADVISORY_") { "warning" } else { "error" };
        assert_eq!(e["severity"], want, "{e}");
        assert_eq!(e["status"], "active", "{e}");
    }
}

#[test]
fn product_serve_rejects_development_identity_options() {
    let output = Command::new(env!("CARGO_BIN_EXE_aip")).args(["service", "serve", "--config", "unused.json", "--dev-actor", "1"]).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--dev-actor"));
}

#[test]
fn product_cli_generates_python_sdk_bindings() {
    let definition = concat!(env!("CARGO_MANIFEST_DIR"), "/../../prototype/example/app.aip");
    let tmp = tempfile::Builder::new().suffix(".py").tempfile().unwrap();
    let out_path = tmp.path().to_str().unwrap();
    let output =
        Command::new(env!("CARGO_BIN_EXE_aip")).args(["service", "gen", definition, "--lang", "python", "--out", out_path]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let content = std::fs::read_to_string(out_path).unwrap();
    assert!(content.contains("CONTRACT_FINGERPRINT"), "{content}");
    assert!(content.contains("ReadResult"), "{content}");
    assert!(content.contains("ApplyInput"), "{content}");
    assert!(content.contains("JobState"), "{content}");

    let sdk_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../product/python");
    let py_script = r#"
import importlib.util, sys, typing
sys.path.insert(0, sys.argv[1])
spec = importlib.util.spec_from_file_location('generated', sys.argv[2])
assert spec is not None and spec.loader is not None
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
assert hasattr(mod, 'CONTRACT_FINGERPRINT')
assert len(mod.CONTRACT_FINGERPRINT) == 64
assert hasattr(mod, 'ReadResult')
assert hasattr(mod, 'ApplyInput')
assert hasattr(mod, 'JobState')
assert typing.get_args(mod.JobState) == ('QUEUED', 'RUNNING', 'CANCEL_REQUESTED', 'SUCCEEDED', 'FAILED', 'CANCELLED')
print('CLI_IMPORT_OK')
"#;
    let py_check = Command::new("python3").args(["-c", py_script, sdk_path, out_path]).output().unwrap();
    assert!(
        py_check.status.success(),
        "python import failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&py_check.stdout),
        String::from_utf8_lossy(&py_check.stderr)
    );
    assert!(String::from_utf8_lossy(&py_check.stdout).contains("CLI_IMPORT_OK"));
}

#[test]
fn product_cli_generates_python_sdk_bindings_for_operations_with_principal_id() {
    let definition = concat!(env!("CARGO_MANIFEST_DIR"), "/../../spikes/spike-v6-transport/tests/fixtures/operations.aip");
    let tmp = tempfile::Builder::new().suffix(".py").tempfile().unwrap();
    let out_path = tmp.path().to_str().unwrap();
    let output =
        Command::new(env!("CARGO_BIN_EXE_aip")).args(["service", "gen", definition, "--lang", "python", "--out", out_path]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let sdk_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../product/python");
    let py_script = r#"
import importlib.util, sys, typing
sys.path.insert(0, sys.argv[1])
spec = importlib.util.spec_from_file_location('generated_ops', sys.argv[2])
assert spec is not None and spec.loader is not None
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
assert hasattr(mod, 'principalId'), 'operations.aip with actor principal must generate principalId'
assert hasattr(mod, 'LengthOperationInput')
assert mod.LengthOperationInput.__annotations__['owner'] == mod.principalId
assert typing.get_args(mod.JobState) == ('QUEUED', 'RUNNING', 'CANCEL_REQUESTED', 'SUCCEEDED', 'FAILED', 'CANCELLED')
print('OPS_IMPORT_OK')
"#;
    let py_check = Command::new("python3").args(["-c", py_script, sdk_path, out_path]).output().unwrap();
    assert!(
        py_check.status.success(),
        "python import failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&py_check.stdout),
        String::from_utf8_lossy(&py_check.stderr)
    );
    assert!(String::from_utf8_lossy(&py_check.stdout).contains("OPS_IMPORT_OK"));
}
