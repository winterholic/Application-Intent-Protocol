//! One error-code system (founder decision 2026-10-13, audit F9): every code is `AREA.NAME`, the registries are
//! listed and explained through one surface, and no code of the old schemes is left in the tree.
//!
//! Areas: `DEF` (the product definition checker), `WIRE` (the product HTTP API), `CLIENT` (raised by the SDKs
//! themselves), `POC` (the PoC checker, Core IR validation and the PoC runtime).

use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn aip(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_aip")).args(args).output().expect("run aip")
}

fn listing() -> Vec<Value> {
    let out = aip(&["diagnostics", "--json", "--area", "all"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice::<Vec<Value>>(&out.stdout).expect("a JSON array of entries")
}

fn well_formed(code: &str) -> bool {
    let mut parts = code.split('.');
    let area = parts.next().unwrap_or("");
    let rest: Vec<&str> = parts.collect();
    ["DEF", "WIRE", "CLIENT", "POC"].contains(&area)
        && !rest.is_empty()
        && rest.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'))
}

#[test]
fn every_code_is_area_dot_name_and_unique() {
    let all = listing();
    let mut seen = BTreeSet::new();
    let mut areas = BTreeSet::new();
    for e in &all {
        let code = e["code"].as_str().expect("code");
        assert!(well_formed(code), "{code} is not AREA.NAME");
        assert!(seen.insert(code.to_string()), "{code} is registered twice");
        let area = e["area"].as_str().expect("area");
        assert_eq!(code.split('.').next().expect("area prefix").to_lowercase(), area, "{code}: the area field names the prefix");
        areas.insert(area.to_string());
    }
    assert_eq!(areas, ["client", "def", "poc", "wire"].map(String::from).into_iter().collect(), "one registry for all four areas");
    assert!(all.len() > 250, "the unified listing holds every registry: {}", all.len());
    // the four names that used to exist in two registries with two meanings are now told apart
    for name in ["UNKNOWN_FIELD", "UNKNOWN_KEY", "DUPLICATE", "BAD_VALUE"] {
        assert!(seen.contains(&format!("DEF.{name}")) && seen.contains(&format!("WIRE.{name}")), "{name}");
        assert!(!seen.contains(name), "{name} must not exist without an area");
    }
}

#[test]
fn every_listed_code_can_be_explained() {
    for e in listing() {
        let code = e["code"].as_str().expect("code");
        let out = aip(&["explain-code", code, "--json"]);
        assert!(out.status.success(), "{code}: {}", String::from_utf8_lossy(&out.stderr));
        let v: Value = serde_json::from_slice(&out.stdout).expect("json");
        assert_eq!(v["code"], code);
        assert!(v["fix"].as_str().is_some_and(|f| !f.is_empty()), "{code} says how to fix it");
        assert!(aip(&["explain-code", code]).status.success(), "{code}: text form");
    }
}

#[test]
fn a_code_of_an_old_scheme_is_pointed_to_its_new_name() {
    let text = |code: &str| {
        let out = aip(&["explain-code", code]);
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    };
    let (ok, t) = text("AIP-E103");
    assert!(ok && t.contains("renamed") && t.contains("POC.E103"), "{t}");
    let (ok, t) = text("AIP.AUTH.FORBIDDEN");
    assert!(ok && t.contains("renamed") && t.contains("POC.AUTH.FORBIDDEN"), "{t}");
    // a bare name that two areas used: both candidates are shown
    let (ok, t) = text("UNKNOWN_FIELD");
    assert!(ok && t.contains("DEF.UNKNOWN_FIELD") && t.contains("WIRE.UNKNOWN_FIELD"), "{t}");
    let (ok, t) = text("PROTOCOL_ERROR");
    assert!(ok && t.contains("renamed") && t.contains("CLIENT.PROTOCOL_ERROR"), "{t}");
    let (ok, t) = text("ACCESS_DENIED");
    assert!(ok && t.contains("renamed") && t.contains("WIRE.ACCESS_DENIED"), "{t}");
    let (ok, _) = text("NO_SUCH_CODE");
    assert!(!ok);
}

fn scan(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if p.is_dir() {
            if !["target", "node_modules", "dist", ".git", "agent-logs", "reviews", "sources"].contains(&name) {
                scan(&p, out);
            }
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| ["rs", "ts", "mjs", "py", "md", "json", "aip", "aipoc", "expect", "snap", "toml", "sh", "yml"].contains(&x))
        {
            out.push(p);
        }
    }
}

/// `AIP-E103`, `AIP-W603`, `AIP-I110` and the `AIP.<AREA>.<NAME>` runtime codes are gone. The only place that may
/// spell them is the code that maps an old code to its new name.
#[test]
fn no_code_of_the_old_schemes_is_left_in_the_tree() {
    let mut files = Vec::new();
    for d in ["crates", "spikes", "examples", "conformance", "spec", "docs", "product", "prototype", "tools", "scripts"] {
        scan(&root().join(d), &mut files);
    }
    let mut offenders = Vec::new();
    for f in files {
        if ["crates/aip-cli/tests/error_codes.rs", "crates/aip-cli/src/registry.rs", "docs/DECISIONS.md"].iter().any(|s| f.ends_with(s)) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        for (i, line) in text.lines().enumerate() {
            let b = line.as_bytes();
            let hit = line.match_indices("AIP-").any(|(at, _)| {
                matches!(b.get(at + 4), Some(b'E' | b'W' | b'I')) && b.get(at + 5..at + 8).is_some_and(|d| d.iter().all(u8::is_ascii_digit))
            }) || line.match_indices("AIP.").any(|(at, _)| {
                b.get(at + 4).is_some_and(u8::is_ascii_uppercase)
                    && line[at + 4..]
                        .split(|c: char| !(c.is_ascii_uppercase() || c == '_' || c == '.'))
                        .next()
                        .is_some_and(|w| w.contains('.') || w == "INTERNAL" || w == "NOT_FOUND" || w == "UNAVAILABLE")
            });
            if hit {
                offenders.push(format!("{}:{}: {}", f.strip_prefix(root()).unwrap_or(&f).display(), i + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "{} lines still use an old code scheme:\n{}",
        offenders.len(),
        offenders.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
    );
}
