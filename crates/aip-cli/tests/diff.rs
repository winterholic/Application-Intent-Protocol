//! `aip diff`: the public contract of two versions of a program, one test per rule, and the examples against themselves.

use aip_contract::compat::{Level, breaks, compare};
use std::process::Command;

fn core(src: &str) -> aip_ir::Program {
    let checked = aip_sema::pipeline::check_source(src);
    assert!(!checked.has_errors(), "{:?}\n{src}", checked.diagnostics);
    checked.into_core().expect("no errors").0
}

const BASE: &str = r#"
use postgres

enum Level { LOW HIGH }

entity Item {
  name: Text(1..30)
  note: Text?
  level: Level = LOW
  qty: Int(0..100) = 0
}

query Items() {
  allow public
  from Item i
  sort by i.name asc
  page 20 by keyset
  select { id name note level }
}

command AddItem(name: Text(1..30), level: Level) {
  allow public
  do { insert Item { name, level } }
}

command Noop(n: Int, label: Text?) {
  allow public
  do { insert Item { name: "x" } }
}

command Retag(item: Item, name: Text(1..30)) idempotent {
  allow public
  do { set item.name = name }
  returns item { id name }
}
"#;

/// `(level, rule)` of everything that differs between BASE and BASE edited by `edit`.
fn changes(edit: impl Fn(&str) -> String) -> Vec<(Level, &'static str)> {
    let out = compare(&core(BASE), &core(&edit(BASE)));
    out.iter().map(|f| (f.level, f.rule)).collect()
}

fn has(found: &[(Level, &'static str)], level: Level, rule: &str) -> bool {
    found.iter().any(|(l, r)| *l == level && *r == rule)
}

#[track_caller]
fn expect(edit: impl Fn(&str) -> String, level: Level, rule: &str) {
    let found = changes(edit);
    assert!(has(&found, level, rule), "wanted {level:?} {rule}, got {found:?}");
}

#[test]
fn a_program_against_itself_has_no_differences() {
    assert!(changes(|s| s.to_string()).is_empty());
    for example in ["ariari", "cms", "saas", "shop"] {
        let src = std::fs::read_to_string(format!("{}/../../examples/{example}/app.aip", env!("CARGO_MANIFEST_DIR"))).expect("example");
        let c = core(&src);
        assert!(compare(&c, &c).is_empty(), "{example} differs from itself");
    }
}

#[test]
fn intents_removed_renamed_and_added() {
    expect(|s| s.replace("command AddItem(", "command AddThing("), Level::Breaking, "intent.removed");
    expect(|s| s.replace("command AddItem(", "command AddThing("), Level::Compatible, "intent.added");
    expect(|s| format!("{s}\nquery Other() {{\n  allow public\n  from Item i\n  select {{ id }}\n}}\n"), Level::Compatible, "intent.added");
    // a query that became a command is one change, not a removal plus an addition
    let found = changes(|s| {
        s.replace(
            "query Items() {\n  allow public\n  from Item i\n  sort by i.name asc\n  page 20 by keyset\n  select { id name note level }\n}",
            "command Items() {\n  allow public\n  do { insert Item { name: \"x\", level: LOW } }\n}",
        )
    });
    assert!(has(&found, Level::Breaking, "intent.kind_changed") && !has(&found, Level::Breaking, "intent.removed"), "{found:?}");
}

#[test]
fn input_rules() {
    expect(
        |s| s.replace("level: Level) {\n  allow public\n  do { insert", "level: Level, extra: Int) {\n  allow public\n  do { insert"),
        Level::Breaking,
        "input.required_added",
    );
    expect(
        |s| s.replace("level: Level) {\n  allow public\n  do { insert", "level: Level, extra: Int?) {\n  allow public\n  do { insert"),
        Level::Compatible,
        "input.optional_added",
    );
    expect(
        |s| s.replace("name: Text(1..30), level: Level)", "name: Text(1..30))").replace("insert Item { name, level }", "insert Item { name }"),
        Level::Breaking,
        "input.removed",
    );
    expect(
        |s| s.replace("AddItem(name: Text(1..30), level: Level)", "AddItem(name: Text(1..30), level: Level = LOW)"),
        Level::Compatible,
        "input.became_optional",
    );
    expect(|s| s.replace("Retag(item: Item, name: Text(1..30))", "Retag(item: Item, name: Text(1..10))"), Level::Breaking, "input.type_narrowed");
    expect(|s| s.replace("Retag(item: Item, name: Text(1..30))", "Retag(item: Item, name: Text(1..300))"), Level::Compatible, "input.type_widened");
    expect(|s| s.replace("Noop(n: Int,", "Noop(n: Text,"), Level::Breaking, "input.type_changed");
    // optional -> required, from a base that has the optional input
    let with_optional = BASE.replace("level: Level) {\n  allow public\n  do { insert", "level: Level, extra: Int?) {\n  allow public\n  do { insert");
    let required = BASE.replace("level: Level) {\n  allow public\n  do { insert", "level: Level, extra: Int) {\n  allow public\n  do { insert");
    assert!(compare(&core(&with_optional), &core(&required)).iter().any(|f| f.rule == "input.became_required" && f.level == Level::Breaking));
}

#[test]
fn output_rules() {
    expect(|s| s.replace("select { id name note level }", "select { id name note }"), Level::Breaking, "output.field_removed");
    expect(|s| s.replace("select { id name note level }", "select { id name note level qty }"), Level::Compatible, "output.field_added");
    expect(|s| s.replace("note: Text?", "note: Text = \"\""), Level::Compatible, "output.became_required");
    // a required output made nullable
    let nullable = BASE.replace("name: Text(1..30)\n  note", "name: Text(1..30)?\n  note");
    assert!(compare(&core(BASE), &core(&nullable)).iter().any(|f| f.rule == "output.became_nullable" && f.level == Level::Breaking));
    // a field that is the same name with another type
    let retyped = BASE.replace("select { id name note level }", "select { id name: qty note level }");
    assert!(compare(&core(BASE), &core(&retyped)).iter().any(|f| f.rule == "output.type_changed" && f.level == Level::Breaking));
    // returning nothing where a value was returned
    expect(|s| s.replace("  returns item { id name }\n", ""), Level::Breaking, "output.removed");
    expect(|s| s.replace("page 20 by keyset", "page 20 by offset max page 10"), Level::Breaking, "page.mode_changed");
}

#[test]
fn error_and_behavior_rules() {
    expect(|s| s.replace("level: Level = LOW\n", "level: Level = LOW\n  unique (name) else NAME_TAKEN\n"), Level::Warning, "error.added");
    let unique = BASE.replace("level: Level = LOW\n", "level: Level = LOW\n  unique (name) else NAME_TAKEN\n");
    assert!(compare(&core(&unique), &core(BASE)).iter().any(|f| f.rule == "error.removed" && f.level == Level::Compatible));
    expect(
        |s| s.replace("command Retag(item: Item, name: Text(1..30)) idempotent {", "command Retag(item: Item, name: Text(1..30)) {"),
        Level::Breaking,
        "idempotency.removed",
    );
    expect(
        |s| s.replace("AddItem(name: Text(1..30), level: Level) {", "AddItem(name: Text(1..30), level: Level) idempotent {"),
        Level::Breaking,
        "idempotency.key_required",
    );
    expect(
        |s| {
            s.replace("allow public\n  from Item i", "allow authenticated\n  from Item i")
                .replace("use postgres", "use postgres\nuse auth\nactor Item via auth.oidc(google)")
        },
        Level::Warning,
        "policy.allow_changed",
    );
    expect(
        |s| s.replace("do { set item.name = name }", "require name != \"x\" else BAD_NAME\n  do { set item.name = name }"),
        Level::Warning,
        "policy.requires_changed",
    );
}

#[test]
fn enum_rules() {
    // LOW|HIGH used as an input: taking a value away breaks callers, adding one does not
    expect(|s| s.replace("enum Level { LOW HIGH }", "enum Level { LOW }"), Level::Breaking, "enum.value_removed");
    // used in an output as well: a value old clients do not know is a warning
    expect(|s| s.replace("enum Level { LOW HIGH }", "enum Level { LOW MID HIGH }"), Level::Warning, "enum.value_added");
}

#[test]
fn only_breaking_changes_make_the_exit_code_one() {
    let (old, widened) = (core(BASE), core(&BASE.replace("Retag(item: Item, name: Text(1..30))", "Retag(item: Item, name: Text(1..300))")));
    assert!(!breaks(&compare(&old, &widened)));
    let narrowed = core(&BASE.replace("Retag(item: Item, name: Text(1..30))", "Retag(item: Item, name: Text(1..10))"));
    assert!(breaks(&compare(&old, &narrowed)));
}

#[test]
fn the_command_reads_a_core_file_and_exits_with_the_verdict() {
    let dir = std::env::temp_dir().join("aip-diff-cli");
    std::fs::create_dir_all(&dir).expect("dir");
    let (old_core, new_src) = (dir.join("old.core.json"), dir.join("new.aip"));
    std::fs::write(&old_core, serde_json::to_string_pretty(&core(BASE)).expect("json")).expect("write");
    let run = |src: &str| {
        std::fs::write(&new_src, src).expect("write");
        let out = Command::new(env!("CARGO_BIN_EXE_aip"))
            .args(["diff", new_src.to_str().expect("path"), "--against", old_core.to_str().expect("path"), "--json"])
            .output()
            .expect("run aip");
        (out.status.code(), String::from_utf8_lossy(&out.stdout).to_string())
    };
    let (code, body) = run(BASE);
    assert_eq!((code, body.trim()), (Some(0), "[]"));
    let (code, body) = run(&BASE.replace("Retag(item: Item, name: Text(1..30))", "Retag(item: Item, name: Text(1..10))"));
    assert_eq!(code, Some(1));
    assert!(body.contains("\"rule\": \"input.type_narrowed\"") && body.contains("\"level\": \"breaking\""), "{body}");
}
