use aip_ir::Shape;
use aip_sema::pipeline::check_source;

fn command(body: &str) -> String {
    format!(
        "entity Account {{ minimum: Decimal }}\nentity Entry {{ account: Account amount: Decimal }}\ncommand Recompute(account: Account) {{ allow public do {{ {body} }} }}"
    )
}

#[test]
fn required_field_rejects_aggregate_null_propagated_through_arithmetic() {
    let checked = check_source(&command("set account.minimum = min(Entry e where e.account = account: e.amount) + 1"));
    assert!(checked.diagnostics.iter().any(|d| d.code == "POC.E201"), "NULL + 1 remains NULL; diagnostics: {:?}", checked.diagnostics);
}

#[test]
fn required_field_rejects_aggregate_null_propagated_through_local_binding() {
    let checked = check_source(&command("let minimum = min(Entry e where e.account = account: e.amount)\nset account.minimum = minimum"));
    assert!(
        checked.diagnostics.iter().any(|d| d.code == "POC.E201"),
        "a local binding must preserve aggregate nullability; diagnostics: {:?}",
        checked.diagnostics
    );
}

#[test]
fn required_insert_field_rejects_empty_aggregate_null() {
    let checked = check_source(&command("insert Account { minimum: min(Entry e where e.account = account: e.amount) }"));
    assert!(
        checked.diagnostics.iter().any(|d| d.code == "POC.E201"),
        "insert and set must enforce the same required-field contract; diagnostics: {:?}",
        checked.diagnostics
    );
}

#[test]
fn grouped_arithmetic_preserves_aggregate_output_nullability() {
    let source = "entity Account { minimum: Decimal }\nentity Entry { account: Account amount: Decimal }\nquery MinimumByAccount() { allow public from Entry e group by e.account select { minimum: min(Entry x where x.account = e.account: x.amount) + 1 } }";
    let checked = check_source(source);
    assert!(!checked.has_errors(), "query must be valid: {:?}", checked.diagnostics);
    let (core, _) = checked.core().expect("query must lower");
    let aip_ir::Intent::Query(query) = &core.intents["MinimumByAccount"] else { panic!("query expected") };
    let Shape::List { of, .. } = &query.output else { panic!("list expected") };
    let Shape::Object { fields, .. } = of.as_ref() else { panic!("object expected") };
    assert!(matches!(&fields[0].1, Shape::Value { nullable: true, .. }), "arithmetic must preserve SQL NULL: {:?}", fields[0].1);
}

#[test]
fn coalesced_empty_aggregate_with_value_fallback_remains_assignable() {
    let checked = check_source(&command("set account.minimum = coalesce(min(Entry e where e.account = account: e.amount), 0)"));
    assert!(!checked.has_errors(), "an explicit non-null fallback must remain supported: {:?}", checked.diagnostics);
}
