use spike_v1_fixture::{diag::Span, load_str, parser::parse_expr_str, Form};

#[test]
fn ordinary_parentheses_negations_and_call_arguments_keep_their_meaning() {
    let span = Span { line: 17, col: 6 };
    for expression in [
        format!("{}x = x{}", "(".repeat(63), ")".repeat(63)),
        format!("{}x = x", "not ".repeat(63)),
        format!("{}x{}", "p(".repeat(63), ")".repeat(63)),
    ] {
        assert!(parse_expr_str(&expression, span).is_ok(), "supported expression: {expression}");
    }
    for expression in [
        format!("{}x = x{}", "(".repeat(512), ")".repeat(512)),
        format!("{}x = x", "not ".repeat(512)),
        format!("{}x{}", "p(".repeat(512), ")".repeat(512)),
    ] {
        let error = parse_expr_str(&expression, span).unwrap_err();
        assert_eq!(error.code, "PARSE_NESTING");
        assert_eq!(error.span.line, 17);
        assert!(error.span.col >= 6);
    }
}

#[test]
fn supported_host_literal_depth_reaches_the_existing_shape_diagnostic() {
    let ts = include_str!("../fixture/recruitment.h.ts");
    let index = ts.rfind("})").unwrap();
    let value = format!("{}1{}", "[".repeat(63), "]".repeat(63));
    let source = format!("{} nestedTest: {value},\n{}", &ts[..index], &ts[index..]);
    let errors = load_str(&source, Form::HTs).err().expect("unknown key after parsing the supported literal");
    assert_eq!(errors[0].code, "UNKNOWN_KEY");
}
