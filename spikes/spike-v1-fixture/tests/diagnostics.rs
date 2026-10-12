//! What a caller (a person or an AI) gets back from a wrong definition: where, why and how to repair it.
//! The samples are the fixture with one deliberate mistake each, the way a generated definition is wrong.

use serde_json::Value;
use spike_v1_fixture::{codes, load_str, Form};
use std::collections::BTreeSet;
use std::path::PathBuf;

const A: &str = include_str!("../fixture/recruitment.aip");
const E_TS: &str = include_str!("../fixture/recruitment.e.ts");
const H_TS: &str = include_str!("../fixture/recruitment.h.ts");

fn sub(base: &str, old: &str, new: &str) -> String {
    assert_eq!(base.matches(old).count(), 1, "the sample needs `{old}` exactly once");
    base.replacen(old, new, 1)
}

fn diagnostics(src: &str, form: Form) -> Vec<Value> {
    match load_str(src, form) {
        Ok(_) => panic!("the sample was accepted"),
        Err(ds) => ds.iter().map(|d| d.json()).collect(),
    }
}

fn text<'a>(d: &'a Value, key: &str) -> &'a str {
    d[key].as_str().unwrap_or("")
}

#[test]
fn the_fixture_itself_is_accepted() {
    assert!(load_str(A, Form::A).is_ok());
}

#[test]
fn a_misspelled_field_names_its_scope_and_the_close_spelling() {
    let ds = diagnostics(&sub(A, "select id, title, periodEnd", "select id, titel, periodEnd"), Form::A);
    assert_eq!(ds.len(), 1);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "UNKNOWN_FIELD");
    assert_eq!((d["line"].as_u64(), d["column"].as_u64()), (Some(54), Some(16)));
    assert_eq!(text(d, "scope"), "resource Recruitment > expose read > select");
    assert!(text(d, "help").contains("did you mean `title`?"), "{d}");
    assert_eq!(text(d, "explain"), "aip explain-code UNKNOWN_FIELD");
}

#[test]
fn an_unknown_key_lists_what_the_block_accepts() {
    let ds = diagnostics(&sub(A, "sort periodEnd, views, id", "sorts periodEnd, views, id"), Form::A);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "PARSE_UNKNOWN_KEY");
    assert_eq!(text(d, "scope"), "resource Recruitment > expose read");
    let help = text(d, "help");
    assert!(help.contains("did you mean `sort`?"), "{help}");
    assert!(help.contains("`expose read` accepts: select, filter, sort, traverse, budget."), "{help}");
}

#[test]
fn a_syntax_error_shows_the_token_as_written() {
    let ds = diagnostics(&sub(A, "rows 50; depth 2", "rows fifty; depth 2"), Form::A);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "PARSE_EXPECTED");
    assert_eq!(text(d, "message"), "정수 필요, `fifty` 발견", "no compiler internals such as Ident(..) in the message");
    assert_eq!(text(d, "scope"), "resource Recruitment > expose read > budget");
    assert!(text(d, "help").contains("`budget` accepts: rows, depth, deadline, cost, offset, cursor."), "{d}");
}

#[test]
fn the_root_cause_comes_first_and_its_consequences_say_so() {
    // One typo in a field type makes every use of that enum's values unresolvable.
    let ds = diagnostics(&sub(A, "status: RecruitmentStatus\n", "status: RecruitmentStatuz\n"), Form::A);
    assert!(ds.len() > 2, "{ds:?}");
    assert_eq!(text(&ds[0], "code"), "UNKNOWN_TYPE");
    assert!(text(&ds[0], "help").contains("did you mean `RecruitmentStatus`?"), "{}", ds[0]);
    for d in &ds[1..] {
        assert!(text(d, "help").contains("cascade: this may follow from UNKNOWN_TYPE at line 45"), "{d}");
        assert!(!text(d, "help").contains("fix:"), "a consequence does not repeat the repair: {d}");
    }
}

#[test]
fn the_repair_of_a_code_is_stated_once_per_answer() {
    let ds = diagnostics(&sub(&sub(A, "periodEnd: Time", "periodEnd: Tiem"), "views: Int", "views: Integer"), Form::A);
    let unknown: Vec<&Value> = ds.iter().filter(|d| text(d, "code") == "UNKNOWN_TYPE").collect();
    assert_eq!(unknown.len(), 2, "{ds:?}");
    assert!(text(unknown[0], "help").contains("fix:"));
    assert!(!text(unknown[1], "help").contains("fix:"));
    assert!(text(unknown[0], "help").contains("did you mean `Time`?"), "{}", unknown[0]);
}

#[test]
fn a_predicate_call_with_a_typo_offers_the_declared_name() {
    let ds = diagnostics(&sub(A, "rows read when active(this)", "rows read when activ(this)"), Form::A);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "UNKNOWN_SYMBOL");
    assert_eq!(text(d, "scope"), "resource Recruitment > rows read");
    assert!(text(d, "help").contains("`active`"), "{d}");
}

#[test]
fn a_declaration_without_a_block_is_named_by_its_own_header() {
    let ds = diagnostics(
        &sub(A, "predicate active(r: Recruitment) = r.status = PUBLISHED", "predicate active(r: Recruitment) = r.statuss = PUBLISHED"),
        Form::A,
    );
    let d = ds.iter().find(|d| text(d, "code") == "UNKNOWN_FIELD").expect("a field diagnostic");
    assert_eq!(text(d, "scope"), "predicate active");
    assert!(text(d, "help").contains("`status`"), "{d}");
}

#[test]
fn an_unknown_filter_operator_lists_the_operators() {
    let ds = diagnostics(&sub(A, "filter periodEnd.gte, periodEnd.lte", "filter periodEnd.gtee, periodEnd.lte"), Form::A);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "UNKNOWN_OPERATOR");
    assert!(text(d, "help").contains("did you mean `gte`?"), "{d}");
    assert!(text(d, "help").contains("operators: eq, in, isNull, gte, lte, gt, lt, prefix, contains."), "{d}");
}

#[test]
fn an_unknown_declaration_keyword_lists_the_declarations() {
    let ds = diagnostics(&format!("resorce Club {{ fields {{ id: Id }} }}\n{A}"), Form::A);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "PARSE_UNKNOWN_DECL");
    assert!(text(d, "help").contains("did you mean `resource`?"), "{d}");
}

#[test]
fn the_embedded_form_keeps_host_file_lines_and_the_scope() {
    let wrong = sub(E_TS, "sort periodEnd, views, id", "sorts periodEnd, views, id");
    let ds = diagnostics(&wrong, Form::ETs);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "PARSE_UNKNOWN_KEY");
    let line = wrong.lines().position(|l| l.contains("sorts periodEnd")).expect("line") as u64 + 1;
    assert_eq!(d["line"].as_u64(), Some(line), "the line is the line of the host file");
    assert_eq!(text(d, "scope"), "resource Recruitment > expose read");
}

#[test]
fn the_host_literal_form_gets_help_without_a_scope() {
    let ds = diagnostics(&sub(H_TS, "rows: 50,", "rows: \"50\","), Form::HTs);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "H_SHAPE");
    assert!(d.get("scope").is_none(), "no declaration path can be derived from host data: {d}");
    assert!(text(d, "help").contains("fix:"), "{d}");
}

#[test]
fn every_code_is_registered_and_none_is_dead() {
    let src_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let v11 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../spike-v11-dev-checks/src");
    let mut literals = BTreeSet::new();
    for dir in [src_dir, v11] {
        for e in std::fs::read_dir(dir).expect("src dir") {
            let p = e.expect("entry").path();
            // The registry and the enrichment name codes only as data; every other file emits them.
            if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("codes.rs") && !p.ends_with("enrich.rs") {
                let t = std::fs::read_to_string(&p).expect("source");
                for part in t.split('"').skip(1).step_by(2) {
                    if part.len() >= 4
                        && part.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                        && part.starts_with(|c: char| c.is_ascii_uppercase())
                    {
                        literals.insert(part.to_string());
                    }
                }
            }
        }
    }
    let registered: BTreeSet<String> = codes::all().iter().map(|c| c.code.to_string()).collect();
    assert_eq!(registered.len(), codes::all().len(), "a code is registered twice");
    let missing: Vec<&String> = literals.iter().filter(|l| !registered.contains(*l)).collect();
    assert!(missing.is_empty(), "codes emitted by the sources but not in src/codes.rs: {missing:?}");
    // INVALID_DEFINITION is the envelope the service adds around the list.
    let dead: Vec<&String> = registered.iter().filter(|r| *r != "INVALID_DEFINITION" && !literals.contains(*r)).collect();
    assert!(dead.is_empty(), "registered but nothing emits them: {dead:?}");
    for c in codes::all() {
        assert!(!c.title.is_empty() && !c.explain.is_empty() && !c.fix.is_empty(), "{} needs title, explain and fix", c.code);
        assert!(!c.title.contains('\n'), "{} title is one line", c.code);
    }
}

#[test]
fn every_example_is_an_accepted_definition() {
    for c in codes::all().iter().filter(|c| !c.example.is_empty()) {
        if let Err(ds) = load_str(c.example, Form::A) {
            panic!("the example of {} is rejected: {}", c.code, ds.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("; "));
        }
    }
}

#[test]
fn the_keys_a_block_accepts_are_the_keys_the_parser_reads() {
    let parser = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/parser.rs")).expect("parser source");
    for (block, keys) in codes::BLOCK_KEYS {
        for key in *keys {
            assert!(
                parser.contains(&format!("\"{key}\" =>")) || parser.contains(&format!("\"{key}\" |")),
                "`{block}`: the parser has no arm for `{key}`"
            );
        }
    }
}

#[test]
fn spec_definition_diagnostics_is_generated_from_the_registry() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/definition-diagnostics.md");
    let generated = codes::render_markdown();
    if std::env::var("AIP_UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &generated).expect("write spec/definition-diagnostics.md");
    }
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(on_disk == generated, "spec/definition-diagnostics.md differs from the registry (AIP_UPDATE_GOLDEN=1 regenerates it)");
}

#[test]
fn every_registered_code_points_to_its_explanation() {
    for c in codes::all() {
        let d = spike_v1_fixture::diag::Diag::new(c.code, "x", Default::default());
        assert_eq!(d.json()["explain"], format!("aip explain-code {}", c.code));
    }
}
