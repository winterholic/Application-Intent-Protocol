//! `club: Club` is a foreign key. `club: Club.Id` in a resource's `fields` is a definition error
//! (founder decision 2026-10-13), so the DDL can no longer contain a bare club id column by that spelling.
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::sqlgen;

const BASE: &str = "actor Member\nresource Member { fields { id: Id } }\nresource Club { fields { id: Id; name: Text } }\n";

#[test]
fn a_reference_is_a_foreign_key_and_the_id_spelling_is_rejected() {
    let facts = load_str(&format!("{BASE}resource Note {{ fields {{ id: Id; club: Club }} }}\n"), Form::A).expect("valid").execution;
    let ddl = sqlgen::ddl(&facts).expect("ddl").join("\n");
    let note: String = ddl.lines().skip_while(|l| !l.contains("note")).collect::<Vec<_>>().join("\n");
    assert!(note.contains("club_id bigint REFERENCES"), "{note}");

    let errs = load_str(&format!("{BASE}resource Note {{ fields {{ id: Id; club: Club.Id }} }}\n"), Form::A).err().expect("rejected");
    assert!(errs.iter().any(|d| d.code == "DEF.ID_FIELD_NEEDS_REFERENCE"), "{errs:?}");
}
