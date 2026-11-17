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
fn shared_fixture_agrees_with_core_ir_nullability_and_predicate_contract() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let expected = &fixture["expected"];
    let cases = fixture["cases"].as_array().unwrap();
    assert!(cases.iter().any(|row| row["note"].is_null()), "fixture must include an explicit nullable input");
    assert!(cases.iter().any(|row| row["note"].as_str() == Some("kept")), "fixture must include a present value");
    assert_eq!(
        cases.iter().find(|row| row["id"] == "12").unwrap()["votes"].as_array().unwrap().len(),
        expected["emptyCount"].as_u64().unwrap() as usize
    );
    assert_eq!(expected["emptyCount"], 0);
    assert_eq!(expected["nullEqualityMatches"], false);
    assert_eq!(expected["nonNullEqualityMatches"], true);
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
