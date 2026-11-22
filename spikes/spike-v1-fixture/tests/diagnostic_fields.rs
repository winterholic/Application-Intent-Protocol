//! The machine fields of a definition diagnostic: what a tool may branch on (`code`, `severity`, the position, `anchor`,
//! `scopePath`) and what is only for people (`message`, `help`, `scope`). These tests need no database.
//!
//! The old fields keep their meaning; everything added here is optional, so a reader of the old fields only must still
//! work (`a_reader_of_the_old_fields_only_still_reads_every_diagnostic`).

use serde_json::Value;
use spike_v1_fixture::{codes, load_str, Form};
use std::collections::BTreeSet;

const A: &str = include_str!("../fixture/recruitment.aip");
const E_TS: &str = include_str!("../fixture/recruitment.e.ts");
const H_TS: &str = include_str!("../fixture/recruitment.h.ts");

fn failure(src: &str, form: Form) -> Vec<Value> {
    match load_str(src, form) {
        Ok(_) => panic!("the sample was accepted"),
        Err(ds) => ds.iter().map(|d| d.json()).collect(),
    }
}

fn unknown_key() -> Vec<Value> {
    failure(&A.replacen("sort periodEnd, views, id", "sorts periodEnd, views, id", 1), Form::A)
}

/// A reader written against the output before the machine fields existed: the five fields it always had, with
/// their types, and the three optional strings. Anything else is none of its business.
struct OldReader {
    code: String,
    severity: String,
    message: String,
    line: u64,
    column: u64,
}

impl OldReader {
    fn read(d: &Value) -> Result<OldReader, String> {
        let text = |k: &str| d[k].as_str().map(str::to_owned).ok_or(format!("{k} is not a string: {d}"));
        let int = |k: &str| d[k].as_u64().ok_or(format!("{k} is not an unsigned integer: {d}"));
        for k in ["scope", "help", "explain"] {
            if d.get(k).is_some_and(|v| !v.is_string()) {
                return Err(format!("{k} must stay a string: {d}"));
            }
        }
        Ok(OldReader { code: text("code")?, severity: text("severity")?, message: text("message")?, line: int("line")?, column: int("column")? })
    }
}

#[test]
fn a_reader_of_the_old_fields_only_still_reads_every_diagnostic() {
    let mut all = unknown_key();
    all.extend(failure("", Form::A));
    all.extend(failure("", Form::ETs));
    all.extend(failure(&"x".repeat(1024 * 1024 + 1), Form::A));
    all.extend(failure(&H_TS.replacen("rows: 50,", "rows: \"50\",", 1), Form::HTs));
    all.extend(failure(&E_TS.replacen("sort periodEnd, views, id", "sorts periodEnd, views, id", 1), Form::ETs));
    assert!(all.len() >= 6, "{all:?}");
    for d in &all {
        let old = OldReader::read(d).unwrap_or_else(|e| panic!("{e}"));
        assert!(old.code.starts_with("DEF."), "{}", old.code);
        assert_eq!(old.severity, "error");
        assert!(!old.message.is_empty());
        // The historical coordinates are untouched: a diagnostic without a position is still 0:0 for this reader.
        if d["located"] == false {
            assert_eq!((old.line, old.column), (0, 0), "{d}");
        } else {
            assert!(old.line >= 1 && old.column >= 1, "{d}");
        }
    }
}

#[test]
fn a_diagnostic_without_a_position_says_so_and_keeps_the_zero_coordinates() {
    for (src, form, code) in [("", Form::A, "DEF.MISSING_ITEM"), ("", Form::ETs, "DEF.NO_BLOCK")] {
        let ds = failure(src, form);
        let d = ds.iter().find(|d| d["code"] == code).unwrap_or_else(|| panic!("{code} not in {ds:?}"));
        assert_eq!((d["line"].as_u64(), d["column"].as_u64()), (Some(0), Some(0)), "{d}");
        assert_eq!(d["located"], false, "{d}");
        assert!(d.get("anchor").is_none(), "an unlocated diagnostic has no anchor: {d}");
    }
    let too_large = failure(&"x".repeat(1024 * 1024 + 1), Form::A);
    assert_eq!(too_large[0]["code"], "DEF.SOURCE_TOO_LARGE");
    assert_eq!(too_large[0]["located"], false, "{}", too_large[0]);
}

#[test]
fn a_positioned_diagnostic_has_a_line_anchor_that_repeats_its_coordinates() {
    let ds = unknown_key();
    let d = &ds[0];
    assert_eq!((d["line"].as_u64(), d["column"].as_u64()), (Some(56), Some(5)));
    assert_eq!(d["located"], true, "{d}");
    assert_eq!(d["anchor"], "line:56:col:5", "{d}");
}

#[test]
fn the_scope_path_is_the_scope_cut_into_its_segments() {
    let ds = unknown_key();
    let d = &ds[0];
    assert_eq!(d["scope"], "resource Recruitment > expose read");
    assert_eq!(d["scopePath"], serde_json::json!(["resource Recruitment", "expose read"]), "{d}");
    let joined = d["scopePath"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect::<Vec<_>>().join(" > ");
    assert_eq!(d["scope"], joined);
}

#[test]
fn a_diagnostic_without_a_scope_has_neither_scope_nor_scope_path() {
    let ds = failure(&H_TS.replacen("rows: 50,", "rows: \"50\",", 1), Form::HTs);
    let d = &ds[0];
    assert!(d.get("scope").is_none() && d.get("scopePath").is_none(), "{d}");
    assert_eq!(d["located"], true, "the host literal form still has a position: {d}");
}

#[test]
fn the_machine_fields_do_not_depend_on_the_wording_of_the_message() {
    // `message` and `help` may be reworded; what a tool branches on is the same for any wording.
    let ds = unknown_key();
    let mut reworded = ds[0].clone();
    reworded["message"] = Value::from("reworded");
    reworded["help"] = Value::from("reworded");
    let machine = |d: &Value| ["code", "severity", "line", "column", "located", "anchor", "scopePath"].map(|k| d[k].clone());
    assert_eq!(machine(&ds[0]), machine(&reworded));
    assert!(machine(&ds[0]).iter().all(|v| !v.is_null()), "{:?}", machine(&ds[0]));
}

#[test]
fn every_registered_code_has_a_severity_and_a_status() {
    let mut advisories = 0;
    for c in codes::all() {
        let entry = c.json();
        let want = if c.code.starts_with("DEF.ADVISORY_") { "warning" } else { "error" };
        assert_eq!(entry["severity"], want, "{}", c.code);
        assert_eq!(entry["status"], "active", "{}", c.code);
        // `since` and `replacedBy` are present only when they are known.
        assert_eq!(entry.get("since").is_some(), c.since.is_some(), "{}", c.code);
        assert_eq!(entry.get("replacedBy").is_some(), c.replaced_by.is_some(), "{}", c.code);
        advisories += usize::from(want == "warning");
    }
    assert!(advisories >= 9, "the advisory codes are warnings: {advisories}");
}

#[test]
fn the_registry_entry_keeps_the_keys_the_listing_always_had() {
    let entry = codes::lookup("DEF.UNKNOWN_FIELD").unwrap().json();
    for k in ["code", "title", "explain", "fix", "example"] {
        assert!(entry[k].is_string(), "{k}: {entry}");
    }
}

#[test]
fn a_published_code_is_never_removed_from_the_registry() {
    let published = include_str!("published_codes.txt");
    let registered: BTreeSet<&str> = codes::all().iter().map(|c| c.code).collect();
    let removed: Vec<&str> = published.lines().map(str::trim).filter(|l| !l.is_empty() && !registered.contains(l)).collect();
    assert!(
        removed.is_empty(),
        "a code that was published is gone from src/codes.rs (retire it with `status: retired`, never delete it): {removed:?}"
    );
}
