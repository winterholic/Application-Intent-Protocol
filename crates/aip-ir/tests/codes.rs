//! The registry in `src/codes.rs` is the only list of diagnostic and error
//! codes. These tests keep it complete (every code the sources can produce is
//! registered), well-formed and free of dead entries.

use aip_ir::codes::{self, CodeInfo, Kind};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Every `.rs` file under `crates/*/src` and `crates/*/tests`, except this test (its fixtures use fake codes).
fn sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, String)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("aip-ir/tests/codes.rs") {
                out.push((p.clone(), std::fs::read_to_string(&p).expect("read source")));
            }
        }
    }
    let mut out = Vec::new();
    let mut crates: Vec<PathBuf> = std::fs::read_dir(crates_dir()).expect("crates dir").filter_map(|e| e.ok().map(|e| e.path())).collect();
    crates.sort();
    for c in crates {
        walk(&c.join("src"), &mut out);
        walk(&c.join("tests"), &mut out);
    }
    out
}

/// `POC.[EWI][0-9]{3}` and `POC.<GROUP>.<NAME>` occurrences in a text. The scan is textual on
/// purpose: a code in a comment, a message or a test is still a code readers will see.
fn scan(text: &str) -> BTreeSet<String> {
    let b = text.as_bytes();
    let mut found = BTreeSet::new();
    let mut i = 0;
    while i + 4 <= b.len() {
        let word_start = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        if word_start && &b[i..i + 4] == b"POC." {
            let mut j = i + 4;
            while j < b.len() && (b[j].is_ascii_uppercase() || b[j].is_ascii_digit() || b[j] == b'_' || b[j] == b'.') {
                j += 1;
            }
            let code = text[i..j].trim_end_matches('.');
            let name = code.get(4..).unwrap_or("");
            let diag = name.len() == 4 && matches!(name.as_bytes()[0], b'E' | b'W' | b'I') && name[1..].bytes().all(|d| d.is_ascii_digit());
            // a name with digits that is not a diagnostic number (`POC.I1234`) is not a code
            if name.len() > 1 && (diag || !name.bytes().any(|d| d.is_ascii_digit())) {
                found.insert(code.to_string());
            }
            i = j.max(i + 4);
        } else {
            i += 1;
        }
    }
    found
}

fn unregistered(found: &BTreeSet<String>, registry: &[CodeInfo]) -> Vec<String> {
    found.iter().filter(|c| !registry.iter().any(|r| r.code == c.as_str())).cloned().collect()
}

fn all_found() -> BTreeSet<String> {
    sources().iter().flat_map(|(_, t)| scan(t)).collect()
}

#[test]
fn every_code_in_the_sources_is_registered() {
    let found = all_found();
    assert!(found.len() > 50, "the scan found only {} codes; is it looking at the right files?", found.len());
    let missing = unregistered(&found, codes::all());
    assert!(missing.is_empty(), "codes used in sources but missing from crates/aip-ir/src/codes.rs: {missing:?}");
}

#[test]
fn removing_a_code_from_the_registry_is_detected() {
    // negative control: the completeness check must fail when an entry is missing
    let found = all_found();
    let reduced: Vec<CodeInfo> = codes::all().iter().filter(|c| c.code != codes::E103).copied().collect();
    assert_eq!(unregistered(&found, &reduced), vec![codes::E103.to_string()]);
}

#[test]
fn registry_codes_are_unique() {
    let mut seen = BTreeSet::new();
    for c in codes::all() {
        assert!(seen.insert(c.code), "{} is registered twice", c.code);
    }
}

#[test]
fn entries_are_well_formed() {
    for c in codes::all() {
        let name = c.code.strip_prefix("POC.").unwrap_or_else(|| panic!("{} is not in the POC area", c.code));
        let diag = |letter: char| name.len() == 4 && name.starts_with(letter) && name[1..].bytes().all(|d| d.is_ascii_digit());
        let prefix_ok = match c.kind {
            Kind::Error => diag('E'),
            Kind::Warning => diag('W'),
            Kind::Ir => diag('I'),
            Kind::Runtime => !name.bytes().any(|d| d.is_ascii_digit()),
        };
        assert!(prefix_ok, "{} does not match its kind {:?}", c.code, c.kind);
        assert!(!c.title.is_empty() && !c.explain.is_empty() && !c.fix.is_empty(), "{} needs title, explain and fix", c.code);
        assert!(!c.title.contains('\n'), "{} title must be one line", c.code);
        let runtime = c.kind == Kind::Runtime;
        assert_eq!(c.http_status.is_some(), runtime, "{}: http_status only for runtime codes", c.code);
        assert_eq!(c.retryable.is_some(), runtime, "{}: retryable only for runtime codes", c.code);
        if let Some(s) = c.http_status {
            assert!((400..600).contains(&s), "{}: status {s}", c.code);
        }
        assert_eq!(c.kind == Kind::Warning, c.severity == codes::Severity::Warning, "{}: severity follows the W prefix", c.code);
    }
}

#[test]
fn every_registered_code_is_used_outside_the_registry() {
    // a code nothing can emit is a dead entry; deprecated codes are exempt
    let srcs = sources();
    let mut unused = Vec::new();
    for c in codes::all().iter().filter(|c| !c.deprecated) {
        let name = c.code.trim_start_matches("POC.").replace('.', "_");
        let used = srcs.iter().filter(|(p, _)| !p.ends_with("aip-ir/src/codes.rs")).any(|(_, t)| {
            t.contains(&format!("codes::{name}"))
                || t.contains(&format!("\"{}\"", c.code))
                || t.contains(&format!("`{}`", c.code))
                || t.contains(&format!(" {} ", c.code))
        });
        if !used {
            unused.push(c.code);
        }
    }
    assert!(unused.is_empty(), "registered but never referenced outside the registry: {unused:?}");
}

#[test]
fn http_status_and_retry_follow_the_registry() {
    assert_eq!(codes::http_status(codes::NOT_FOUND), 404);
    assert_eq!(codes::http_status(codes::CONFLICT_STALE_VERSION), 409);
    assert_eq!(codes::http_status("POC.NOT.IN.REGISTRY"), 409, "unknown codes keep the runtime's historical default");
    assert!(codes::retryable(codes::CONCURRENCY_CONFLICT));
    assert!(!codes::retryable(codes::CONFLICT_STALE_VERSION));
    assert!(!codes::retryable("POC.NOT.IN.REGISTRY"));
    assert_eq!(codes::http_status(codes::UNAVAILABLE), 503, "a transient dependency failure is a 503, not a conflict");
    assert!(codes::retryable(codes::UNAVAILABLE));
}

#[test]
fn scanner_reads_both_shapes() {
    let found = scan("x \"POC.E103\" `POC.CONFLICT.UNIQUE`. POC.NOT_FOUND, POC.I1234 NOTPOC.E101 POC.X101");
    let want: BTreeSet<String> = ["POC.E103", "POC.CONFLICT.UNIQUE", "POC.NOT_FOUND"].iter().map(|s| s.to_string()).collect();
    assert_eq!(found, want);
}

/// Emission sites per code: `NAME file=count ...`, one line per constant, from `codes::NAME` uses in `crates/*/src`.
fn emission_sites() -> String {
    let mut per: std::collections::BTreeMap<String, std::collections::BTreeMap<String, usize>> = Default::default();
    for (path, text) in sources() {
        let p = path.to_string_lossy().replace('\\', "/");
        if !p.contains("/src/") || p.ends_with("aip-ir/src/codes.rs") {
            continue;
        }
        let file = p.rsplit("/src/").next().unwrap_or_default().to_string();
        let mut rest = text.as_str();
        while let Some(i) = rest.find("codes::") {
            let after = &rest[i + 7..];
            let end = after.find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')).unwrap_or(after.len());
            if end > 0 {
                *per.entry(after[..end].to_string()).or_default().entry(file.clone()).or_default() += 1;
            }
            rest = after;
        }
    }
    let mut out = String::new();
    for (name, files) in per {
        let sites: Vec<String> = files.iter().map(|(f, n)| format!("{f}={n}")).collect();
        out.push_str(&format!("{name} {}\n", sites.join(" ")));
    }
    out
}

/// A code means one thing, which no machine can check; what a test can do is make every new emission
/// site visible. Adding a `codes::X` use changes this snapshot, so the author has to confirm that the
/// new site has exactly the meaning and the fix the registry entry states, or allocate a new code.
#[test]
fn emission_sites_per_code_are_pinned() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/code_sites.snap");
    let now = emission_sites();
    if std::env::var("AIP_UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &now).expect("write snapshot");
        return;
    }
    let pinned = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        pinned == now,
        "emission sites changed. Check each new site against the registry entry of its code (same meaning, same fix), \
         use a new code otherwise, then regenerate with AIP_UPDATE_GOLDEN=1 cargo test -p aip-ir --test codes.\n--- pinned\n{pinned}--- now\n{now}"
    );
}
