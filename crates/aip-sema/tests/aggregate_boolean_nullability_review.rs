use aip_sema::pipeline::check_source;

fn bool_command(expression: &str) -> String {
    format!(
        "entity Account {{ flag: Bool }}\nentity Entry {{ account: Account amount: Decimal }}\ncommand Check(account: Account) {{ allow public do {{ set account.flag = {expression} }} }}"
    )
}

#[test]
fn null_safe_boolean_operators_remain_assignable_to_required_bool() {
    let aggregate = "min(Entry e where e.account = account: e.amount)";
    let cases = [
        format!("{aggregate} = null"),
        format!("{aggregate} != 0"),
        format!("({aggregate} > 0) and true"),
        format!("({aggregate} > 0) or false"),
        format!("not ({aggregate} > 0)"),
    ];
    let invalid: Vec<_> = cases
        .iter()
        .filter_map(|expression| {
            let checked = check_source(&bool_command(expression));
            checked.has_errors().then_some((expression, checked.diagnostics))
        })
        .collect();
    assert!(invalid.is_empty(), "IS NULL, IS DISTINCT FROM and fail-closed boolean operators return non-null Bool: {invalid:?}");
}

#[test]
fn nullable_range_membership_cannot_be_written_to_required_bool() {
    let aggregate = "min(Entry e where e.account = account: e.amount)";
    let checked = check_source(&bool_command(&format!("{aggregate} in [0, 10)")));
    assert!(checked.diagnostics.iter().any(|d| d.code == "POC.E201"), "a NULL value in a SQL range comparison stays NULL: {:?}", checked.diagnostics);

    let nullable_target = bool_command(&format!("{aggregate} in [0, 10)")).replace("flag: Bool", "flag: Bool?");
    let checked = check_source(&nullable_target);
    assert!(checked.diagnostics.is_empty(), "a nullable Bool target accepts the SQL NULL result: {:?}", checked.diagnostics);
}

#[test]
fn nullable_list_membership_cannot_be_written_to_required_bool() {
    let aggregate = "min(Entry e where e.account = account: e.amount)";
    let checked = check_source(&bool_command(&format!("{aggregate} in [0, 10]")));
    assert!(checked.diagnostics.iter().any(|d| d.code == "POC.E201"), "a NULL value in SQL IN stays NULL: {:?}", checked.diagnostics);
}

#[test]
fn nullable_set_membership_cannot_be_written_to_required_bool() {
    let aggregate = "min(Entry e where e.account = account: e.amount)";
    let source = format!(
        "entity Account {{ flag: Bool }}\nentity Entry {{ account: Account amount: Decimal }}\ncommand Check(account: Account, allowed: Set<Decimal> max 5) {{ allow public do {{ set account.flag = {aggregate} in allowed }} }}"
    );
    let checked = check_source(&source);
    assert!(checked.diagnostics.iter().any(|d| d.code == "POC.E201"), "a NULL value in SQL set membership stays NULL: {:?}", checked.diagnostics);
}

#[test]
fn null_propagating_builtin_preserves_aggregate_nullability() {
    let source = "entity Account { days: Int }\nentity Entry { account: Account occurred: Time }\ncommand Check(account: Account) { allow public do { set account.days = days_until(min(Entry e where e.account = account: e.occurred)) } }";
    let checked = check_source(source);
    assert!(checked.diagnostics.iter().any(|d| d.code == "POC.E201"), "days_until(NULL) is SQL NULL: {:?}", checked.diagnostics);
}
