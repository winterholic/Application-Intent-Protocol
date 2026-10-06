use spike_v1_fixture::{diag::Span, extract::Host, host::tokenize, lexer::lex, load_str, parser::parse_expr_str, Form};
use std::process::Command;

const MAX_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_TOKENS: usize = 65_536;
const MAX_EXPR_NODES: usize = 4_096;

#[test]
fn rejects_source_over_the_byte_budget_before_parsing_any_form() {
    let src = format!("{}x", " ".repeat(MAX_SOURCE_BYTES));
    for form in [Form::A, Form::ETs, Form::EPy, Form::HTs, Form::HPy] {
        let errors = load_str(&src, form).err().expect("oversized source must fail");
        assert_eq!(errors[0].code, "SOURCE_TOO_LARGE", "{form:?}");
    }
}

#[test]
fn lexer_rejects_the_token_that_would_exceed_the_budget() {
    let src = std::iter::repeat_n("x", MAX_TOKENS + 1).collect::<Vec<_>>().join(" ");
    let error = match lex(&src, 1, 1) {
        Err(error) => error,
        Ok(_) => panic!("lexer accepted more than {MAX_TOKENS} significant tokens"),
    };
    assert_eq!(error.code, "TOKEN_LIMIT");
}

#[test]
fn lexer_accepts_exactly_the_token_budget() {
    let src = std::iter::repeat_n("x", MAX_TOKENS).collect::<Vec<_>>().join(" ");
    assert_eq!(lex(&src, 1, 1).unwrap().len(), MAX_TOKENS + 1);
}

#[test]
fn host_tokenizer_rejects_tokens_before_building_an_unbounded_vector() {
    let src = "x ".repeat(MAX_TOKENS + 1);
    for host in [Host::Ts, Host::Py] {
        let error = tokenize(&src, host).unwrap_err();
        assert_eq!(error.code, "TOKEN_LIMIT", "{host:?}");
    }
}

#[test]
fn expression_depth_accepts_63_wrappers_and_rejects_the_64th() {
    let span = Span { line: 1, col: 1 };
    let accepted = format!("{}x = x{}", "(".repeat(63), ")".repeat(63));
    assert!(parse_expr_str(&accepted, span).is_ok());

    let rejected = format!("{}x = x{}", "(".repeat(64), ")".repeat(64));
    let error = parse_expr_str(&rejected, span).unwrap_err();
    assert_eq!(error.code, "PARSE_NESTING");

    let not_rejected = format!("{}x = x", "not ".repeat(64));
    assert_eq!(parse_expr_str(&not_rejected, span).unwrap_err().code, "PARSE_NESTING");
}

#[test]
fn host_literal_depth_accepts_the_existing_shape_and_rejects_one_more_level() {
    let source = include_str!("../fixture/recruitment.h.ts");
    let index = source.rfind("})").unwrap();
    let accepted = format!("{} nestedTest: {}1{},\n{}", &source[..index], "[".repeat(63), "]".repeat(63), &source[index..]);
    let errors = load_str(&accepted, Form::HTs).err().expect("unknown key after literal parse");
    assert_eq!(errors[0].code, "UNKNOWN_KEY");

    let rejected = format!("{} nestedTest: {}1{},\n{}", &source[..index], "[".repeat(64), "]".repeat(64), &source[index..]);
    let errors = load_str(&rejected, Form::HTs).err().expect("literal depth limit");
    assert_eq!(errors[0].code, "H_LITERAL_NESTING");
}

#[test]
fn flat_boolean_expression_is_bounded_by_expression_nodes() {
    let src = std::iter::repeat_n("true", MAX_EXPR_NODES + 1).collect::<Vec<_>>().join(" and ");
    let error = parse_expr_str(&src, Span { line: 1, col: 1 }).unwrap_err();
    assert_eq!(error.code, "PARSE_NESTING");
}

#[test]
fn expression_node_budget_boundary_is_enforced_while_parsing() {
    let span = Span { line: 1, col: 1 };
    let at_limit = std::iter::repeat_n("true", MAX_EXPR_NODES - 1).collect::<Vec<_>>().join(" and ");
    assert!(parse_expr_str(&at_limit, span).is_ok());

    let over_limit = std::iter::repeat_n("true", MAX_EXPR_NODES).collect::<Vec<_>>().join(" and ");
    assert_eq!(parse_expr_str(&over_limit, span).unwrap_err().code, "PARSE_NESTING");
}

#[test]
fn call_and_in_operand_nodes_share_the_expression_node_budget() {
    let span = Span { line: 1, col: 1 };
    let args = std::iter::repeat_n("true", MAX_EXPR_NODES).collect::<Vec<_>>().join(", ");
    assert_eq!(parse_expr_str(&format!("f({args})"), span).unwrap_err().code, "PARSE_NESTING");

    let items = std::iter::repeat_n("1", MAX_EXPR_NODES).collect::<Vec<_>>().join(", ");
    assert_eq!(parse_expr_str(&format!("x in ({items})"), span).unwrap_err().code, "PARSE_NESTING");
}

#[test]
fn flat_path_segments_remain_bounded_by_source_and_token_limits() {
    let path = format!("root{}", ".segment".repeat(10_000));
    assert!(path.len() < MAX_SOURCE_BYTES);
    assert!(lex(&path, 1, 1).unwrap().len() < MAX_TOKENS);
    assert!(parse_expr_str(&path, Span { line: 1, col: 1 }).is_ok());
}

#[test]
fn executable_policy_path_has_a_segment_budget_even_though_flat_syntax_parses() {
    let base = include_str!("../fixture/recruitment.aip");
    let active = "predicate active(r: Recruitment) = r.status = PUBLISHED and r.periodEnd >= now";
    let recruitment_fields = "  fields {\n    id: Id\n    title: Text(1..100)";
    assert_eq!(base.matches(active).count(), 1);
    assert_eq!(base.matches(recruitment_fields).count(), 1);
    let at_limit_path = format!("r{}.title = \"x\"", ".next".repeat(63));
    let at_limit = base.replacen(active, &format!("predicate active(r: Recruitment) = {at_limit_path}"), 1).replacen(
        recruitment_fields,
        "  fields {\n    id: Id\n    next: Recruitment?\n    title: Text(1..100)",
        1,
    );
    assert!(load_str(&at_limit, Form::A).is_ok(), "exactly 64 field steps remain supported");

    let long_path = format!("r{}.title = \"x\"", ".next".repeat(65));
    let source = base.replacen(active, &format!("predicate active(r: Recruitment) = {long_path}"), 1).replacen(
        recruitment_fields,
        "  fields {\n    id: Id\n    next: Recruitment?\n    title: Text(1..100)",
        1,
    );
    let errors = load_str(&source, Form::A).err().expect("runtime-expensive path must be rejected semantically");
    assert_eq!(errors[0].code, "POLICY_EXPANSION_LIMIT", "{errors:?}");
}

#[test]
fn actual_recursive_ast_depth_is_measured_after_parse() {
    let span = Span { line: 1, col: 1 };
    let nested_chain = format!("{}x = x and true", "not ".repeat(63));
    assert_eq!(parse_expr_str(&nested_chain, span).unwrap_err().code, "PARSE_NESTING");
}

#[test]
fn flat_chain_predicate_child_probe() {
    if std::env::var_os("AIP_FLAT_CHAIN_CHILD").is_some() {
        let base = include_str!("../fixture/recruitment.aip");
        let old = "predicate active(r: Recruitment) = r.status = PUBLISHED and r.periodEnd >= now";
        assert_eq!(base.matches(old).count(), 1);
        let terms = std::iter::repeat_n("true", 10_000).collect::<Vec<_>>().join(" and ");
        let source = base.replacen(old, &format!("predicate active(r: Recruitment) = {terms}"), 1);
        assert!(source.len() < MAX_SOURCE_BYTES, "{} source bytes", source.len());
        assert!(lex(&source, 1, 1).unwrap().len() < MAX_TOKENS);
        let errors = load_str(&source, Form::A).err().expect("oversized flat chain must be rejected");
        assert_eq!(errors[0].code, "PARSE_NESTING");
        return;
    }

    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "flat_chain_predicate_child_probe"])
        .env("AIP_FLAT_CHAIN_CHILD", "1")
        .status()
        .unwrap();
    assert!(status.success(), "flat-chain child exited with {status}");
}

#[test]
fn predicate_call_chain_child_probe() {
    if std::env::var_os("AIP_PREDICATE_CHAIN_CHILD").is_some() {
        let base = include_str!("../fixture/recruitment.aip");
        let old = "predicate active(r: Recruitment) = r.status = PUBLISHED and r.periodEnd >= now";
        assert_eq!(base.matches(old).count(), 1);
        let definitions = (0..1_000)
            .map(|i| format!("predicate p{i}() = p{}()", i + 1))
            .chain(std::iter::once("predicate p1000() = true".to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        let source = base.replacen(old, &format!("predicate active(r: Recruitment) = p0()\n{definitions}"), 1);
        assert!(source.len() < MAX_SOURCE_BYTES, "{} source bytes", source.len());
        assert!(lex(&source, 1, 1).unwrap().len() < MAX_TOKENS);
        let errors = load_str(&source, Form::A).err().expect("long predicate call chain must be rejected");
        assert_eq!(errors[0].code, "POLICY_EXPANSION_LIMIT");
        return;
    }

    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "predicate_call_chain_child_probe"])
        .env("AIP_PREDICATE_CHAIN_CHILD", "1")
        .status()
        .unwrap();
    assert!(status.success(), "predicate-chain child exited with {status}");
}

#[test]
fn predicate_diamond_child_probe() {
    if std::env::var_os("AIP_PREDICATE_DIAMOND_CHILD").is_some() {
        let base = include_str!("../fixture/recruitment.aip");
        let old = "predicate active(r: Recruitment) = r.status = PUBLISHED and r.periodEnd >= now";
        assert_eq!(base.matches(old).count(), 1);
        let definitions = std::iter::once("predicate p0() = true".to_string())
            .chain((1..=30).map(|i| format!("predicate p{i}() = p{}() and p{}()", i - 1, i - 1)))
            .collect::<Vec<_>>()
            .join("\n");
        let source = base.replacen(old, &format!("predicate active(r: Recruitment) = p30()\n{definitions}"), 1);
        assert!(source.len() < MAX_SOURCE_BYTES, "{} source bytes", source.len());
        assert!(lex(&source, 1, 1).unwrap().len() < MAX_TOKENS);
        let errors = load_str(&source, Form::A).err().expect("predicate diamond expansion must be rejected");
        assert_eq!(errors[0].code, "POLICY_EXPANSION_LIMIT");
        return;
    }

    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "predicate_diamond_child_probe"])
        .env("AIP_PREDICATE_DIAMOND_CHILD", "1")
        .status()
        .unwrap();
    assert!(status.success(), "predicate-diamond child exited with {status}");
}

#[test]
fn ordinary_multiple_predicate_calls_remain_accepted() {
    let base = include_str!("../fixture/recruitment.aip");
    let old = "predicate active(r: Recruitment) = r.status = PUBLISHED and r.periodEnd >= now";
    assert_eq!(base.matches(old).count(), 1);
    let source = base.replacen(old, "predicate active(r: Recruitment) = p0() and p1()\npredicate p0() = true\npredicate p1() = true", 1);
    assert!(load_str(&source, Form::A).is_ok());
}

#[test]
fn small_stack_child_process_parses_supported_nesting_without_overflow() {
    if std::env::var_os("AIP_SMALL_STACK_CHILD").is_some() {
        let thread = std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(|| {
                let span = Span { line: 1, col: 1 };
                let parens = format!("{}x = x{}", "(".repeat(63), ")".repeat(63));
                assert!(parse_expr_str(&parens, span).is_ok());
                let recursive_ast = format!("{}x = x", "not ".repeat(63));
                assert!(parse_expr_str(&recursive_ast, span).is_ok());
                let host = include_str!("../fixture/recruitment.h.ts");
                let index = host.rfind("})").unwrap();
                let nested = format!("{} nestedTest: {}1{},\n{}", &host[..index], "[".repeat(63), "]".repeat(63), &host[index..]);
                let error = load_str(&nested, Form::HTs).err().expect("unknown nested host key");
                assert_eq!(error[0].code, "UNKNOWN_KEY");
                let wide = std::iter::repeat_n("true", MAX_EXPR_NODES - 1).collect::<Vec<_>>().join(" and ");
                assert!(parse_expr_str(&wide, span).is_ok());
                let base = include_str!("../fixture/recruitment.aip");
                let old = "predicate active(r: Recruitment) = r.status = PUBLISHED and r.periodEnd >= now";
                let definitions = (0..63)
                    .map(|i| format!("predicate p{i}() = p{}()", i + 1))
                    .chain(std::iter::once("predicate p63() = true".to_string()))
                    .collect::<Vec<_>>()
                    .join("\n");
                let chain = base.replacen(old, &format!("predicate active(r: Recruitment) = p0()\n{definitions}"), 1);
                assert!(load_str(&chain, Form::A).is_ok());
            })
            .unwrap();
        thread.join().unwrap();
        return;
    }

    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "small_stack_child_process_parses_supported_nesting_without_overflow"])
        .env("AIP_SMALL_STACK_CHILD", "1")
        .status()
        .unwrap();
    assert!(status.success(), "small-stack child exited with {status}");
}
