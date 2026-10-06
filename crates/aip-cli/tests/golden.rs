//! Behaviour-preservation golden: the compiled `aip_plan::Program` (plus the
//! backend diagnostics) of every example and every error-free conformance case
//! must stay byte-identical while the backend's input changes.
//! Regenerate on purpose with `AIP_UPDATE_GOLDEN=1 cargo test -p aip-cli --test golden`.

mod common;

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn golden_dir() -> PathBuf {
    root().join("crates/aip-pg/tests/golden")
}

/// `(golden name, source)` of every input that passes the frontend without errors.
fn inputs() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for app in ["ariari", "shop", "saas", "cms"] {
        let src = std::fs::read_to_string(root().join(format!("examples/{app}/app.aip"))).expect("example source");
        out.push((app.to_string(), src));
    }
    let dir = root().join("conformance/sema");
    let prelude = std::fs::read_to_string(dir.join("_prelude.aip")).expect("prelude");
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("conformance dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "aip") && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('_')))
        .collect();
    cases.sort();
    for case in cases {
        let body = std::fs::read_to_string(&case).expect("case");
        let stem = case.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        // a case that must not have an actor starts with `// no prelude`
        let src = if body.starts_with("// no prelude") { body } else { format!("{prelude}\n{body}") };
        out.push((format!("sema_{stem}"), src));
    }
    out
}

fn read_golden(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

#[test]
fn compiled_plans_match_golden() {
    let update = std::env::var("AIP_UPDATE_GOLDEN").is_ok_and(|v| v == "1");
    let dir = golden_dir();
    let mut checked = 0;
    let mut failures = Vec::new();
    for (name, src) in inputs() {
        let Some(compiled) = common::compile_source(&src) else { continue };
        let json = serde_json::to_string_pretty(&compiled).expect("serialize") + "\n";
        let path = dir.join(format!("{name}.plan.json"));
        if update {
            std::fs::create_dir_all(&dir).expect("golden dir");
            std::fs::write(&path, &json).expect("write golden");
        } else if read_golden(&path).as_deref() != Some(json.as_str()) {
            failures.push(name);
        }
        checked += 1;
    }
    assert!(checked >= 2, "expected at least the two examples, compiled {checked}");
    assert!(failures.is_empty(), "plans differ from golden (AIP_UPDATE_GOLDEN=1 regenerates): {failures:?}");
}
