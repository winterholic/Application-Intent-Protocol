#![allow(clippy::unwrap_used, clippy::expect_used)]

use aip_ir::Shape;
use aip_sema::pipeline::check_source;
use serde_json::Value;

const FIXTURE: &str = include_str!("../../../conformance/semantic-v1.json");
const SOURCE: &str = r#"
entity Account { label: Text }
entity Entry { account: Account note: Text? amount: Decimal? }
query Conformance(account: Account) {
  allow public
  from Entry e
  where e.account = account
  select { note }
}
query Conditional(account: Account) {
  allow public
  from Entry e
  where e.account = account and e.note = "kept"
  select { id }
}
query EmptyCount(account: Account) {
  allow public
  from Entry e
  group by e.account
  select { count: count(Entry x where x.account = account) }
}
"#;

#[test]
fn shared_fixture_static_ir_shape_contract() {
    // This checks fixture consistency and Core IR shape only. Runtime values are
    // exercised against PostgreSQL in aip-cli/tests/core_semantic_fixture.rs.
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let expected = &fixture["expected"];
    let supported_by = fixture["supportedBy"].as_object().expect("fixture support matrix");
    let unsupported = fixture["unsupported"].as_object().expect("fixture unsupported matrix");
    let expected_keys = expected.as_object().expect("fixture expected values");
    assert_eq!(
        supported_by.keys().collect::<std::collections::BTreeSet<_>>(),
        expected_keys.keys().collect::<std::collections::BTreeSet<_>>(),
        "every expected key must have an explicit path support declaration"
    );
    for key in expected_keys.keys() {
        for path in ["core", "service"] {
            let supported = supported_by[key].as_array().unwrap().iter().any(|value| value == path);
            let unsupported = unsupported.get(key).is_some_and(|entry| entry.get(path).is_some());
            assert_ne!(supported, unsupported, "{key} must be exactly supported or explicitly unsupported by {path}");
        }
    }
    let cases = fixture["cases"].as_array().unwrap();
    assert!(cases.iter().any(|row| row["note"].is_null()), "fixture must include an explicit nullable input");
    assert!(cases.iter().any(|row| row["note"].as_str() == Some("kept")), "fixture must include a present value");
    assert_eq!(
        cases.iter().find(|row| row["id"] == "12").unwrap()["votes"].as_array().unwrap().len(),
        expected["emptyCount"].as_u64().unwrap() as usize
    );
    assert_eq!(fixture["unsupported"]["avgAggregate"]["service"], "unsupported");
    assert_eq!(fixture["unsupported"]["decimalAggregates"]["service"], "unsupported");
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 3);

    let checked = check_source(SOURCE);
    assert!(!checked.has_errors(), "fixture Core source diagnostics: {:?}", checked.diagnostics);
    let (core, _) = checked.core().expect("fixture lowers through semantic analysis to Core IR");
    let aip_ir::Intent::Query(query) = &core.intents["Conformance"] else { panic!("expected query intent") };
    let Shape::List { of, .. } = &query.output else { panic!("expected list output") };
    let Shape::Object { fields, .. } = of.as_ref() else { panic!("expected object output") };
    assert!(
        matches!(fields.iter().find(|(field, _)| field == "note").unwrap().1, Shape::Value { nullable: true, .. }),
        "selected nullable fixture field must retain JSON null: {fields:?}"
    );
    assert!(
        expected["missingFields"].as_array().unwrap().iter().all(|missing| fields.iter().all(|(name, _)| name != missing.as_str().unwrap())),
        "unselected fields must be absent from Core output shape: {fields:?}"
    );
    let aip_ir::Intent::Query(count_query) = &core.intents["EmptyCount"] else { panic!("expected aggregate query intent") };
    let Shape::List { of, .. } = &count_query.output else { panic!("expected aggregate list output") };
    let Shape::Object { fields, .. } = of.as_ref() else { panic!("expected aggregate object output") };
    assert!(
        matches!(fields.iter().find(|(field, _)| field == "count").unwrap().1, Shape::Value { nullable: false, .. }),
        "empty count must remain non-null: {fields:?}"
    );
}
