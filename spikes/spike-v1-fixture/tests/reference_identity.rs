use serde_json::json;
use spike_v1_fixture::{load_str, Form};

#[test]
fn references_require_an_explicit_target_identity_in_every_definition_form() {
    for ty in ["Link", "Link?"] {
        let source = format!("resource Member {{ fields {{ id: Id }} }} actor Member resource Link {{ fields {{ member: Member }} }} resource Item {{ fields {{ id: Id; link: {ty} }} }}");
        let host = json!({"actor":"Member","resources":{"Member":{"fields":{"id":"Id"}},"Link":{"fields":{"member":"Member"}},"Item":{"fields":{"id":"Id","link":ty}}}});
        for (form, wrapped) in [
            (Form::A, source.clone()),
            (Form::ETs, format!("import {{ aip }} from '@aip/define'; export default aip`{source}`;")),
            (Form::EPy, format!("from aip.define import aip\nSPEC = aip(\"\"\"{source}\"\"\")")),
            (Form::HTs, format!("import {{ define }} from '@aip/define'; export const spec = define({host});")),
            (Form::HPy, format!("from aip.define import define\nSPEC = define({host})")),
        ] {
            let errors = load_str(&wrapped, form).err().expect("Ref cannot point to a resource without id");
            assert!(
                errors.iter().any(|error| error.code == "TYPE_MISMATCH" && error.msg.contains("Link") && error.msg.contains("id")),
                "{form:?}: {errors:?}"
            );
        }
    }
    let self_ref = "resource Member { fields { id: Id } } actor Member resource Link { fields { parent: Link? } }";
    assert!(load_str(self_ref, Form::A).is_err());
}

#[test]
fn private_relationships_remain_usable_without_id() {
    let source = "resource Member { fields { id: Id } } actor Member predicate owned(link: Link) = link.member = actor resource Link { fields { member: Member } check ownedLink when owned(this) } resource Item { fields { id: Id; legacy: Link.Id? } rows read when exists Link where member = actor }";
    assert!(load_str(source, Form::A).is_ok(), "exists sources and opaque typed IDs do not require a target FK");
    let referenced = "resource Member { fields { id: Id } } actor Member resource Link { fields { id: Id; member: Member } } resource Item { fields { id: Id; link: Link? } }";
    assert!(load_str(referenced, Form::A).is_ok());
}
