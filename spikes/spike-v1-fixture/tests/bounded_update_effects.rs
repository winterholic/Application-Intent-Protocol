use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};

const SOURCE: &str = r#"
enum OrderStatus { OPEN, CANCELLED }
enum AllocationStatus { ACTIVE, RELEASED }
resource Member { fields { id: Id } }
actor Member
resource Order {
  fields { id: Id; owner: Member; status: OrderStatus }
  transition cancel {
    from status = OPEN
    to status = CANCELLED
    allow owner = actor
    update many Allocation maxRows 100 where order = this.id and status = ACTIVE { status = RELEASED }
  }
  expose apply cancel { target id; bulk maxRows 10 }
}
resource Allocation {
  fields { id: Id; order: Order; status: AllocationStatus }
}
"#;

fn h_definition() -> Value {
    json!({
        "enums": {"OrderStatus": ["OPEN", "CANCELLED"], "AllocationStatus": ["ACTIVE", "RELEASED"]},
        "actor": "Member",
        "resources": {
            "Member": {"fields": {"id": "Id"}},
            "Order": {
                "fields": {"id": "Id", "owner": "Member", "status": "OrderStatus"},
                "transitions": {"cancel": {
                    "from": "status = OPEN", "to": "status = CANCELLED", "allow": "owner = actor",
                    "effects": [{
                        "update": "Allocation", "many": true, "maxRows": 100,
                        "where": {"order": "this.id", "status": "ACTIVE"},
                        "values": {"status": "RELEASED"}
                    }]
                }},
                "exposeApply": {"cancel": {"target": ["id"], "bulkMaxRows": 10}}
            },
            "Allocation": {"fields": {"id": "Id", "order": "Order", "status": "AllocationStatus"}}
        }
    })
}

fn host_source(value: &Value, form: Form) -> String {
    match form {
        Form::HTs => format!("import {{ define }} from '@aip/define';\nexport const spec = define({value});"),
        Form::HPy => format!("from aip.define import define\nSPEC = define({})", python_literal(value)),
        _ => unreachable!(),
    }
}

fn python_literal(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(value) => if *value { "True" } else { "False" }.into(),
        Value::Array(values) => format!("[{}]", values.iter().map(python_literal).collect::<Vec<_>>().join(",")),
        Value::Object(values) => {
            format!("{{{}}}", values.iter().map(|(key, value)| format!("{}:{}", json!(key), python_literal(value))).collect::<Vec<_>>().join(","))
        }
        _ => value.to_string(),
    }
}

#[test]
fn bounded_update_effect_has_matching_facts_in_all_definition_forms() {
    let expected = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("A: {errors:?}")).execution;
    for form in [Form::ETs, Form::EPy, Form::HTs, Form::HPy] {
        let actual = match form {
            Form::ETs => load_str(&format!("import {{ aip }} from '@aip/define';\nexport default aip`{SOURCE}`;"), form),
            Form::EPy => load_str(&format!("from aip.define import aip\nSPEC = aip('''{SOURCE}''')"), form),
            _ => load_str(&host_source(&h_definition(), form), form),
        }
        .unwrap_or_else(|errors| panic!("{form:?}: {errors:?}"))
        .execution;
        assert_eq!(actual, expected, "{form:?}");
    }
}

#[test]
fn bounded_update_effect_rejects_unbounded_or_unscoped_targets() {
    let cases = [
        ("maxRows 100", "maxRows 0"),
        ("maxRows 100", "maxRows 1001"),
        ("order: Order;", "order: Order?;"),
        ("where order = this.id and status = ACTIVE", "where status = ACTIVE"),
        ("{ status = RELEASED }", "{ status = RELEASED; order = this.id }"),
    ];
    for (from, to) in cases {
        assert_eq!(SOURCE.matches(from).count(), 1);
        let invalid = SOURCE.replace(from, to);
        assert!(load_str(&invalid, Form::A).is_err(), "accepted `{to}`");
    }
}

#[test]
fn host_shapes_are_explicit_and_exact_one_facts_stay_unchanged() {
    for form in [Form::HTs, Form::HPy] {
        for bad in [
            json!({"update":"Allocation","many":true,"where":{"order":"this.id"},"values":{"status":"RELEASED"}}),
            json!({"update":"Allocation","maxRows":10,"where":{"order":"this.id"},"values":{"status":"RELEASED"}}),
            json!({"update":"Allocation","many":false,"maxRows":10,"where":{"order":"this.id"},"values":{"status":"RELEASED"}}),
            json!({"update":"Allocation","many":true,"maxRows":"10","where":{"order":"this.id"},"values":{"status":"RELEASED"}}),
        ] {
            let mut definition = h_definition();
            definition["resources"]["Order"]["transitions"]["cancel"]["effects"][0] = bad;
            let errors = load_str(&host_source(&definition, form), form).err().expect("ambiguous bounds rejected");
            assert!(errors.iter().any(|error| error.code == "H_SHAPE"), "{form:?}: {errors:?}");
        }
        let exact_source = SOURCE.replace("update many Allocation maxRows 100", "update Allocation");
        let expected = load_str(&exact_source, Form::A).unwrap().execution;
        let mut definition = h_definition();
        let effect = definition["resources"]["Order"]["transitions"]["cancel"]["effects"][0].as_object_mut().unwrap();
        effect.remove("many");
        effect.remove("maxRows");
        assert_eq!(load_str(&host_source(&definition, form), form).unwrap().execution, expected);
        assert!(expected["resources"]["Order"]["transitions"]["cancel"]["effects"][0].get("maxRows").is_none());
    }
}
