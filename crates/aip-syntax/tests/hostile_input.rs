//! Hostile definitions must produce diagnostics, never a stack overflow or an arithmetic overflow panic.
use aip_syntax::lexer::{Tok, lex};
use aip_syntax::parser::parse_file;

fn entity_with(invariant: &str) -> String {
    format!("entity Item {{\n  stock: Int\n  invariant deep: {invariant}\n}}\n")
}

#[test]
fn deeply_nested_parentheses_are_a_diagnostic_not_a_stack_overflow() {
    let source = entity_with(&format!("{}stock >= 0{}", "(".repeat(5000), ")".repeat(5000)));
    assert!(parse_file(&source).is_err());
}

#[test]
fn long_prefix_operator_chains_are_a_diagnostic() {
    let source = entity_with(&format!("{}stock >= 0", "- ".repeat(5000)));
    assert!(parse_file(&source).is_err());
}

#[test]
fn huge_size_literals_saturate_instead_of_overflowing() {
    let toks = lex("99999999999GB").expect("lexes");
    assert!(matches!(toks[0].tok, Tok::Size(size) if size >= 1 << 40));
}
