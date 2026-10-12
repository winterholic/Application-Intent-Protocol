//! `club: Club` is a foreign key; `club: Club.Id` is a bare bigint. The authoring card and the
//! ADVISORY_ID_WITHOUT_REFERENCE advice depend on this difference, so it is pinned at the DDL.
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::sqlgen;

const BASE: &str = "actor Member\nresource Member { fields { id: Id } }\nresource Club { fields { id: Id; name: Text } }\n";

fn ddl(note_fields: &str) -> String {
    let facts = load_str(&format!("{BASE}resource Note {{ fields {{ {note_fields} }} }}\n"), Form::A).expect("valid").execution;
    sqlgen::ddl(&facts).expect("ddl").join("\n")
}

#[test]
fn a_reference_is_a_foreign_key_and_an_id_value_is_not() {
    let reference = ddl("id: Id; club: Club");
    let id_value = ddl("id: Id; club: Club.Id");
    let note = |ddl: &str| ddl.lines().skip_while(|l| !l.contains("note")).collect::<Vec<_>>().join("\n");
    assert!(note(&reference).contains("club_id bigint REFERENCES"), "{}", note(&reference));
    assert!(!note(&id_value).contains("REFERENCES"), "{}", note(&id_value));
    assert!(note(&id_value).contains("club bigint"), "{}", note(&id_value));
}
