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
    assert_eq!(text(d, "code"), "DEF.UNKNOWN_FIELD");
    assert_eq!((d["line"].as_u64(), d["column"].as_u64()), (Some(54), Some(16)));
    assert_eq!(text(d, "scope"), "resource Recruitment > expose read > select");
    assert!(text(d, "help").contains("did you mean `title`?"), "{d}");
    assert_eq!(text(d, "explain"), "aip explain-code DEF.UNKNOWN_FIELD");
}

#[test]
fn an_unknown_key_lists_what_the_block_accepts() {
    let ds = diagnostics(&sub(A, "sort periodEnd, views, id", "sorts periodEnd, views, id"), Form::A);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "DEF.PARSE_UNKNOWN_KEY");
    assert_eq!(text(d, "scope"), "resource Recruitment > expose read");
    let help = text(d, "help");
    assert!(help.contains("did you mean `sort`?"), "{help}");
    assert!(help.contains("`expose read` accepts: select, refIds, filter, sort, traverse, budget."), "{help}");
}

#[test]
fn a_syntax_error_shows_the_token_as_written() {
    let ds = diagnostics(&sub(A, "rows 50; depth 2", "rows fifty; depth 2"), Form::A);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "DEF.PARSE_EXPECTED");
    assert_eq!(text(d, "message"), "정수 필요, `fifty` 발견", "no compiler internals such as Ident(..) in the message");
    assert_eq!(text(d, "scope"), "resource Recruitment > expose read > budget");
    assert!(text(d, "help").contains("`budget` accepts: rows, depth, deadline, cost, offset, cursor."), "{d}");
}

#[test]
fn the_root_cause_comes_first_and_its_consequences_say_so() {
    // One typo in a field type makes every use of that enum's values unresolvable.
    let ds = diagnostics(&sub(A, "status: RecruitmentStatus\n", "status: RecruitmentStatuz\n"), Form::A);
    assert!(ds.len() > 2, "{ds:?}");
    assert_eq!(text(&ds[0], "code"), "DEF.UNKNOWN_TYPE");
    assert!(text(&ds[0], "help").contains("did you mean `RecruitmentStatus`?"), "{}", ds[0]);
    for d in &ds[1..] {
        assert!(text(d, "help").contains("cascade: this may follow from DEF.UNKNOWN_TYPE at line 45"), "{d}");
        assert!(!text(d, "help").contains("fix:"), "a consequence does not repeat the repair: {d}");
    }
}

#[test]
fn the_repair_of_a_code_is_stated_once_per_answer() {
    let ds = diagnostics(&sub(&sub(A, "periodEnd: Time", "periodEnd: Tiem"), "views: Int", "views: Integer"), Form::A);
    let unknown: Vec<&Value> = ds.iter().filter(|d| text(d, "code") == "DEF.UNKNOWN_TYPE").collect();
    assert_eq!(unknown.len(), 2, "{ds:?}");
    assert!(text(unknown[0], "help").contains("fix:"));
    assert!(!text(unknown[1], "help").contains("fix:"));
    assert!(text(unknown[0], "help").contains("did you mean `Time`?"), "{}", unknown[0]);
}

#[test]
fn a_predicate_call_with_a_typo_offers_the_declared_name() {
    let ds = diagnostics(&sub(A, "rows read when active(this)", "rows read when activ(this)"), Form::A);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "DEF.UNKNOWN_SYMBOL");
    assert_eq!(text(d, "scope"), "resource Recruitment > rows read");
    assert!(text(d, "help").contains("`active`"), "{d}");
}

#[test]
fn a_declaration_without_a_block_is_named_by_its_own_header() {
    let ds = diagnostics(
        &sub(A, "predicate active(r: Recruitment) = r.status = PUBLISHED", "predicate active(r: Recruitment) = r.statuss = PUBLISHED"),
        Form::A,
    );
    let d = ds.iter().find(|d| text(d, "code") == "DEF.UNKNOWN_FIELD").expect("a field diagnostic");
    assert_eq!(text(d, "scope"), "predicate active");
    assert!(text(d, "help").contains("`status`"), "{d}");
}

#[test]
fn an_unknown_filter_operator_lists_the_operators() {
    let ds = diagnostics(&sub(A, "filter periodEnd.gte, periodEnd.lte", "filter periodEnd.gtee, periodEnd.lte"), Form::A);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "DEF.UNKNOWN_OPERATOR");
    assert!(text(d, "help").contains("did you mean `gte`?"), "{d}");
    assert!(text(d, "help").contains("operators: eq, in, isNull, gte, lte, gt, lt, prefix, contains, like."), "{d}");
}

#[test]
fn an_unknown_declaration_keyword_lists_the_declarations() {
    let ds = diagnostics(&format!("resorce Club {{ fields {{ id: Id }} }}\n{A}"), Form::A);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "DEF.PARSE_UNKNOWN_DECL");
    assert!(text(d, "help").contains("did you mean `resource`?"), "{d}");
}

#[test]
fn the_embedded_form_keeps_host_file_lines_and_the_scope() {
    let wrong = sub(E_TS, "sort periodEnd, views, id", "sorts periodEnd, views, id");
    let ds = diagnostics(&wrong, Form::ETs);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "DEF.PARSE_UNKNOWN_KEY");
    let line = wrong.lines().position(|l| l.contains("sorts periodEnd")).expect("line") as u64 + 1;
    assert_eq!(d["line"].as_u64(), Some(line), "the line is the line of the host file");
    assert_eq!(text(d, "scope"), "resource Recruitment > expose read");
}

#[test]
fn the_host_literal_form_gets_help_without_a_scope() {
    let ds = diagnostics(&sub(H_TS, "rows: 50,", "rows: \"50\","), Form::HTs);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "DEF.H_SHAPE");
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
                    if part.strip_prefix("DEF.").is_some_and(|name| {
                        name.len() >= 4
                            && name.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                            && name.starts_with(|c: char| c.is_ascii_uppercase())
                    }) {
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
    // A retired code is kept in the registry on purpose (a published code is never deleted), so it need not be emitted.
    let retired: BTreeSet<String> = codes::all().iter().filter(|c| c.status == codes::Status::Retired).map(|c| c.code.to_string()).collect();
    let dead: Vec<&String> =
        registered.iter().filter(|r| *r != "DEF.INVALID_DEFINITION" && !literals.contains(*r) && !retired.contains(*r)).collect();
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

#[test]
fn the_card_notes_cover_exactly_the_keys_the_parser_reads() {
    use spike_v1_fixture::syntax_card::NOTES;
    for (block, keys) in codes::BLOCK_KEYS {
        for key in *keys {
            let n = NOTES.iter().filter(|(b, k, _, _)| b == block && k == key).count();
            assert_eq!(n, 1, "`{block}` key `{key}` needs exactly one note in src/syntax_card.rs, has {n}");
        }
    }
    for (block, key, shape, meaning) in NOTES {
        let known = codes::BLOCK_KEYS.iter().any(|(b, keys)| b == block && keys.contains(key));
        assert!(known, "a note for `{block}` `{key}`, which the parser does not read");
        assert!(!shape.is_empty() && !meaning.is_empty(), "`{block}` `{key}` needs a shape and a meaning");
    }
}

#[test]
fn spec_definition_syntax_is_generated_from_the_card() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/definition-syntax.md");
    let generated = spike_v1_fixture::syntax_card::render_markdown();
    if std::env::var("AIP_UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &generated).expect("write spec/definition-syntax.md");
    }
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(on_disk == generated, "spec/definition-syntax.md differs from the card (AIP_UPDATE_GOLDEN=1 regenerates it)");
}

#[test]
fn the_worked_examples_of_the_card_are_accepted() {
    for (what, src) in [("recruitment.aip", A), ("prototype/example/app.aip", include_str!("../../../prototype/example/app.aip"))] {
        if let Err(ds) = load_str(src, Form::A) {
            panic!("{what}: {}", ds.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("; "));
        }
    }
}

/// Each sentence of "언어가 가정하지 않는 것" in the card, shown by a definition.
#[test]
fn the_defaults_the_card_denies_hold() {
    let facts = load_str(A, Form::A).expect("fixture").execution;
    let resources = &facts["resources"];
    // no `rows read`: no row is readable; no `expose read`: no read exposure
    assert_eq!(resources["School"]["rowRead"], serde_json::json!({"default": "denyAll"}));
    assert!(resources["School"].get("exposeRead").is_none());
    // no `budget`: not readable as a root
    assert_eq!(resources["Club"]["exposeRead"]["rootQueryable"], false);
    assert_eq!(resources["Recruitment"]["exposeRead"]["rootQueryable"], true);

    let budget = "budget { rows 50; depth 2; deadline 2s; cost 1000 }";
    for partial in [
        "budget { depth 2; deadline 2s; cost 1000 }",
        "budget { rows 50; deadline 2s; cost 1000 }",
        "budget { rows 50; depth 2; cost 1000 }",
        "budget { rows 50; depth 2; deadline 2s }",
    ] {
        let ds = diagnostics(&sub(A, budget, partial), Form::A);
        assert!(
            ds.iter().any(|d| text(d, "code") == "DEF.MISSING_ITEM" && text(d, "message").contains("rows/depth/deadline/cost")),
            "{partial}: {ds:?}"
        );
    }

    // `expose apply` without `bulk maxRows`
    let apply = "resource Note {\n  fields { id: Id; done: Bool; owner: Member }\n  rows read when owner = actor\n  transition finish { from done = false; to done = true; allow owner = actor }\n  expose apply finish { target id }\n}\n";
    let base = "actor Member\nresource Member { fields { id: Id } }\n";
    let ds = diagnostics(&format!("{base}{apply}"), Form::A);
    assert!(ds.iter().any(|d| text(d, "code") == "DEF.MISSING_ITEM" && text(d, "message").contains("bulk maxRows")), "{ds:?}");
    assert!(load_str(&format!("{base}{}", apply.replace("target id }", "target id; bulk maxRows 5 }")), Form::A).is_ok());

    // an aggregate without `sourceAccess`
    let ds = diagnostics(&sub(A, "    sourceAccess fixedTotalOfVisibleRecruitment\n", ""), Form::A);
    assert!(ds.iter().any(|d| text(d, "code") == "DEF.MISSING_ITEM" && text(d, "message").contains("sourceAccess")), "{ds:?}");

    // a field with a read policy cannot be filtered
    let ds = diagnostics(&sub(A, "filter periodEnd.gte, periodEnd.lte", "filter periodEnd.gte, internalNote.eq"), Form::A);
    assert!(ds.iter().any(|d| text(d, "code") == "DEF.POLICY_FIELD_NOT_FILTERABLE"));

    // an unknown key is an error, and a syntax error stops at the first one
    let ds = diagnostics(&sub(A, "sort periodEnd, views, id", "sorts periodEnd, views, id"), Form::A);
    assert_eq!((ds.len(), text(&ds[0], "code")), (1, "DEF.PARSE_UNKNOWN_KEY"));
}

const TRANSITION_BASE: &str = "actor Member\nresource Member { fields { id: Id; name: Text } }\nresource Note {\n  fields { id: Id; owner: Member; done: Bool; views: Int(0..1000) }\n  rows read when owner = actor\n  transition finish { from done = false; TO; allow owner = actor }\n  expose apply finish { target id; bulk maxRows 5 }\n}\n";

#[test]
fn a_semicolon_inside_the_to_clause_names_the_separator() {
    let semicolon = TRANSITION_BASE.replace("TO", "to done = true; views = views + 1");
    let ds = diagnostics(&semicolon, Form::A);
    let d = &ds[0];
    assert_eq!(text(d, "code"), "DEF.PARSE_UNKNOWN_KEY");
    let message = text(d, "message");
    assert!(message.contains("`views`") && message.contains("쉼표") && message.contains("to done = true, views = views + 1"), "{message}");
    assert_eq!((d["line"].as_u64(), d["column"].as_u64()), (Some(6), Some(58)), "위치는 구분자 뒤의 항목: {d}");

    let comma = TRANSITION_BASE.replace("TO", "to done = true, views = views + 1");
    assert!(load_str(&comma, Form::A).is_ok(), "쉼표로 이은 to는 통과");

    // `to` 없이 알 수 없는 항목은 구분자 안내를 붙이지 않는다.
    let unknown = TRANSITION_BASE.replace("TO", "to done = true; views");
    let message = text(&diagnostics(&unknown, Form::A)[0], "message").to_string();
    assert!(!message.contains("쉼표"), "{message}");
}

#[test]
fn the_card_states_the_real_caller_filter_and_to_rules() {
    let card = spike_v1_fixture::syntax_card::render_markdown();
    assert!(card.contains("to f = v, g = g + 1"), "to 절 구분자는 쉼표");
    assert!(!card.contains("to f = v; g = g + 1"));
    assert!(!card.contains("생략하면 숨긴 행의 존재를 필터로 셀 수 있어"), "callerFilter 생략은 경고가 아니라 오류");
    assert!(card.contains("생략하면 `DEF.MISSING_ITEM` 오류"));
    assert!(card.contains("DEF.CALLER_FILTER_POLICY_REQUIRED") && card.contains("DEF.ADVISORY_PUBLIC_COUNT"));
    assert!(card.contains("## 공개 count 계약"));
    // 설명이 실제 규칙이라는 근거: 생략은 오류다.
    let omitted = A.replace("    callerFilter none\n", "");
    if omitted != A {
        let ds = diagnostics(&omitted, Form::A);
        assert!(ds.iter().any(|d| text(d, "code") == "DEF.MISSING_ITEM" && text(d, "message").contains("callerFilter")), "{ds:?}");
    }
}

#[test]
fn user_facing_diagnostics_do_not_mention_spike() {
    let unsupported_extension = "actor Member\nresource Member {\n  fields { id: Id }\n  extension stream x { input { a: Int }; output { b: Int }; effect none; deadline 2s; implementation \"m.f\" }\n}\n";
    let unsupported_filter = A.replacen("callerFilter none", "callerFilter bad", 1);
    let unsupported_row_output = A.replacen("rowOutput none", "rowOutput rows", 1);
    let two_blocks = format!("{E_TS}\nexport const other = aip`actor Member`\n");
    for (what, src, form) in [
        ("callerFilter", unsupported_filter.as_str(), Form::A),
        ("rowOutput", unsupported_row_output.as_str(), Form::A),
        ("extension kind", unsupported_extension, Form::A),
        ("two blocks", two_blocks.as_str(), Form::ETs),
    ] {
        let ds = diagnostics(src, form);
        // help에는 저장소 경로(`spikes/...`)가 있다. crate 이름 변경은 이번 범위가 아니라서 메시지 문장만 본다.
        let rendered = ds.iter().map(|d| text(d, "message").to_lowercase()).collect::<Vec<_>>().join("; ");
        assert!(!rendered.contains("spike") && !rendered.contains("스파이크"), "{what}: {rendered}");
        assert!(rendered.contains("지원하지 않") || rendered.contains("하나만"), "{what}가 거부되지 않았거나 다른 이유로 거부됨: {rendered}");
    }
}

const DECIMAL_BASE: &str = "actor Member\nresource Member { fields { id: Id; name: Text } }\nresource Invoice {\n  fields { id: Id; owner: Member; total: Decimal(10,2); paid: Bool }\n  rows read when owner = actor\n  transition reset { from FROM; to TO; allow owner = actor }\n  expose apply reset { target id; bulk maxRows 1 }\n}\n";

fn decimal_sample(from: &str, to: &str) -> String {
    DECIMAL_BASE.replace("FROM", from).replace("TO", to)
}

#[test]
fn a_traverse_via_without_child_dot_field_shows_the_right_form() {
    let registry = codes::lookup("DEF.TRAVERSE_VIA_FORM").expect("registered").example;
    assert!(load_str(registry, Form::A).is_ok(), "정답 형태 `via Vote.record`는 통과");
    for wrong in ["via record {", "via Vote {"] {
        let src = sub(registry, "via Vote.record {", wrong);
        let ds = diagnostics(&src, Form::A);
        assert_eq!(ds.len(), 1, "{ds:?}");
        assert_eq!(text(&ds[0], "code"), "DEF.TRAVERSE_VIA_FORM", "{wrong}: {ds:?}");
        let message = text(&ds[0], "message");
        assert!(message.contains("`Child.field`") && message.contains("traverse votes via Child."), "{message}");
    }
}

#[test]
fn the_card_writes_the_real_traverse_form() {
    let card = spike_v1_fixture::syntax_card::render_markdown();
    assert!(card.contains("traverse name via Child.field"), "문서는 via Child.field");
    assert!(!card.contains("traverse Child via field"));
}

#[test]
fn a_bare_number_for_a_decimal_names_the_quoted_literal() {
    assert!(load_str(&decimal_sample("paid = false", "total = \"0.00\""), Form::A).is_ok(), "따옴표 문자열은 통과");
    for (what, from, to) in [
        ("정수", "paid = false", "total = 0"),
        ("소수점 숫자", "paid = false", "total = 0.00"),
        ("조건의 정수", "total > 0", "total = \"1.00\""),
        ("조건의 소수점 숫자", "total > 0.50", "total = \"1.00\""),
    ] {
        let ds = diagnostics(&decimal_sample(from, to), Form::A);
        assert_eq!(ds.len(), 1, "{what}: {ds:?}");
        assert_eq!(text(&ds[0], "code"), "DEF.BAD_VALUE", "{what}: {ds:?}");
        let message = text(&ds[0], "message");
        assert!(message.contains("따옴표 문자열") && (message.contains("\"0.00\"") || message.contains("\"1.50\"")), "{what}: {message}");
    }
    let message = text(&diagnostics(&decimal_sample("paid = false", "total = 0"), Form::A)[0], "message").to_string();
    assert!(message.contains("Decimal<10,2>") && message.contains("\"0.00\""), "자릿수에 맞는 예시: {message}");
    let card = spike_v1_fixture::syntax_card::render_markdown();
    assert!(card.contains("`Decimal(p,s)` 값은 따옴표 문자열 리터럴로 쓴다"));
}

#[test]
fn a_transition_without_from_says_that_from_is_required() {
    let patch_only = "actor Member\nresource Member { fields { id: Id } }\nresource Note {\n  fields { id: Id; owner: Member; title: Text }\n  rows read when owner = actor\n  transition edit { FROM allow owner = actor; patch title }\n  expose apply edit { target id; bulk maxRows 1 }\n}\n";
    let with_from = patch_only.replace("FROM", "from true;");
    assert!(load_str(&with_from, Form::A).is_ok(), "`from true`는 무조건 허용");
    let ds = diagnostics(&patch_only.replace("FROM", ""), Form::A);
    assert_eq!((ds.len(), text(&ds[0], "code")), (1, "DEF.MISSING_ITEM"), "{ds:?}");
    let message = text(&ds[0], "message");
    assert!(message.contains("`from`") && message.contains("필수") && message.contains("from true") && message.contains("`edit`"), "{message}");
    assert!(!message.contains("`allow`("), "allow는 빠지지 않았다: {message}");
    // allow와 본문이 빠진 경우도 무엇이 빠졌는지 가른다.
    let ds = diagnostics("actor Member\nresource Member { fields { id: Id } }\nresource Note {\n  fields { id: Id; owner: Member }\n  rows read when owner = actor\n  transition edit { from true }\n}\n", Form::A);
    let message = text(&ds[0], "message");
    assert!(message.contains("`allow`") && message.contains("`to`, `patch`, `delete` 중 하나") && !message.contains("`from`("), "{message}");
    // host 리터럴 형식도 같은 안내를 준다.
    let ds = diagnostics(&sub(H_TS, "close: { from: \"status = PUBLISHED\", to:", "close: { to:"), Form::HTs);
    let message = text(&ds[0], "message");
    assert_eq!(text(&ds[0], "code"), "DEF.MISSING_ITEM");
    assert!(message.contains("`from`") && message.contains("필수") && message.contains("`close`"), "{message}");
    let card = spike_v1_fixture::syntax_card::render_markdown();
    assert!(card.contains("필수다(patch만 있는 전이도 마찬가지이며") && card.contains("`from true`"));
}

#[test]
fn a_second_actor_is_reported_alone_before_any_follow_on_error() {
    let two = format!("actor Club\n{A}");
    let ds = diagnostics(&two, Form::A);
    assert_eq!(ds.len(), 1, "연쇄 오류 없이 하나: {ds:?}");
    assert_eq!(text(&ds[0], "code"), "DEF.MULTIPLE_ACTORS");
    let message = text(&ds[0], "message");
    assert!(message.contains("정확히 하나") && message.contains("`Club`"), "{message}");
    // 마지막 선언이 이기던 때는 첫 actor를 쓰는 정책이 줄줄이 실패했다. 한 개면 정상이다.
    assert!(load_str(A, Form::A).is_ok());
    let embedded = sub(E_TS, "aip`", "aip`\n  actor Club\n");
    let ds = diagnostics(&embedded, Form::ETs);
    assert_eq!((ds.len(), text(&ds[0], "code")), (1, "DEF.MULTIPLE_ACTORS"), "{ds:?}");
}
