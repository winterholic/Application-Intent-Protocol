//! `exists R where <expr>` takes the whole `or` chain to its right as the row condition, exactly as the grammar
//! says (`set-expr = source ident? ('where' expr)?`, `expr = or`). A disjunct that never mentions the row
//! (`viewer.primary_group.mod`) is therefore only reachable when some row of `R` exists. The FlaskBB port hit this
//! as "primary-group Moderator can delete topics only while `user_group` is non-empty". This pins the parse so the
//! scope rule stays visible; the declaration fix is to parenthesize the `exists`.
use spike_v1_fixture::ast::Expr;
use spike_v1_fixture::diag::Span;
use spike_v1_fixture::parser::parse_expr_str;

fn parse(src: &str) -> Expr {
    parse_expr_str(src, Span::default()).unwrap_or_else(|d| panic!("{src}: {d:?}"))
}

#[test]
fn exists_where_absorbs_the_trailing_or_chain() {
    match parse("viewer.mod or exists UserGroup where user = viewer or viewer.admin") {
        Expr::Or(outer) => {
            assert_eq!(outer.len(), 2, "outer or has the leading disjunct and the exists only");
            match &outer[1] {
                Expr::Exists(source, condition, _) => {
                    assert_eq!(source, "UserGroup");
                    assert!(
                        matches!(**condition, Expr::Or(ref inner) if inner.len() == 2),
                        "viewer.admin sits inside the exists condition: {condition:?}"
                    );
                }
                other => panic!("second disjunct should be the exists: {other:?}"),
            }
        }
        other => panic!("expected an outer or: {other:?}"),
    }
}

#[test]
fn parenthesized_exists_leaves_the_trailing_disjunct_outside() {
    match parse("viewer.mod or (exists UserGroup where user = viewer) or viewer.admin") {
        Expr::Or(outer) => {
            assert_eq!(outer.len(), 3, "three sibling disjuncts: {outer:?}");
            assert!(matches!(&outer[1], Expr::Exists(_, condition, _) if matches!(**condition, Expr::Cmp(..))), "{:?}", outer[1]);
            assert!(matches!(&outer[2], Expr::Path(..)), "{:?}", outer[2]);
        }
        other => panic!("expected an outer or: {other:?}"),
    }
}
