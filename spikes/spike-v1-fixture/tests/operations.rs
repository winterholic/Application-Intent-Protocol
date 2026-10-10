use serde_json::json;
use spike_v1_fixture::{load_str, Form};

const SOURCE: &str = r#"
actor principal
operation read count {
  input { text: Text }
  output { length: Int }
  allow actor != null
  effect none
  deadline 2s
  implementation "compute.count"
}
"#;

#[test]
fn operations_have_one_contract_in_all_five_forms_without_a_resource() {
    let host = json!({"actor":"principal", "resources":{}, "operations":{
        "count":{"kind":"read","input":{"text":"Text"},"output":{"length":"Int"},
        "allow":"actor != null","effect":"none","deadline":"2s","implementation":"compute.count"}
    }});
    let mut canonical = None;
    for (form, wrapped) in [
        (Form::A, SOURCE.to_string()),
        (Form::ETs, format!("import {{ aip }} from '@aip/define'; export default aip`{SOURCE}`;")),
        (Form::EPy, format!("from aip.define import aip\nSPEC = aip(\"\"\"{SOURCE}\"\"\")")),
        (Form::HTs, format!("import {{ define }} from '@aip/define'; export const spec = define({host});")),
        (Form::HPy, format!("from aip.define import define\nSPEC = define({host})")),
    ] {
        let facts = load_str(&wrapped, form).unwrap_or_else(|e| panic!("{form:?}: {e:?}")).execution;
        assert_eq!(facts["actorMode"], "principal");
        assert!(facts["resources"].as_object().unwrap().is_empty());
        assert_eq!(facts["operations"]["count"]["input"], json!([["text", "Text", null]]));
        assert_eq!(facts["operations"]["count"]["deadlineMs"], 2000);
        assert_eq!(facts["operations"]["count"]["dependencies"], json!({"worker":[],"authorization":["database","principal","deployment"]}));
        assert!(!facts["operations"]["count"]["allow"].is_null());
        if let Some(expected) = &canonical {
            assert_eq!(&facts, expected);
        } else {
            canonical = Some(facts);
        }
    }
}

#[test]
fn operations_fail_closed_on_missing_policy_capabilities_and_invalid_contracts() {
    for source in [
        SOURCE.replace("allow actor != null", ""),
        SOURCE.replace("allow actor != null", "allow this.id = actor.id"),
        SOURCE.replace("effect none", "effect db"),
        SOURCE.replace("operation read", "operation write"),
        SOURCE.replace("deadline 2s", "deadline 0ms"),
        SOURCE.replace("effect none", "access Compute.total\n effect none"),
        format!("{SOURCE}\n{SOURCE}"),
        SOURCE.replace("input { text: Text }", "input { text: Text; text: Int }"),
        SOURCE.replace("actor principal", "actor Missing"),
        format!("{SOURCE}\nresource Other {{ fields {{ id: Id }} }}"),
    ] {
        assert!(load_str(&source, Form::A).is_err(), "accepted: {source}");
    }
}

#[test]
fn operations_can_reuse_server_predicates_and_real_identity_resources() {
    let source = SOURCE
        .replace(
            "actor principal",
            "resource Member { fields { id: Id; enabled: Bool } } actor Member predicate active(m: Member) = m.enabled = true",
        )
        .replace("allow actor != null", "allow actor != null and active(actor)");
    let facts = load_str(&source, Form::A).unwrap().execution;
    assert!(facts.get("actorMode").is_none());
    assert_eq!(facts["operations"]["count"]["dependencies"], json!({"worker":[],"authorization":["database","principal","deployment"]}));
    assert_eq!(facts["resources"].as_object().unwrap().len(), 1);

    let intrinsic_id = SOURCE
        .replace("input { text: Text }", "input { text: Text; owner: principal.Id }")
        .replace("allow actor != null", "allow actor.id = input.owner");
    assert!(load_str(&intrinsic_id, Form::A).is_ok());
}

#[test]
fn an_existing_principal_resource_retains_its_identity_contract() {
    let source = format!(
        "resource principal {{ fields {{ id: Id; enabled: Bool }} }}\n{}",
        SOURCE.replace("allow actor != null", "allow actor != null and actor.enabled = true")
    );
    let facts = load_str(&source, Form::A).unwrap().execution;
    assert!(facts.get("actorMode").is_none());
    assert!(facts["resources"]["principal"].is_object());
    let legacy = load_str("actor principal resource principal { fields { id: Id } }", Form::A).unwrap().execution;
    assert!(legacy.get("operations").is_none());
    assert!(legacy.get("actorMode").is_none());
}

#[test]
fn operation_range_metadata_must_fit_the_sdk_integer_domain() {
    for declaration in ["input { text: Text(1..9007199254740992) }", "input { text: Int(0..9007199254740992) }"] {
        assert!(load_str(&SOURCE.replace("input { text: Text }", declaration), Form::A).is_err(), "{declaration}");
    }
    let large = SOURCE.replace("output { length: Int }", "output { length: Int(0..9007199254740992) }");
    assert!(load_str(&large, Form::A).is_err());
    let boundary = SOURCE.replace("output { length: Int }", "output { length: Int(0..9007199254740991) }");
    assert!(load_str(&boundary, Form::A).is_ok());
}
