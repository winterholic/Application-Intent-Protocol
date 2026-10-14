use spike_v1_fixture::{load_str, Form};
#[test]
fn uuid_identity_is_part_of_execution_facts() {
    let src = "actor Member\nresource Member { fields { id: Id } rows read when true }\nresource Note { fields { id: Uuid; owner: Member; title: Text } rows read when true expose read { select id, owner, title filter id.eq, id.gt sort id budget { rows 10; depth 1; cost 100; deadline 1s } } }";
    let out = load_str(src, Form::A).unwrap();
    assert_eq!(out.execution["resources"]["Note"]["idKind"], "uuid");
    assert_eq!(out.execution["resources"]["Note"]["fields"]["id"]["ty"], "Id<Note>");
}
