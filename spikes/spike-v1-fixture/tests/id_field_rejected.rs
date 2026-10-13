//! Founder decision 2026-10-13: a resource field is never spelled `X.Id`. A reference (`X`) is the single
//! spelling for "belongs to an X"; `X.Id` stays legal only where no database column is involved.
use spike_v1_fixture::{load_str, Form};

const BASE: &str = "actor Member\nresource Member { fields { id: Id } }\nresource Club { fields { id: Id; name: Text } }\n";

fn load(extra: &str) -> Result<(), Vec<spike_v1_fixture::diag::Diag>> {
    load_str(&format!("{BASE}{extra}"), Form::A).map(|_| ())
}

#[test]
fn a_field_declared_as_x_id_is_a_definition_error_with_the_fix() {
    for field in ["club: Club.Id", "club: Club.Id?"] {
        let errs = load(&format!("resource Note {{ fields {{ id: Id; {field} }} }}\n")).expect_err(field);
        let d = errs.iter().find(|d| d.code == "ID_FIELD_NEEDS_REFERENCE").unwrap_or_else(|| panic!("{field}: {errs:?}"));
        assert!(d.msg.contains("club: Club"), "the message names the fix: {}", d.msg);
    }
}

#[test]
fn a_reference_and_non_column_places_still_accept_x_id() {
    load("resource Note { fields { id: Id; club: Club; owner: Member? } }\n").expect("reference field");
    let src =
        "resource Note { fields { id: Id; club: Club }\n  expose read { select id\n    budget { rows 10; depth 1; deadline 2s; cost 100 } } }\n\
               access pick(a: Member, c: Club.Id) = a.id = a.id\n";
    load(src).expect("access argument keeps X.Id");
}

#[test]
fn the_primary_key_is_spelled_id_and_nothing_else() {
    // D-A16 applies to the key too: one meaning, one spelling. `id: Note.Id` used to pass because the
    // column-name exemption skipped the check; the bare `Id` is the only accepted spelling.
    for field in ["id: Note.Id", "id: Club.Id"] {
        let errs = load(&format!("resource Note {{ fields {{ {field} }} }}\n")).expect_err(field);
        let d = errs.iter().find(|d| d.code == "ID_FIELD_NEEDS_REFERENCE").unwrap_or_else(|| panic!("{field}: {errs:?}"));
        assert!(d.msg.contains("id: Id"), "the message names the fix `id: Id`: {}", d.msg);
    }
    load("resource Note { fields { id: Id } }\n").expect("bare Id is the key");
}
