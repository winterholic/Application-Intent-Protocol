use serde_json::json;
use spike_v1_fixture::{load_str, Form};

fn assert_rejected(source: &str, code: &str) {
    for (form, wrapper) in [
        (Form::A, source.to_owned()),
        (Form::ETs, format!("import {{ aip }} from '@aip/define'; export default aip`{source}`;")),
        (Form::EPy, format!("from aip.define import aip\nSPEC = aip(\"\"\"{source}\"\"\")")),
    ] {
        let errors = load_str(&wrapper, form).err().expect("invalid resource identity must be rejected");
        assert!(errors.iter().any(|error| error.code == code), "{form:?}: {errors:?}");
    }
}

#[test]
fn explicitly_declared_ids_are_non_nullable_and_belong_to_their_resource() {
    for ty in ["Int", "Text", "Id?", "Member.Id", "Item"] {
        let source = format!("resource Member {{ fields {{ id: Id }} }} actor Member resource Item {{ fields {{ id: {ty} }} }}");
        assert_rejected(&source, "TYPE_MISMATCH");
        let object = json!({"actor":"Member","resources":{"Member":{"fields":{"id":"Id"}},"Item":{"fields":{"id":ty}}}});
        for (form, wrapper) in [
            (Form::HTs, format!("import {{ define }} from '@aip/define'; export const spec = define({object});")),
            (Form::HPy, format!("from aip.define import define\nSPEC = define({object})")),
        ] {
            let errors = load_str(&wrapper, form).err().expect("invalid host ID declaration must fail");
            assert!(errors.iter().any(|error| error.code == "TYPE_MISMATCH"), "{form:?}: {errors:?}");
        }
    }
    let valid = "resource Member { fields { id: Member.Id } } actor Member resource Item { fields { id: Item.Id; previous: Item.Id? } }";
    assert!(load_str(valid, Form::A).is_ok());
    let join = "resource Member { fields { id: Id } } actor Member resource Membership { fields { member: Member; active: Bool } }";
    assert!(load_str(join, Form::A).is_ok(), "private relationship resources may omit id");
}

#[test]
fn transitions_cannot_move_the_primary_id_away_from_their_locked_target() {
    let source = "resource Member { fields { id: Id } } actor Member resource Item { fields { id: Id; replacement: Item.Id } transition rekey { from true; to id = replacement; allow true } expose apply rekey { target id; bulk maxRows 1 } }";
    assert_rejected(source, "UNSUPPORTED_EFFECT");
}
