//! Several rules that become true in one transaction run in the order they are declared.
//!
//! Each rule here writes the same field, so the final value shows which rule ran last. The names are chosen
//! so that declaration order, alphabetical order and the order of the rule table's rows all differ.
//! Requires explicit AIP_TEST_DATABASE_URL and AIP_TEST_ADMIN_URL values for a worker-owned isolated PostgreSQL.

mod common;

use common::{ok, sql_one};
use serde_json::json;

const APP: &str = "
use auth
actor Member via auth.oidc(kakao)

entity Member {
  nickname: Text(2..20)
}

entity Item {
  n: Int(0..100)
  label: Text(0..20)
}

rule Zeta on Item i when i.n >= 1 do {
  set i.label = \"zeta\"
}

rule Mid on Item i when i.n >= 1 do {
  set i.label = \"mid\"
}

rule Alpha on Item i when i.n >= 1 do {
  set i.label = \"alpha\"
}

command Make() idempotent {
  allow public
  do { insert Item { n: 1, label: \"\" } as i }
  returns i { id }
}
";

fn compile() -> aip_plan::Program {
    let checked = aip_sema::pipeline::check_source(APP);
    assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
    let (core, map) = checked.into_core().expect("no errors");
    let compiled = aip_pg::compile(&core, &map);
    assert!(!compiled.diagnostics.iter().any(|d| d.is_error()), "{:?}", compiled.diagnostics);
    compiled.program
}

#[tokio::test]
async fn rules_that_fire_together_run_in_declaration_order() {
    let engine = common::setup_program("aip_rule_order_e2e", compile()).await;
    for round in 0..5 {
        let made = ok(&engine, "Make", None, json!({}), Some(&format!("round-{round}"))).await;
        let id = made["id"].as_str().expect("id");
        let label = sql_one(&engine, &format!("SELECT label FROM item WHERE id = '{id}'")).await;
        // the last rule in the source writes last
        assert_eq!(label, json!("alpha"), "round {round}: rules did not run in declaration order");
    }
}
