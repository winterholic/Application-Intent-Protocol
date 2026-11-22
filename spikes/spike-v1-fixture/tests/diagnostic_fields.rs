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

#[test]
fn an_access_to_a_transition_that_is_not_exposed_has_its_own_code() {
    let def = |expose: &str| {
        format!(
            "actor Member\nresource Member {{ fields {{ id: Id }} }}\nresource Event {{\n  fields {{ id: Id; member: Member; checked: Bool }}\n  rows read when member = actor\n  transition mark {{ allow member = actor; from checked = false; to checked = true }}\n  {expose}\n  extension write run {{\n    input {{ id: Event.Id }}\n    output {{ id: Event.Id; count: Int }}\n    access Event.mark\n    effect db\n    deadline 2s\n    implementation \"write.run\"\n  }}\n}}\n"
        )
    };
    assert!(load_str(&def("expose apply mark { target id; bulk maxRows 2 }"), Form::A).is_ok());
    let ds = failure(&def(""), Form::A);
    assert!(ds.iter().any(|d| d["code"] == "DEF.ACCESS_TARGET_NOT_EXPOSED"), "{ds:?}");
    assert!(ds.iter().all(|d| d["code"] != "DEF.UNKNOWN_SYMBOL"), "{ds:?}");
}

fn only(ds: &[Value], code: &str) -> Value {
    ds.iter().find(|d| d["code"] == code).cloned().unwrap_or_else(|| panic!("{code} not in {ds:?}"))
}

fn at(d: &Value) -> (Option<u64>, Option<u64>, &Value) {
    (d["line"].as_u64(), d["column"].as_u64(), &d["located"])
}

#[test]
fn a_source_over_the_size_limit_points_at_the_byte_that_crosses_it() {
    let limit = 1024 * 1024;
    let ds = failure(&format!("{}\n{}", "a".repeat(limit - 10), "b".repeat(100)), Form::A);
    let d = only(&ds, "DEF.SOURCE_TOO_LARGE");
    // The first line holds limit-10 bytes plus the newline; the limit is crossed 10 characters into the second line.
    assert_eq!(at(&d), (Some(2), Some(10), &Value::Bool(true)), "{d}");
    assert_eq!(d["anchor"], "line:2:col:10");
    // Multibyte characters are counted as characters, and the crossing character is the one that does not fit.
    let ds = failure(&"가".repeat(limit / 3 + 2), Form::A);
    let d = only(&ds, "DEF.SOURCE_TOO_LARGE");
    assert_eq!(at(&d), (Some(1), Some(limit as u64 / 3 + 1), &Value::Bool(true)), "{d}");
}

#[test]
fn a_second_block_in_an_embedded_file_points_at_the_second_block() {
    let two = format!(
        "{E_TS}\n{}",
        E_TS.replacen("import { aip } from \"@aip/define\"\n", "", 1).replacen("export default aip", "export const second = aip", 1)
    );
    let line = (E_TS.lines().count() + 2) as u64 + 1;
    let ds = failure(&two, Form::ETs);
    let d = only(&ds, "DEF.MULTIPLE_BLOCKS");
    assert_eq!(d["located"], true, "{d}");
    assert!(d["line"].as_u64().is_some_and(|l| l >= line), "the second block starts after line {line}: {d}");
    let none = failure("import { aip } from \"@aip/define\"\n", Form::ETs);
    assert_eq!(only(&none, "DEF.NO_BLOCK")["located"], false, "there is nothing to point at when no block exists");
}

#[test]
fn a_second_define_in_a_host_literal_file_points_at_the_second_define() {
    let two = format!("{H_TS}\nexport const again = define({{}})\n");
    let ds = failure(&two, Form::HTs);
    let d = only(&ds, "DEF.MULTIPLE_BLOCKS");
    assert_eq!(d["located"], true, "{d}");
    // The fixture ends with a newline, then the blank line of the format string, then the new statement.
    assert_eq!((d["line"].as_u64(), d["column"].as_u64()), (Some(H_TS.lines().count() as u64 + 2), Some(22)), "{d}");
}

#[test]
fn a_block_without_its_official_import_points_at_the_first_use() {
    let missing = E_TS.replacen("import { aip } from \"@aip/define\"\n", "", 1);
    let ds = failure(&missing, Form::ETs);
    let d = only(&ds, "DEF.WRONG_BINDING");
    assert_eq!(at(&d), (Some(2), Some(16), &Value::Bool(true)), "{d}");
}

#[test]
fn a_diagnostic_about_the_whole_definition_stays_without_a_position() {
    // Nothing in the text is wrong: the definition lacks a part. The 0:0 is deliberate (decision D-E3 keeps it open).
    for (src, form, code) in [("", Form::A, "DEF.MISSING_ITEM"), ("", Form::ETs, "DEF.NO_BLOCK"), ("", Form::HTs, "DEF.NO_BLOCK")] {
        let ds = failure(src, form);
        assert_eq!(only(&ds, code)["located"], false, "{code}");
    }
}

#[test]
fn a_bad_literal_points_at_the_literal() {
    let src = "actor Member\nresource Member {\n  fields { id: Id; email: Email; n: Int }\n  rows read when email = \"not-an-email\"\n}\nresource Note {\n  fields { id: Id; owner: Member; d: Date }\n  rows read when d = \"2024-99-99\"\n}\n";
    let ds = failure(src, Form::A);
    let bad: Vec<&Value> = ds.iter().filter(|d| d["code"] == "DEF.BAD_VALUE").collect();
    assert_eq!(bad.len(), 2, "{ds:?}");
    // The literal, not the start of the definition: `  rows read when email = ` is 25 characters, `  rows read when d = ` is 21.
    assert_eq!((bad[0]["line"].as_u64(), bad[0]["column"].as_u64(), &bad[0]["located"]), (Some(4), Some(26), &Value::Bool(true)), "{}", bad[0]);
    assert_eq!((bad[1]["line"].as_u64(), bad[1]["column"].as_u64(), &bad[1]["located"]), (Some(8), Some(22), &Value::Bool(true)), "{}", bad[1]);
    // A condition that is not a Bool: the literal 5 is where the error is.
    let ds = failure("actor Member\nresource Member {\n  fields { id: Id }\n  rows read when 5\n}\n", Form::A);
    let d = only(&ds, "DEF.TYPE_MISMATCH");
    assert_eq!((d["line"].as_u64(), d["column"].as_u64()), (Some(4), Some(18)), "{d}");
}
