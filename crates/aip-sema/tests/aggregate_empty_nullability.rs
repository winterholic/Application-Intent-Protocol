use aip_ir::Shape;
use aip_sema::pipeline::check_source;

const ASSIGNMENT_SOURCE: &str = r#"
entity Account { minimum: Decimal }
entity Entry {
  account: Account
  amount: Decimal
}
command Recompute(account: Account) {
  allow public
  do { set account.minimum = min(Entry e where e.account = account: e.amount) }
}
"#;

const GROUPED_QUERY_SOURCE: &str = r#"
entity Account { minimum: Decimal }
entity Entry {
  account: Account
  amount: Decimal
}
query MinimumByAccount() {
  allow public
  from Entry e
  group by e.account
  select { account: e.account
    minimum: min(Entry candidate where candidate.account = e.account and candidate.amount > 0: candidate.amount) }
}
"#;

#[test]
fn min_of_empty_set_is_not_assignable_to_a_required_field() {
    let checked = check_source(ASSIGNMENT_SOURCE);

    assert!(
        checked.diagnostics.iter().any(|d| d.code == "POC.E201"),
        "min over an empty set is SQL NULL and must not be assigned to required Decimal; diagnostics: {:?}",
        checked.diagnostics
    );
}

#[test]
fn grouped_min_output_shape_is_nullable_for_empty_input() {
    let checked = check_source(GROUPED_QUERY_SOURCE);
    assert!(!checked.has_errors(), "grouped query fixture must pass: {:?}", checked.diagnostics);
    let (core, _) = checked.core().expect("valid grouped query should lower");
    let aip_ir::Intent::Query(query) = &core.intents["MinimumByAccount"] else { panic!("expected query intent") };
    let Shape::List { of, .. } = &query.output else { panic!("expected list output") };
    let Shape::Object { fields, .. } = of.as_ref() else { panic!("expected object rows") };
    let minimum = fields.iter().find(|(name, _)| name == "minimum").unwrap().1.clone();
    assert!(
        matches!(minimum, Shape::Value { nullable: true, .. }),
        "grouped min can be NULL when a group's input is empty, output shape must mark it nullable: {minimum:?}"
    );
}

#[test]
fn nullable_aggregate_results_are_assignable_to_nullable_fields() {
    let source = r#"
entity Account { minimum: Decimal? }
entity Entry {
  account: Account
  amount: Decimal
}
command Recompute(account: Account) {
  allow public
  do { set account.minimum = min(Entry e where e.account = account: e.amount) }
}
"#;

    let checked = check_source(source);
    assert!(checked.diagnostics.is_empty(), "nullable Decimal target should accept nullable min: {:?}", checked.diagnostics);
}

#[test]
fn max_and_avg_of_empty_sets_are_not_assignable_to_required_fields() {
    for aggregate in ["max", "avg"] {
        let source = format!(
            r#"
entity Account {{ total: Decimal }}
entity Entry {{
  account: Account
  amount: Decimal
}}
command Recompute(account: Account) {{
  allow public
  do {{ set account.total = {aggregate}(Entry e where e.account = account: e.amount) }}
}}
"#
        );
        let checked = check_source(&source);
        assert!(
            checked.diagnostics.iter().any(|d| d.code == "POC.E201"),
            "{aggregate} over an empty set is SQL NULL and must not be assigned to required Decimal; diagnostics: {:?}",
            checked.diagnostics
        );
    }
}

#[test]
fn grouped_aggregate_output_nullability_matches_empty_set_sql_contract() {
    let source = r#"
entity Account { label: Text }
entity Entry {
  account: Account
  amount: Decimal
}
query AggregateByAccount() {
  allow public
  from Entry e
  group by e.account
  select {
    minimum: min(Entry x where x.account = e.account: x.amount)
    maximum: max(Entry x where x.account = e.account: x.amount)
    average: avg(Entry x where x.account = e.account: x.amount)
    total: sum(Entry x where x.account = e.account: x.amount)
    count: count(Entry x where x.account = e.account)
  }
}
"#;
    let checked = check_source(source);
    assert!(checked.diagnostics.is_empty(), "query fixture must pass: {:?}", checked.diagnostics);
    let (core, _) = checked.core().expect("valid grouped query should lower");
    let aip_ir::Intent::Query(query) = &core.intents["AggregateByAccount"] else { panic!("expected query intent") };
    let Shape::List { of, .. } = &query.output else { panic!("expected list output") };
    let Shape::Object { fields, .. } = of.as_ref() else { panic!("expected object rows") };

    for name in ["minimum", "maximum", "average"] {
        assert!(
            matches!(fields.iter().find(|(field, _)| field == name).unwrap().1, Shape::Value { nullable: true, .. }),
            "empty-set {name} must be nullable: {:?}",
            fields
        );
    }
    for name in ["total", "count"] {
        assert!(
            matches!(fields.iter().find(|(field, _)| field == name).unwrap().1, Shape::Value { nullable: false, .. }),
            "empty-set {name} uses the established zero contract: {:?}",
            fields
        );
    }
}
