//! `club: Club.Id` and `club: Club` both read as "belongs to a club", but only the second is a foreign key.
//! The development check says so; the definition stays valid and its contract does not change.
use serde_json::json;
use spike_v11_dev_checks::load_checked;
use spike_v1_fixture::{digest, Form};

const BASE: &str = "actor Member\nresource Member { fields { id: Id } }\nresource Club { fields { id: Id; name: Text } }\n";

fn advisories(extra: &str, dev_checks: bool) -> Vec<(String, String)> {
    let checked = load_checked(&format!("{BASE}{extra}"), Form::A, &json!({"devChecks": dev_checks})).expect("valid");
    checked.advisories.iter().map(|a| (a.code.to_string(), a.anchor.clone())).collect()
}

#[test]
fn an_id_field_without_a_reference_is_reported_and_a_reference_is_not() {
    let id_field = "resource Note { fields { id: Id; club: Club.Id; owner: Member.Id? } }\n";
    let found = advisories(id_field, true);
    assert_eq!(
        found,
        vec![
            ("ADVISORY_ID_WITHOUT_REFERENCE".to_string(), "resource:Note.club".to_string()),
            ("ADVISORY_ID_WITHOUT_REFERENCE".to_string(), "resource:Note.owner".to_string())
        ]
    );
    assert!(advisories("resource Note { fields { id: Id; club: Club; owner: Member? } }\n", true).is_empty());
    assert!(advisories(id_field, false).is_empty(), "advice is opt-in");
}

#[test]
fn advice_never_changes_the_contract_and_the_two_spellings_really_differ() {
    let (a, b) = ("resource Note { fields { id: Id; club: Club } }\n", "resource Note { fields { id: Id; club: Club.Id } }\n");
    let on = |extra: &str| load_checked(&format!("{BASE}{extra}"), Form::A, &json!({"devChecks": true})).expect("valid");
    let off = |extra: &str| load_checked(&format!("{BASE}{extra}"), Form::A, &json!({"devChecks": false})).expect("valid");
    assert_eq!(digest(&on(b).output.execution), digest(&off(b).output.execution));
    // the reason for the advice: the two declarations are different contracts
    assert_ne!(digest(&on(a).output.execution), digest(&on(b).output.execution));
}
