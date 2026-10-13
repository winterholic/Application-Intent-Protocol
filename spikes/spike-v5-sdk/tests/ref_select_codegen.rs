//! Founder decision 2026-10-13 (O-01): a directly selected reference is null when the caller cannot read the target
//! row, so even a required `team: Team` is typed `Id<"Team"> | null`. `refIds` keeps the declared nullability.
use spike_v1_fixture::{load_str, Form};
use spike_v5_sdk::{contract_fingerprint, contract_ts};

fn source(declaration: &str) -> String {
    format!(
        r#"
resource Member {{ fields {{ id: Id }} }}
actor Member
resource Team {{ fields {{ id: Id; owner: Member }} rows read when owner = actor }}
resource Task {{
  fields {{ id: Id; team: Team?; reqTeam: Team }}
  rows read when true
  expose read {{
    select id, team, reqTeam
    {declaration}
    budget {{ rows 10; depth 1; deadline 2s; cost 100 }}
  }}
}}
"#
    )
}

fn facts(declaration: &str) -> serde_json::Value {
    load_str(&source(declaration), Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution
}

#[test]
fn a_selected_reference_is_nullable_in_the_generated_type_unless_declared_with_ref_ids() {
    let default = contract_ts(&facts(""));
    assert!(default.contains("reqTeam: Id<\"Team\"> | null;"), "a required reference becomes nullable by default:\n{default}");
    assert!(default.contains("team: Id<\"Team\"> | null;"), "{default}");

    let declared = contract_ts(&facts("refIds team, reqTeam"));
    assert!(declared.contains("reqTeam: Id<\"Team\">;"), "a declared required reference stays non-null:\n{declared}");
    assert!(declared.contains("team: Id<\"Team\"> | null;"), "a declared optional reference keeps its nullability:\n{declared}");
    assert!(!declared.contains("reqTeam: Id<\"Team\"> | null;"), "{declared}");
}

#[test]
fn the_default_changes_the_contract_fingerprint() {
    assert_ne!(contract_fingerprint(&facts("")), contract_fingerprint(&facts("refIds team, reqTeam")));
}
