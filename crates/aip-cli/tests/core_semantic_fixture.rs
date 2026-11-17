//! Executes the shared semantic fixture through Core IR, PostgreSQL planning,
//! and the Core runtime Engine. This is the runtime counterpart to the static
//! output-shape checks in aip-sema/tests/shared_semantic_fixture.rs.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use aip_runtime::engine::{Call, Engine};
use serde_json::{Value, json};
use std::sync::Arc;

const FIXTURE: &str = include_str!("../../../conformance/semantic-v1.json");
const SOURCE: &str = r#"
entity Account { label: Text }
entity Entry {
  account: Account
  fixtureKey: Text
  title: Text
  note: Text?
  amount: Decimal(10,2)?
  phase: Text
  score: Decimal(10,2)
  checked: Bool
}
query Conformance(account: Account) {
  allow public
  from Entry e
  where e.account = account
  sort by e.score asc, e.fixtureKey asc
  select { fixtureKey note amount voteCount: count(Vote v where v.entry = e) }
}
query NullEquality(account: Account) {
  allow public
  from Entry e
  where e.account = account and e.note = "kept"
  select { fixtureKey }
}
query EmptyAggregates(account: Account) {
  allow public
  from Account a
  where a = account
  select {
    count: count(Entry x where x.account = account)
    sum: sum(Entry x where x.account = account: x.amount)
    min: min(Entry x where x.account = account: x.amount)
    max: max(Entry x where x.account = account: x.amount)
  }
}
entity Vote { entry: Entry }
"#;

const ACCOUNT_ID: &str = "00000000-0000-4000-8000-000000000042";

async fn setup() -> Arc<Engine> {
    let checked = aip_sema::pipeline::check_source(SOURCE);
    assert!(!checked.has_errors(), "Core fixture diagnostics: {:?}", checked.diagnostics);
    let (core, map) = checked.into_core().expect("fixture lowers to Core IR");
    let compiled = aip_pg::compile(&core, &map);
    assert!(!compiled.diagnostics.iter().any(|d| d.is_error()), "PG plan diagnostics: {:?}", compiled.diagnostics);
    let db = format!("aip_semantic_fixture_{}", uuid::Uuid::new_v4().simple());
    common::setup_program(&db, compiled.program).await
}

fn call(intent: &str) -> Call {
    Call { intent: intent.into(), input: json!({"account": ACCOUNT_ID}), ..Default::default() }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_fixture_core_runtime_matches_expected_values_on_postgres() {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("valid semantic fixture");
    let expected = &fixture["expected"];
    let cases = fixture["cases"].as_array().expect("fixture cases");
    let engine = setup().await;
    let conn = engine.pool.get().await.expect("Core fixture database connection");
    conn.execute("INSERT INTO account (id, label) VALUES ($1, 'fixture')", &[&uuid::Uuid::parse_str(ACCOUNT_ID).unwrap()])
        .await
        .expect("insert fixture account");

    for row in cases {
        let key = row["id"].as_str().expect("fixture id");
        let title = row["title"].as_str().expect("fixture title");
        let note = row["note"].as_str();
        let amount = row["amount"].as_str();
        let phase = row["phase"].as_str().expect("fixture phase");
        let score = row["score"].as_str().expect("fixture score");
        let checked = row["checked"].as_bool().expect("fixture checked flag");
        conn.execute(
            "INSERT INTO entry (account_id, fixture_key, title, note, amount, phase, score, checked) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            &[&uuid::Uuid::parse_str(ACCOUNT_ID).unwrap(), &key, &title, &note, &amount, &phase, &score, &checked],
        )
        .await
        .expect("insert fixture entry");
        for _ in row["votes"].as_array().expect("fixture votes") {
            conn.execute("INSERT INTO vote (entry_id) SELECT id FROM entry WHERE fixture_key = $1", &[&key]).await.expect("insert fixture vote");
        }
    }

    let rows = engine.call(call("Conformance")).await.expect("execute Core conformance query").data;
    let rows = rows.as_array().expect("Core query returns rows");
    assert_eq!(
        rows.iter().map(|row| row["fixtureKey"].as_str().unwrap()).collect::<Vec<_>>(),
        expected["sortIds"].as_array().unwrap().iter().map(|id| id.as_str().unwrap()).collect::<Vec<_>>(),
        "Core runtime explicit sort must match fixture order"
    );
    assert_eq!(rows[0]["note"], expected["null"], "nullable value reaches Core runtime as JSON null");
    assert_eq!(rows[1]["note"], expected["value"], "present value reaches Core runtime unchanged");
    assert_eq!(rows[0]["amount"], expected["decimalExact"], "Decimal boundary wire value is exact");
    assert_eq!(rows[0]["voteCount"], 1, "fixture relation selection preserves its non-empty count");
    assert_eq!(rows[1]["voteCount"], expected["emptyCount"], "fixture relation selection preserves its empty count");
    assert!(
        expected["missingFields"].as_array().unwrap().iter().all(|field| rows[0].get(field.as_str().unwrap()).is_none()),
        "unselected fields are absent from Core runtime output: {}",
        rows[0]
    );

    let null_equality = engine.call(call("NullEquality")).await.expect("execute Core NULL predicate query").data;
    let matched = null_equality.as_array().expect("predicate query returns rows");
    assert_eq!(
        matched.iter().map(|row| row["fixtureKey"].as_str().unwrap()).collect::<Vec<_>>(),
        vec!["12"],
        "SQL NULL equality does not match, while non-null equality does"
    );
    assert_eq!(matched.iter().any(|row| row["fixtureKey"] == "11"), expected["nullEqualityMatches"]);
    assert_eq!(matched.iter().any(|row| row["fixtureKey"] == "12"), expected["nonNullEqualityMatches"]);

    let empty_account = "00000000-0000-4000-8000-000000000043";
    conn.execute("INSERT INTO account (id, label) VALUES ($1, 'empty')", &[&uuid::Uuid::parse_str(empty_account).unwrap()])
        .await
        .expect("insert empty aggregate account");
    let empty = engine
        .call(Call { intent: "EmptyAggregates".into(), input: json!({"account": empty_account}), ..Default::default() })
        .await
        .expect("execute Core empty aggregate query")
        .data;
    assert_eq!(empty["count"], expected["emptyCount"], "empty count follows fixture");
    assert_eq!(empty["sum"], expected["emptySum"], "empty sum follows fixture");
    assert_eq!(empty["min"], expected["emptyMin"], "empty min follows fixture");
    assert_eq!(empty["max"], expected["emptyMax"], "empty max follows fixture");
}
