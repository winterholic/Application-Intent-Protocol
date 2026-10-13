//! Two languages, two extensions (founder decision 2026-10-13, audit F5): `.aip` is the product definition language
//! (`resource`, `expose`, `transition`; checked by `aip service check`), `.aipoc` is the PoC language (`entity`,
//! `command`, `query`; checked by `aip check`). Each checker names the other one when it is given its file.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn aip(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aip")).args(args).output().expect("run aip")
}

fn product_file() -> String {
    root().join("spikes/spike-v1-fixture/fixture/recruitment.aip").to_string_lossy().into_owned()
}

fn poc_file() -> String {
    root().join("examples/shop/app.aipoc").to_string_lossy().into_owned()
}

fn temp_file(name: &str, text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aip-ext-{}-{}", std::process::id(), name.replace('.', "_")));
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let file = dir.join(name);
    std::fs::write(&file, text).expect("write");
    file
}

#[test]
fn poc_commands_name_the_product_extension() {
    let out = aip(&["check", &product_file(), "--json"]);
    assert!(!out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).expect("a JSON array of diagnostics");
    assert_eq!(v.as_array().map(Vec::len), Some(1), "one diagnostic, no typos reported inside a file of the other language: {v}");
    assert_eq!(v[0]["code"], "POC.E001");
    assert!(v[0]["message"].as_str().is_some_and(|m| m.contains(".aip") && m.contains(".aipoc")), "{v}");
    assert!(v[0]["help"].as_str().is_some_and(|h| h.contains("aip service check")), "{v}");
    for command in ["parse", "ddl", "ir", "core", "contract"] {
        let out = aip(&[command, &product_file()]);
        assert!(!out.status.success(), "{command}");
        let text = String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("POC.E001"), "{command}: {text}");
    }
}

#[test]
fn poc_commands_want_the_aipoc_extension() {
    let source = std::fs::read_to_string(poc_file()).unwrap_or_default();
    let file = temp_file("app.txt", &source);
    let out = aip(&["check", &file.to_string_lossy(), "--json"]);
    std::fs::remove_dir_all(file.parent().expect("dir")).ok();
    assert!(!out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v[0]["code"], "POC.E001");
    assert!(v[0]["help"].as_str().is_some_and(|h| h.contains("rename")), "{v}");
}

#[test]
fn a_poc_program_in_an_aipoc_file_is_checked() {
    let out = aip(&["check", &poc_file()]);
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}

#[test]
fn service_commands_name_the_poc_extension() {
    let out = aip(&["service", "check", &poc_file()]);
    assert!(!out.status.success());
    let v: Value = serde_json::from_slice(&out.stderr).expect("a failed service command prints its JSON error on stderr");
    assert_eq!(v["code"], "DEF.INVALID_DEFINITION", "{v}");
    assert_eq!(v["diagnostics"][0]["code"], "DEF.WRONG_LANGUAGE", "{v}");
    assert!(v["diagnostics"][0]["help"].as_str().is_some_and(|h| h.contains("aip check")), "{v}");
    let out = aip(&["service", "check", &product_file()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

fn files(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if p.is_dir() {
            if !["target", "node_modules", "dist", ".git"].contains(&name) {
                files(&p, extension, out);
            }
        } else if p.extension().and_then(|x| x.to_str()) == Some(extension) {
            out.push(p);
        }
    }
}

/// Which language a file is written in, by the declaration keywords at the start of a line (`expose` exists in both).
fn keywords(path: &Path) -> (bool, bool) {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let line_starts = |words: &[&str]| text.lines().any(|l| words.iter().any(|w| l.starts_with(&format!("{w} "))));
    (line_starts(&["entity", "command", "query"]), line_starts(&["resource"]))
}

#[test]
fn each_extension_holds_one_language() {
    let (mut product, mut poc) = (Vec::new(), Vec::new());
    files(&root(), "aip", &mut product);
    files(&root(), "aipoc", &mut poc);
    assert!(product.len() >= 10, "the product definitions keep `.aip`: {}", product.len());
    assert!(poc.len() >= 100, "the PoC programs and conformance cases use `.aipoc`: {}", poc.len());
    for p in &product {
        assert!(!keywords(p).0, "{} has the product extension but PoC declarations", p.display());
    }
    for p in &poc {
        assert!(!keywords(p).1, "{} has the PoC extension but product declarations", p.display());
    }
}

#[test]
fn check_docs_reads_aipoc_blocks_and_leaves_product_blocks_to_their_own_checks() {
    let md = temp_file("doc.md", "```aipoc\nentity Club { name: Text }\n```\n\n```aip\nresource Club { fields { id: Id } }\n```\n");
    let out = aip(&["check-docs", &md.to_string_lossy()]);
    let ok = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success() && ok.contains("1 blocks: 1 parsed"), "{ok}");
    std::fs::write(&md, "```aipoc\nentity { name: Text }\n```\n").expect("write");
    let out = aip(&["check-docs", &md.to_string_lossy()]);
    std::fs::remove_dir_all(md.parent().expect("dir")).ok();
    assert!(!out.status.success(), "a broken PoC block fails");
}
