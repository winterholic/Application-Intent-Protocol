use serde_json::json;
use spike_v1_fixture::{load_str, Form};

const TEXT: &str = "resource Member { fields { id: Id } } actor Member\nresource Note { fields { id: Id; owner: Member; title: Text(1..40); content: Text?; version: Int } rows read when owner = actor transition edit { from true; allow owner = actor; patch title, content; expect version; to version = version + 1 } expose apply edit { target id; bulk maxRows 1 } }";

#[test]
fn general_update_has_identical_facts_and_digest_in_five_forms() {
    let expected = load_str(TEXT, Form::A).expect("patch declaration");
    let obj = json!({"actor":"Member","resources":{
      "Member":{"fields":{"id":"Id"}},
      "Note":{"fields":{"id":"Id","owner":"Member","title":"Text(1..40)","content":"Text?","version":"Int"},"rows":"owner = actor",
        "transitions":{"edit":{"from":"true","allow":"owner = actor","patch":["title","content"],"expect":["version"],"to":"version = version + 1"}},"exposeApply":{"edit":{"target":["id"],"bulkMaxRows":1}}}
    }});
    for (form, source) in [
        (Form::ETs, format!("import {{ aip }} from \"@aip/define\"; export default aip`{TEXT}`;")),
        (Form::EPy, format!("from aip.define import aip\nSPEC = aip('''{TEXT}''')")),
        (Form::HTs, format!("import {{ define }} from \"@aip/define\"; export const spec = define({obj});")),
        (Form::HPy, format!("from aip.define import define\nSPEC = define({obj})")),
    ] {
        let actual = load_str(&source, form).unwrap_or_else(|e| panic!("{form:?}: {e:?}"));
        assert_eq!(actual.execution, expected.execution);
        assert_eq!(spike_v1_fixture::digest(&actual.execution), spike_v1_fixture::digest(&expected.execution));
    }
    assert_eq!(expected.execution["resources"]["Note"]["transitions"]["edit"]["patch"], json!(["content", "title"]));
}

#[test]
fn patch_only_and_condition_only_actions_are_supported() {
    let source = TEXT.replace("expect version; to version = version + 1", "");
    assert!(load_str(&source, Form::A).is_ok());
    let source = TEXT.replace("patch title, content;", "");
    assert!(load_str(&source, Form::A).is_ok());
}

#[test]
fn unsafe_or_ambiguous_update_declarations_are_rejected() {
    for (old, new) in [
        ("patch title, content", "patch id"),
        ("patch title, content", "patch absent"),
        ("patch title, content", "patch title, title"),
        ("patch title, content", "patch version"),
        ("patch title, content", "patch title; repeat unchanged"),
        ("expect version", "expect version, version"),
        ("expect version", "expect absent"),
        ("expect version", "expect owner"),
        ("patch title, content;", "repeat unchanged;"),
    ] {
        assert!(load_str(&TEXT.replace(old, new), Form::A).is_err(), "accepted {new}");
    }
    let hidden = TEXT.replace("rows read", "field title read when owner = actor; rows read").replace("expect version", "expect title");
    assert!(load_str(&hidden, Form::A).is_err());
    let composed = TEXT.replace("expose apply edit", "expose compose { bulk maxRows 1; transitions edit } expose apply edit");
    assert!(load_str(&composed, Form::A).is_err());
}
