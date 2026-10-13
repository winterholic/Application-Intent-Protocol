//! An `expose read` without `budget` that no resource traverses cannot be read by anyone. The development
//! check says so before the first read fails with NOT_ROOT_QUERYABLE; the definition itself stays valid.
use serde_json::json;
use spike_v11_dev_checks::load_checked;
use spike_v1_fixture::{digest, Form};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");

fn advisories(src: &str, dev_checks: bool) -> Vec<(String, String)> {
    let checked = load_checked(src, Form::A, &json!({"devChecks": dev_checks})).expect("the definition is valid");
    checked.advisories.iter().map(|a| (a.code.to_string(), a.anchor.clone())).collect()
}

#[test]
fn the_fixture_has_no_unreachable_read() {
    // Club has no budget, but Recruitment traverses it
    assert!(advisories(A, true).is_empty());
}

#[test]
fn a_read_without_budget_that_nothing_traverses_is_reported() {
    let src = format!("{A}\nresource Tag {{\n  fields {{ id: Id; name: Text }}\n  expose read {{ select id, name }}\n}}\n");
    assert_eq!(advisories(&src, true), vec![("DEF.ADVISORY_UNREACHABLE_READ".to_string(), "resource:Tag".to_string())]);
    assert!(advisories(&src, false).is_empty(), "advice is opt-in");
    let with = load_checked(&src, Form::A, &json!({"devChecks": true})).expect("valid");
    let without = load_checked(&src, Form::A, &json!({"devChecks": false})).expect("valid");
    assert_eq!(digest(&with.output.execution), digest(&without.output.execution), "advice never changes the contract");
}

#[test]
fn a_budget_or_a_traverse_makes_the_read_reachable() {
    let with_budget = format!(
        "{A}\nresource Tag {{\n  fields {{ id: Id; name: Text }}\n  expose read {{ select id, name\n    budget {{ rows 10; depth 1; deadline 2s; cost 100 }} }}\n}}\n"
    );
    assert!(advisories(&with_budget, true).is_empty());
}
