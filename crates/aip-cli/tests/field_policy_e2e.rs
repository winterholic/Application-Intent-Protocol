//! Field-level `visible to` must hold wherever the value is used, not only when the field is selected by name:
//! a computed selection, a filter and a sort key may not reveal a value the caller cannot read.
//!
//! Requires a local PostgreSQL; set AIP_TEST_ADMIN_URL to override `postgres://localhost/postgres`.

mod common;

use common::ok;
use serde_json::{Value, json};

const APP: &str = "
use auth
actor Member via auth.oidc(kakao)

entity Member personal {
  nickname: Text(2..20, trim)
  phone: Phone? personal visible to self
  unique (nickname) else NICKNAME_TAKEN
}

command Register(nickname: Text(2..20, trim), phone: Phone?) idempotent {
  allow public
  do { insert Member { nickname, phone } as m }
  returns m { id nickname }
}

query Directory() {
  allow authenticated
  from Member m
  sort by m.nickname asc
  page 20 by keyset
  select { id nickname phone copied: m.phone }
}

query ByPhone(q: Phone) {
  allow authenticated
  from Member m
  where m.phone = q
  sort by m.nickname asc
  page 20 by keyset
  select { id nickname }
}

query ByPhoneOrder() {
  allow authenticated
  from Member m
  sort by m.phone asc
  page 20 by keyset
  select { id nickname }
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

fn rows(v: &Value) -> Vec<Value> {
    v["items"].as_array().cloned().unwrap_or_else(|| v.as_array().cloned().expect("rows"))
}

#[tokio::test]
async fn hidden_phone_is_hidden_in_computed_items_filters_and_sort_keys() {
    let e = common::setup_program("aip_e2e_field_policy", compile()).await;
    let alice =
        ok(&e, "Register", None, json!({"nickname": "alice", "phone": "01012345678"}), Some("reg-alice")).await["id"].as_str().unwrap().to_string();
    let bob = ok(&e, "Register", None, json!({"nickname": "bob", "phone": "01087654321"}), Some("reg-bob")).await["id"].as_str().unwrap().to_string();

    let as_alice = ok(&e, "Directory", Some(&alice), json!({}), None).await;
    let list = rows(&as_alice);
    let find = |nick: &str| list.iter().find(|r| r["nickname"] == nick).cloned().expect("row");
    assert_eq!(find("alice")["copied"], find("alice")["phone"], "the owner sees the same value either way");
    assert!(!find("alice")["copied"].is_null(), "the owner reads their own phone");
    assert!(find("bob")["phone"].is_null(), "selecting the field by name already hides it");
    assert!(find("bob")["copied"].is_null(), "a computed item must not copy a value the caller cannot read: {as_alice}");

    // filtering by the hidden value must not tell who owns it
    let hit = ok(&e, "ByPhone", Some(&alice), json!({"q": "01087654321"}), None).await;
    assert!(rows(&hit).is_empty(), "alice must not find bob by his hidden phone: {hit}");
    let own = ok(&e, "ByPhone", Some(&alice), json!({"q": "01012345678"}), None).await;
    assert_eq!(rows(&own).len(), 1, "alice still finds herself: {own}");

    // ordering by the hidden value must not follow it: with every hidden value equal, ties break by id
    let ordered = ok(&e, "ByPhoneOrder", Some(&bob), json!({}), None).await;
    let names: Vec<_> = rows(&ordered).iter().map(|r| r["nickname"].as_str().unwrap().to_string()).collect();
    assert_eq!(names, ["bob", "alice"].map(String::from), "bob sees his own phone (…321) before alice's hidden value, which sorts last as NULL");
}
