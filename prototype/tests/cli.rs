use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("aip-prototype-cli-{}-{}", std::process::id(), NEXT_TEMP.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).expect("create isolated test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_aip-prototype"))
}

fn fixture() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("example/app.aip");
    fs::read_to_string(path).expect("read canonical V15 fixture")
}

fn write_source(dir: &TempDir, name: &str, source: &str) -> PathBuf {
    let path = dir.path().join(name);
    fs::write(&path, source).expect("write temporary AIP source");
    path
}

fn check(source: &Path, dev_checks: bool) -> Output {
    let mut command = binary();
    command.arg("check").arg(source);
    if dev_checks {
        command.arg("--dev-checks");
    }
    command.output().expect("run prototype check command")
}

fn gen(source: &Path, out: &Path, wire: &str, dev_checks: bool) -> Output {
    let mut command = binary();
    command.arg("gen").arg(source).arg("--wire").arg(wire).arg("--out").arg(out).arg("--sdk-import").arg("../sdk/index.ts");
    if dev_checks {
        command.arg("--dev-checks");
    }
    command.output().expect("run prototype gen command")
}

fn json_stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("stdout must be JSON, got {:?}: {error}", String::from_utf8_lossy(&output.stdout)))
}

fn assert_digest(value: &Value) -> &str {
    let digest = value.get("executionDigest").and_then(Value::as_str).expect("check result contains executionDigest");
    assert_eq!(digest.len(), 64, "execution digest is lowercase SHA-256 hex");
    assert!(digest.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)), "execution digest is lowercase hex");
    digest
}

#[test]
fn check_emits_a_successful_execution_summary_as_json() {
    let dir = TempDir::new();
    let source = write_source(&dir, "app.aip", &fixture());
    let output = check(&source, false);

    assert!(output.status.success(), "check should succeed: {}", String::from_utf8_lossy(&output.stderr));
    let result = json_stdout(&output);
    assert_eq!(result.get("ok"), Some(&Value::Bool(true)));
    assert_digest(&result);
    assert!(result.get("advisories").and_then(Value::as_array).is_some());
}

#[test]
fn numeric_overflow_reports_definition_diagnostics_and_preserves_the_binding() {
    let dir = TempDir::new();
    let out = dir.path().join("contract.ts");
    let original = "existing usable binding";
    for (old, new, diagnostic) in [
        ("rows 50", "rows 9223372036854775808", "LEX_NUMBER_RANGE"),
        ("deadline 1s", "deadline 9223372036854775807s", "LEX_DURATION_RANGE"),
        ("deadline 1s", "deadline 307445734561826m", "LEX_DURATION_RANGE"),
    ] {
        assert!(fixture().contains(old));
        let source = write_source(&dir, "overflow.aip", &fixture().replace(old, new));
        fs::write(&out, original).unwrap();
        for output in [check(&source, false), gen(&source, &out, "safe", false)] {
            assert_eq!(output.status.code(), Some(1), "definition error must not panic: {}", String::from_utf8_lossy(&output.stderr));
            assert!(output.stdout.is_empty());
            let error: Value = serde_json::from_slice(&output.stderr).expect("structured definition diagnostic");
            assert_eq!(error["code"], "INVALID_DEFINITION");
            assert_eq!(error["diagnostics"][0]["code"], diagnostic);
            assert!(error["diagnostics"][0]["line"].as_u64().unwrap() > 0);
            assert!(error["diagnostics"][0]["col"].as_u64().unwrap() > 0);
        }
        assert_eq!(fs::read_to_string(&out).unwrap(), original);
        assert!(!fs::read_dir(dir.path()).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".aip-gen-")));
    }
}

#[cfg(unix)]
fn closed_output() -> std::process::Stdio {
    use std::os::{fd::OwnedFd, unix::net::UnixStream};
    let (reader, writer) = UnixStream::pair().unwrap();
    drop(reader);
    let fd: OwnedFd = writer.into();
    fd.into()
}

#[test]
#[cfg(unix)]
fn a_closed_stdout_consumer_does_not_turn_a_successful_check_or_gen_into_a_panic() {
    let dir = TempDir::new();
    let source = write_source(&dir, "app.aip", &fixture());
    let out = dir.path().join("contract.ts");
    let checked = binary().arg("check").arg(&source).stdout(closed_output()).output().unwrap();
    let generated = binary()
        .arg("gen")
        .arg(&source)
        .args(["--wire", "decimal", "--sdk-import", "../sdk/index.ts", "--out"])
        .arg(&out)
        .stdout(closed_output())
        .output()
        .unwrap();
    for output in [checked, generated] {
        assert_eq!(output.status.code(), Some(0), "consumer cancellation after success: {}", String::from_utf8_lossy(&output.stderr));
        assert!(output.stderr.is_empty());
    }
    assert!(fs::read_to_string(&out).unwrap().contains("export const contract:"));
}

#[test]
#[cfg(unix)]
fn a_closed_error_consumer_preserves_the_original_failure_exit_without_a_panic() {
    let dir = TempDir::new();
    let source = write_source(&dir, "invalid.aip", "resource");
    let output = binary().arg("check").arg(source).stderr(closed_output()).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

#[test]
fn excessive_definition_nesting_is_a_diagnostic_instead_of_a_process_abort() {
    let dir = TempDir::new();
    let parentheses = format!("{}m.id = m.id{}", "(".repeat(1000), ")".repeat(1000));
    let negations = format!("{}m.id = m.id", "not ".repeat(1000));
    let with_predicate = |expression: &str| format!("{}\npredicate deep(m: Member) = {expression}\n", fixture());
    let mut cases: Vec<(String, String, &str)> = vec![
        ("parentheses.aip".into(), with_predicate(&parentheses), "PARSE_NESTING"),
        ("not.aip".into(), with_predicate(&negations), "PARSE_NESTING"),
        (
            "parentheses.e.ts".into(),
            format!("import {{aip}} from '@aip/define';\nexport default aip`{}`;", with_predicate(&parentheses)),
            "PARSE_NESTING",
        ),
        ("not.e.py".into(), format!("from aip.define import aip\nSPEC = aip(\"\"\"{}\"\"\")", with_predicate(&negations)), "PARSE_NESTING"),
    ];
    for (suffix, key) in [("ts", "nestedTest"), ("py", "\"nestedTest\"")] {
        let source =
            fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../spikes/spike-v1-fixture/fixture/recruitment.h.{suffix}")))
                .unwrap();
        let index = source.rfind("})").unwrap();
        for (kind, value) in [
            ("arrays", format!("{}1{}", "[".repeat(10000), "]".repeat(10000))),
            ("objects", format!("{}1{}", "{\"a\":".repeat(5000), "}".repeat(5000))),
        ] {
            cases.push((format!("{kind}.h.{suffix}"), format!("{} {key}: {value},\n{}", &source[..index], &source[index..]), "H_LITERAL_NESTING"));
        }
    }
    let out = dir.path().join("contract.ts");
    fs::write(&out, "existing usable binding").unwrap();
    let mut failures = Vec::new();
    for (name, text, diagnostic) in cases {
        let source = write_source(&dir, &name, &text);
        for output in [check(&source, false), gen(&source, &out, "safe", false)] {
            let error = serde_json::from_slice::<Value>(&output.stderr).ok();
            if output.status.code() != Some(1)
                || error.as_ref().is_none_or(|e| e["code"] != "INVALID_DEFINITION" || e["diagnostics"][0]["code"] != diagnostic)
            {
                failures.push(format!("{name}: exit {:?}, stderr {}", output.status.code(), String::from_utf8_lossy(&output.stderr)));
            }
        }
        assert_eq!(fs::read_to_string(&out).unwrap(), "existing usable binding");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn check_dev_advisories_do_not_change_execution_or_read_docs_as_calls() {
    let dir = TempDir::new();
    let source_text = format!("{}\npredicate orphan(m: Member) = m.id = m.id\n", fixture());
    let source = write_source(&dir, "app-with-orphan.aip", &source_text);
    let off = check(&source, false);
    let on = check(&source, true);
    assert!(off.status.success(), "dev checks are off by default");
    assert!(on.status.success(), "advisories do not reject valid definitions");
    let off_json = json_stdout(&off);
    let on_json = json_stdout(&on);
    assert_eq!(assert_digest(&off_json), assert_digest(&on_json));
    assert!(off_json["advisories"].as_array().unwrap().is_empty());
    let advisories = on_json["advisories"].as_array().unwrap();
    assert_eq!(advisories.len(), 1);
    assert_eq!(advisories[0]["code"], "ADVISORY_UNUSED_PREDICATE");
    assert_eq!(advisories[0]["anchor"], "predicate:orphan");

    let docs_text = source_text.replace(
        "  rows read when member = actor\n",
        "  rows read when member = actor\n  docs { summary \"orphan is mentioned in docs\"; visibility internal }\n",
    );
    assert_ne!(docs_text, source_text, "the metadata edit must target the fixture");
    let docs_source = write_source(&dir, "app-with-docs-mention.aip", &docs_text);
    let docs_output = check(&docs_source, true);
    assert!(docs_output.status.success());
    let docs_json = json_stdout(&docs_output);
    assert_eq!(assert_digest(&on_json), assert_digest(&docs_json));
    assert_eq!(docs_json["advisories"].as_array().unwrap().len(), 1);
    assert_eq!(docs_json["advisories"][0]["anchor"], "predicate:orphan");
}

#[test]
fn gen_emits_a_typed_binding_with_the_selected_wire_in_its_fingerprint() {
    let dir = TempDir::new();
    let source = write_source(&dir, "app.aip", &fixture());
    let safe_path = dir.path().join("safe.ts");
    let decimal_path = dir.path().join("decimal.ts");

    let safe = gen(&source, &safe_path, "safe", false);
    let decimal = gen(&source, &decimal_path, "decimal", false);
    assert!(safe.status.success(), "safe generation: {}", String::from_utf8_lossy(&safe.stderr));
    assert!(decimal.status.success(), "decimal generation: {}", String::from_utf8_lossy(&decimal.stderr));

    let safe_json = json_stdout(&safe);
    let decimal_json = json_stdout(&decimal);
    assert_eq!(safe_json["ok"], true);
    assert_eq!(decimal_json["ok"], true);
    let safe_fingerprint = safe_json["fingerprint"].as_str().expect("safe fingerprint");
    let decimal_fingerprint = decimal_json["fingerprint"].as_str().expect("decimal fingerprint");
    for fingerprint in [safe_fingerprint, decimal_fingerprint] {
        assert_eq!(fingerprint.len(), 64);
        assert!(fingerprint.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    }
    assert_ne!(safe_fingerprint, decimal_fingerprint, "the selected Id wire is public contract input");

    let safe_source = fs::read_to_string(safe_path).expect("safe generated binding");
    let decimal_source = fs::read_to_string(decimal_path).expect("decimal generated binding");
    for (generated, wire, fingerprint) in
        [(&safe_source, "safe-number-v13", safe_fingerprint), (&decimal_source, "decimal-string-v13", decimal_fingerprint)]
    {
        assert!(generated.contains("../sdk/index.ts"), "SDK import is explicit");
        assert!(generated.contains("PrototypeBinding"), "generated artifact binds the combined read/apply/WRITE prototype contract");
        assert!(generated.contains("readDescriptors:"), "generated read validation is present even without extensions");
        assert!(generated.contains("link: string"), "Url is exposed as a scalar string");
        assert!(generated.contains(wire), "artifact records its selected wire mode");
        assert!(generated.contains(fingerprint), "stdout and artifact share a fingerprint");
    }
}

#[test]
fn generation_failures_leave_no_output_artifact() {
    let dir = TempDir::new();
    let valid = write_source(&dir, "app.aip", &fixture());
    let invalid_text = fixture().replace("phase: Phase", "phase: MissingPhase");
    assert_ne!(invalid_text, fixture(), "invalid source mutation must match");
    let invalid = write_source(&dir, "invalid.aip", &invalid_text);

    let missing_wire = {
        let mut command = binary();
        command.arg("gen").arg(&valid).arg("--out").arg(dir.path().join("missing-wire.ts")).arg("--sdk-import").arg("../sdk/index.ts");
        command.output().expect("run without required wire")
    };
    assert!(!missing_wire.status.success());
    assert!(!dir.path().join("missing-wire.ts").exists());

    let unknown_wire_path = dir.path().join("unknown-wire.ts");
    let unknown_wire = gen(&valid, &unknown_wire_path, "legacy-ish", false);
    assert!(!unknown_wire.status.success());
    assert!(!unknown_wire_path.exists());

    let unknown_option_path = dir.path().join("unknown-option.ts");
    let mut command = binary();
    command
        .arg("gen")
        .arg(&valid)
        .arg("--wire")
        .arg("safe")
        .arg("--out")
        .arg(&unknown_option_path)
        .arg("--sdk-import")
        .arg("../sdk/index.ts")
        .arg("--not-a-real-option");
    let unknown_option = command.output().expect("run with unknown option");
    assert!(!unknown_option.status.success());
    assert!(!unknown_option_path.exists());

    let invalid_path = dir.path().join("invalid-source.ts");
    let invalid_source = gen(&invalid, &invalid_path, "safe", false);
    assert!(!invalid_source.status.success());
    assert!(!invalid_path.exists());
}

#[test]
fn gen_dev_checks_and_docs_only_edits_leave_the_binding_bytes_unchanged() {
    let dir = TempDir::new();
    let with_orphan = format!("{}\npredicate orphan(m: Member) = m.id = m.id\n", fixture());
    let docs_variant = with_orphan.replace(
        "  rows read when member = actor\n",
        "  rows read when member = actor\n  docs { summary \"metadata-only text\"; visibility internal }\n",
    );
    assert_ne!(with_orphan, docs_variant, "metadata variant must change the source");
    let original = write_source(&dir, "original.aip", &with_orphan);
    let docs = write_source(&dir, "docs.aip", &docs_variant);

    let paths = [("default.ts", &original, false), ("dev-checks.ts", &original, true), ("docs.ts", &docs, true)];
    let mut artifacts = Vec::new();
    for (name, source, dev_checks) in paths {
        let output_path = dir.path().join(name);
        let output = gen(source, &output_path, "decimal", dev_checks);
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        artifacts.push(fs::read(output_path).expect("generated artifact"));
    }
    assert_eq!(artifacts[0], artifacts[1], "optional advisories do not enter generated binding");
    assert_eq!(artifacts[1], artifacts[2], "docs metadata does not enter generated binding");
}

#[test]
fn generation_refuses_to_overwrite_the_source_or_a_hard_link_to_it() {
    let dir = TempDir::new();
    let original = fixture();
    let source = write_source(&dir, "app.aip", &original);
    let aliases = [source.clone(), dir.path().join("alias.ts")];
    fs::hard_link(&source, &aliases[1]).unwrap();
    for target in aliases {
        let output = gen(&source, &target, "safe", false);
        assert!(!output.status.success(), "source cannot be replaced with generated code");
        assert_eq!(fs::read_to_string(&source).unwrap(), original);
        assert_eq!(fs::read_to_string(&target).unwrap(), original);
    }
}

#[test]
fn invalid_definition_preserves_an_existing_binding() {
    let dir = TempDir::new();
    let invalid = write_source(&dir, "invalid.aip", &fixture().replace("phase: Phase", "phase: MissingPhase"));
    let target = dir.path().join("existing.ts");
    fs::write(&target, "original binding").unwrap();
    assert!(!gen(&invalid, &target, "decimal", false).status.success());
    assert_eq!(fs::read_to_string(target).unwrap(), "original binding");
}

#[test]
fn generated_binding_resolves_the_actual_sdk_and_checks_call_site_types() {
    let dir = TempDir::new();
    let source = write_source(&dir, "app.aip", &fixture());
    let out = dir.path().join("generated.ts");
    let sdk = Path::new(env!("CARGO_MANIFEST_DIR")).join("sdk/index.ts").display().to_string();
    let output = binary().arg("gen").arg(&source).args(["--wire", "decimal", "--out"]).arg(&out).arg("--sdk-import").arg(&sdk).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let sdk = serde_json::to_string(&sdk).unwrap();
    let call_site=format!("import {{connect}} from {sdk};\nimport {{contract}} from './generated.ts';\nconst aip=connect('http://unused','unused',contract);\naip.apply({{apply:'Event.mark',target:{{where:[{{field:'phase',op:'eq',value:'READY'}}]}}}});\n// @ts-expect-error undeclared Enum member\naip.apply({{apply:'Event.mark',target:{{where:[{{field:'phase',op:'eq',value:'UNKNOWN'}}]}}}});\n");
    let script = dir.path().join("use-sdk.ts");
    fs::write(&script, &call_site).unwrap();
    let tsc = Path::new(env!("CARGO_MANIFEST_DIR")).join("../spikes/spike-0-ts/node_modules/.bin/tsc");
    let compile = |script: &Path| {
        Command::new(&tsc)
            .args([
                "--noEmit",
                "--strict",
                "--target",
                "esnext",
                "--module",
                "nodenext",
                "--moduleResolution",
                "nodenext",
                "--allowImportingTsExtensions",
                "--skipLibCheck",
            ])
            .arg(script)
            .output()
            .unwrap()
    };
    let positive = compile(&script);
    assert!(positive.status.success(), "{}", String::from_utf8_lossy(&positive.stdout));
    fs::write(&script, call_site.replace("@ts-expect-error", "")).unwrap();
    let negative = compile(&script);
    assert!(!negative.status.success());
    assert!(String::from_utf8_lossy(&negative.stdout).contains("UNKNOWN"), "type error must identify the actual bad member");
}

#[test]
fn output_rename_failure_preserves_the_target_and_removes_owned_temporary_file() {
    let dir = TempDir::new();
    let source = write_source(&dir, "app.aip", &fixture());
    let target = dir.path().join("binding.ts");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("marker"), "preserve").unwrap();
    let output = gen(&source, &target, "safe", false);
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(target.join("marker")).unwrap(), "preserve");
    assert!(!fs::read_dir(dir.path()).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with(".aip-gen-")));
}
