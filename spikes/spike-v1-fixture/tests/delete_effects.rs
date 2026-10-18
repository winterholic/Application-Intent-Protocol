use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};

const SOURCE: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Order {
  fields { id: Id; owner: Member; active: Bool }
  transition withdraw {
    from true
    delete
    allow owner = actor
    delete many Child maxRows 100 where order = this.id
  }
  expose apply withdraw { target id; bulk maxRows 1 }
}
resource Child { fields { id: Id; order: Order?; owner: Member? } }
"#;

fn host_definition() -> Value {
    json!({
        "actor": "Member",
        "resources": {
            "Member": {"fields": {"id": "Id"}},
            "Order": {
                "fields": {"id": "Id", "owner": "Member", "active": "Bool"},
                "transitions": {"withdraw": {
                    "from": "true",
                    "delete": true,
                    "allow": "owner = actor",
                    "effects": [{"delete": "Child", "many": true, "maxRows": 100, "where": {"order": "this.id"}}]
                }},
                "exposeApply": {"withdraw": {"target": ["id"], "bulkMaxRows": 1}}
            },
            "Child": {"fields": {"id": "Id", "order": "Order?", "owner": "Member?"}}
        }
    })
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

fn host_source(value: &Value, form: Form) -> String {
    match form {
        Form::HTs => format!("import {{ define }} from '@aip/define';\nexport const spec = define({value});"),
        Form::HPy => format!("from aip.define import define\nSPEC = define({})", python_literal(value)),
        _ => unreachable!(),
    }
}

fn source_for(form: Form) -> String {
    match form {
        Form::A => SOURCE.into(),
        Form::ETs => format!("import {{ aip }} from '@aip/define';\nexport default aip`{SOURCE}`;"),
        Form::EPy => format!("from aip.define import aip\nSPEC = aip('''{SOURCE}''')"),
        Form::HTs | Form::HPy => host_source(&host_definition(), form),
    }
}

fn compiled(source: &str) -> Value {
    load_str(source, Form::A).unwrap().execution
}

#[test]
fn delete_transition_and_nullable_child_delete_facts_match_all_definition_forms() {
    let expected = load_str(SOURCE, Form::A).unwrap().execution;
    for form in [Form::ETs, Form::EPy, Form::HTs, Form::HPy] {
        let actual = load_str(&source_for(form), form).unwrap_or_else(|errors| panic!("{form:?}: {errors:?}")).execution;
        assert_eq!(actual, expected, "{form:?}");
    }
    let transition = &expected["resources"]["Order"]["transitions"]["withdraw"];
    assert_eq!(transition["delete"], true);
    assert_eq!(transition["to"], json!({}), "delete transition has an empty update target: {transition}");
    assert_eq!(transition["effects"][0]["delete"], "Child");
    assert_eq!(transition["effects"][0]["maxRows"], 100);
    assert_eq!(transition["effects"][0]["match"]["order"]["path"]["root"], "this");
}

#[test]
fn delete_transition_rejects_update_fields_expect_and_repeat_unchanged() {
    for (from, to) in [
        ("delete\n", "delete\n    to active = false\n"),
        ("delete\n", "delete\n    patch active\n"),
        ("delete\n", "delete\n    expect active\n"),
        ("delete\n", "delete\n    repeat unchanged\n"),
    ] {
        let invalid = SOURCE.replacen(from, to, 1);
        assert!(load_str(&invalid, Form::A).is_err(), "accepted delete combination: {to}");
    }
}

#[test]
fn delete_many_requires_parent_scope_and_bounded_rows_but_accepts_nullable_parent_fk() {
    for (from, to) in [
        ("maxRows 100", "maxRows 0"),
        ("maxRows 100", "maxRows 1001"),
        ("where order = this.id", "where owner = this.id"),
        ("where order = this.id", "where order = actor.id"),
        ("where order = this.id", "where owner = actor"),
    ] {
        assert_eq!(SOURCE.matches(from).count(), 1);
        let invalid = SOURCE.replace(from, to);
        assert!(load_str(&invalid, Form::A).is_err(), "accepted `{to}`");
    }
    for max_rows in [1, 1000] {
        let bounded = SOURCE.replace("maxRows 100", &format!("maxRows {max_rows}"));
        load_str(&bounded, Form::A).unwrap_or_else(|errors| panic!("maxRows {max_rows}: {errors:?}"));
    }
    // Nullable Child.order is intentional: deleting nullable-FK children is safe when scoped to this parent.
    load_str(SOURCE, Form::A).expect("nullable parent FK is permitted for delete-many");
}

#[test]
fn delete_effect_h_literals_reject_ambiguous_bounds_and_keep_all_forms_aligned() {
    for form in [Form::HTs, Form::HPy] {
        for bad in [
            json!({"delete":"Child","many":true,"where":{"order":"this.id"}}),
            json!({"delete":"Child","maxRows":10,"where":{"order":"this.id"}}),
            json!({"delete":"Child","many":false,"maxRows":10,"where":{"order":"this.id"}}),
            json!({"delete":"Child","many":true,"maxRows":"10","where":{"order":"this.id"}}),
        ] {
            let mut definition = host_definition();
            definition["resources"]["Order"]["transitions"]["withdraw"]["effects"][0] = bad;
            let errors = load_str(&host_source(&definition, form), form).err().expect("ambiguous delete bounds rejected");
            assert!(errors.iter().any(|error| error.code == "DEF.H_SHAPE"), "{form:?}: {errors:?}");
        }
    }
}

#[test]
fn single_delete_effect_facts_work_without_many_or_parent_scope() {
    let single = SOURCE.replace("delete many Child maxRows 100 where order = this.id", "delete Child where owner = actor");
    let facts = compiled(&single);
    let effect = &facts["resources"]["Order"]["transitions"]["withdraw"]["effects"][0];
    assert_eq!(effect["delete"], "Child");
    assert!(effect.get("maxRows").is_none());
}
