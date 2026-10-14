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
        format!("{SOURCE}\nresource Other {{ fields {{ id: Id }} rows read when missingField = true }}"),
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

#[test]
fn durable_jobs_are_explicit_and_managed_capabilities_are_server_scoped() {
    let job = SOURCE.replace("operation read", "operation job").replace("deadline 2s", "deadline 120s");
    for (form, source) in [
        (Form::A, job.clone()),
        (Form::ETs, format!("import {{ aip }} from '@aip/define'; export default aip`{job}`;")),
        (Form::EPy, format!("from aip.define import aip\nSPEC = aip(\"\"\"{job}\"\"\")")),
        (
            Form::HTs,
            format!(
                "import {{ define }} from '@aip/define'; export const spec = define({});",
                json!({"actor":"principal","resources":{},"operations":{"count":{
            "kind":"job","input":{"text":"Text"},"output":{"length":"Int"},"allow":"actor != null",
            "effect":"none","deadline":"120s","implementation":"compute.count"}}})
            ),
        ),
        (
            Form::HPy,
            format!(
                "from aip.define import define\nSPEC = define({})",
                json!({"actor":"principal","resources":{},"operations":{"count":{
            "kind":"job","input":{"text":"Text"},"output":{"length":"Int"},"allow":"actor != null",
            "effect":"none","deadline":"120s","implementation":"compute.count"}}})
            ),
        ),
    ] {
        let facts = load_str(&source, form).unwrap_or_else(|e| panic!("{form:?}: {e:?}")).execution;
        assert_eq!(facts["operations"]["count"]["kind"], "job");
        assert_eq!(facts["operations"]["count"]["deadlineMs"], 120_000);
    }
    let managed = job.replace("effect none", "access http.billing, files.reports, events.notifications, jobs.count\n effect managed");
    let facts = load_str(&managed, Form::A).unwrap().execution;
    assert_eq!(facts["operations"]["count"]["access"]["http.billing"], true);
    assert_eq!(facts["operations"]["count"]["effect"], "managed");
    let helper = SOURCE.replace("operation read count", "operation read helper");
    let job_calling_read = format!("{}\n{}", helper, managed.replace("jobs.count", "jobs.helper"));
    assert!(load_str(&job_calling_read, Form::A).is_ok(), "job capabilities may target a declared read operation");
    for invalid in [
        managed.replace("http.billing", "shell.exec"),
        managed.replace("jobs.count", "jobs.missing"),
        managed.replace("effect managed", "effect none"),
        job.replace("deadline 120s", "deadline 3601s"),
        managed.replace("operation job", "operation read"),
    ] {
        assert!(load_str(&invalid, Form::A).is_err(), "accepted: {invalid}");
    }
}

#[test]
fn managed_jobs_can_declare_message_queue_capabilities() {
    let job = SOURCE.replace("operation read", "operation job").replace("deadline 2s", "deadline 120s");
    let managed = job.replace("effect none", "access messages.invoice_events\n effect managed");
    let facts = load_str(&managed, Form::A).unwrap().execution;
    assert_eq!(facts["operations"]["count"]["access"]["messages.invoice_events"], true);

    let invalid_queue = managed.replace("messages.invoice_events", "messages.인보이스");
    assert!(load_str(&invalid_queue, Form::A).is_err(), "queue names must stay ASCII identifiers");
    let non_job = managed.replace("operation job", "operation read");
    assert!(load_str(&non_job, Form::A).is_err(), "message queues are a durable job capability");
}

#[test]
fn managed_principal_can_coexist_with_external_business_resources() {
    let facts = load_str(&format!("{SOURCE}\nresource Other {{fields {{id: Uuid}} rows read when false}}"), Form::A).unwrap().execution;
    assert_eq!(facts["actorMode"], "principal");
    assert_eq!(facts["resources"]["Other"]["idKind"], "uuid");
}
