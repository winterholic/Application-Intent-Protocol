//! Runs `conformance/sema/*.aip`: each case is appended to `_prelude.aip` and
//! must produce exactly the diagnostic codes listed in its `.expect` file.
//!
//! Frontend rules (names, types, syntax) are checked on the AST. Semantic rules
//! are checked on Core IR: `ir_json_alone_yields_the_semantic_codes` takes the IR
//! of every such case through JSON and runs only `validate` and `analyze`, the
//! checks a TS or Python frontend would get.

use aip_ir::analyze::CODES as SEMANTIC;
use std::collections::BTreeSet;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../conformance/sema")
}

struct Case {
    name: String,
    src: String,
    expected: BTreeSet<String>,
}

fn cases() -> Vec<Case> {
    let dir = root();
    let prelude = std::fs::read_to_string(dir.join("_prelude.aip")).expect("prelude");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("conformance dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "aip") && !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('_')))
        .collect();
    paths.sort();
    assert!(!paths.is_empty());
    paths
        .iter()
        .map(|case| {
            let body = std::fs::read_to_string(case).expect("case");
            let expected =
                std::fs::read_to_string(case.with_extension("expect")).expect("expect file").split_whitespace().map(String::from).collect();
            Case {
                name: case.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                // a case that must not have an actor starts with `// no prelude`
                src: if body.starts_with("// no prelude") { body } else { format!("{prelude}\n{body}") },
                expected,
            }
        })
        .collect()
}

#[test]
fn sema_conformance() {
    let cases = cases();
    let mut failures = Vec::new();
    for c in &cases {
        let checked = aip_sema::pipeline::check_source(&c.src);
        let got: BTreeSet<String> = checked.diagnostics.iter().map(|d| d.code.clone()).collect();
        if got != c.expected {
            let detail = checked.diagnostics.iter().map(|d| format!("    {}", d.render("case"))).collect::<Vec<_>>().join("\n");
            failures.push(format!("{}: expected {:?}, got {got:?}\n{detail}", c.name, c.expected));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases failed:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

#[test]
fn ir_json_alone_yields_the_semantic_codes() {
    let mut exercised = BTreeSet::new();
    let mut failures = Vec::new();
    for c in cases() {
        // a case the frontend lets through to the IR is a semantic case (or a clean one); frontend errors stop lowering
        let checked = aip_sema::pipeline::check_source(&c.src);
        let Some((core, _)) = checked.lowered else { continue };
        // what another frontend would hand over: the IR as JSON text, nothing of the source
        let text = serde_json::to_string(&core).expect("IR serializes");
        let back: aip_ir::Program = serde_json::from_str(&text).expect("IR deserializes");
        let mut diags = aip_ir::validate::validate(&back);
        diags.extend(aip_ir::analyze::analyze(&back));
        let got: BTreeSet<String> = diags.iter().map(|d| d.code.clone()).collect();
        if got != c.expected {
            failures.push(format!("{}: from the IR alone expected {:?}, got {got:?}", c.name, c.expected));
        }
        exercised.extend(c.expected.iter().filter(|x| SEMANTIC.contains(&x.as_str())).cloned());
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    let unexercised: Vec<&&str> = SEMANTIC.iter().filter(|c| !exercised.contains(**c)).collect();
    assert!(unexercised.is_empty(), "semantic codes without a conformance case: {unexercised:?}");
}

#[test]
fn ariari_is_clean() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/ariari/app.aip");
    let src = std::fs::read_to_string(path).expect("ariari");
    let checked = aip_sema::pipeline::check_source(&src);
    let rendered: Vec<String> = checked.diagnostics.iter().map(|d| d.render("ariari")).collect();
    assert!(rendered.is_empty(), "{}", rendered.join("\n"));
}
