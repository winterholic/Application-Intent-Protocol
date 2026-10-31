use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};

const SOURCE: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Order {
  fields { id: Id; owner: Member; refreshed: Bool; created: Time? }
  transition refresh {
    from refreshed = false
    to refreshed = true
    allow owner = actor
    update Summary where order = this.id {
      item = latest Item candidate where candidate.order = this.id and candidate.active = true order by candidate.created select candidate.id
      label = latest Item candidate where candidate.order = this.id and candidate.active = true order by candidate.created select candidate.label
      created = latest Item candidate where candidate.order = this.id and candidate.active = true order by candidate.created select candidate.created otherwise this.created
    }
  }
}
resource Summary { fields { id: Id; order: Order; item: Item?; label: Text?; created: Time? } }
resource Item { fields { id: Id; order: Order; active: Bool; label: Text; created: Time? } }
"#;

fn h_definition() -> Value {
    json!({
        "actor": "Member",
        "resources": {
            "Member": {"fields": {"id": "Id"}},
            "Order": {
                "fields": {"id": "Id", "owner": "Member", "refreshed": "Bool", "created": "Time?"},
                "transitions": {"refresh": {
                    "from": "refreshed = false", "to": "refreshed = true", "allow": "owner = actor",
                    "effects": [{
                        "update": "Summary", "where": {"order": "this.id"},
                        "values": {
                            "item": "latest Item candidate where candidate.order = this.id and candidate.active = true order by candidate.created select candidate.id",
                            "label": "latest Item candidate where candidate.order = this.id and candidate.active = true order by candidate.created select candidate.label",
                            "created": "latest Item candidate where candidate.order = this.id and candidate.active = true order by candidate.created select candidate.created otherwise this.created"
                        }
                    }]
                }}
            },
            "Summary": {"fields": {"id": "Id", "order": "Order", "item": "Item?", "label": "Text?", "created": "Time?"}},
            "Item": {"fields": {"id": "Id", "order": "Order", "active": "Bool", "label": "Text", "created": "Time?"}}
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

fn first_difference(left: &Value, right: &Value, path: &str) -> Option<String> {
    if left == right {
        return None;
    }
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            for key in left.keys().chain(right.keys()) {
                let Some(a) = left.get(key) else {
                    return Some(format!("{path}.{key}: absent on left, right={}", right[key]));
                };
                let Some(b) = right.get(key) else {
                    return Some(format!("{path}.{key}: left={a}, absent on right"));
                };
                if let Some(diff) = first_difference(a, b, &format!("{path}.{key}")) {
                    return Some(diff);
                }
            }
            None
        }
        (Value::Array(left), Value::Array(right)) => {
            if left.len() != right.len() {
                return Some(format!("{path}: array lengths {} != {}", left.len(), right.len()));
            }
            left.iter().zip(right).enumerate().find_map(|(i, (a, b))| first_difference(a, b, &format!("{path}[{i}]")))
        }
        _ => Some(format!("{path}: left={left}, right={right}")),
    }
}

#[test]
fn latest_server_selected_values_are_typed_and_serialized_in_effects() {
    let facts = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let values = &facts["resources"]["Order"]["transitions"]["refresh"]["effects"][0]["values"];
    let item = &values["item"]["latest"];
    assert_eq!(item["resource"], "Item");
    assert_eq!(item["alias"], "candidate");
    assert_eq!(item["tieBreak"], "idDesc");
    assert_eq!(item["orderBy"]["path"]["segs"], json!(["created"]));
    assert_eq!(item["where"]["and"].as_array().unwrap().len(), 2);
    assert_eq!(item["select"]["path"]["root"], "var.candidate");
    assert_eq!(item["select"]["path"]["segs"], json!([]));
    assert_eq!(values["created"]["latest"]["otherwise"]["path"]["root"], "this");
    assert_eq!(values["created"]["ty"], "Time?");
}

#[test]
fn latest_effect_selection_marks_transition_and_compose_serializable() {
    let source = SOURCE.replace("\n}\nresource Summary", "\n  expose compose { bulk maxRows 1; transitions refresh }\n}\nresource Summary");
    let facts = load_str(&source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    assert_eq!(facts["resources"]["Order"]["transitions"]["refresh"]["isolation"], "serializable");
    assert_eq!(facts["resources"]["Order"]["exposeCompose"]["isolation"], "serializable");

    // Literal/reference-only effects retain the ordinary isolation policy.
    let source = source
        .replace("      item = latest Item candidate where candidate.order = this.id and candidate.active = true order by candidate.created select candidate.id\n", "")
        .replace("latest Item candidate where candidate.order = this.id and candidate.active = true order by candidate.created select candidate.label", "\"fixed\"")
        .replace("latest Item candidate where candidate.order = this.id and candidate.active = true order by candidate.created select candidate.created otherwise this.created", "this.created");
    let facts = load_str(&source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    assert!(facts["resources"]["Order"]["transitions"]["refresh"]["isolation"].is_null());
    assert!(facts["resources"]["Order"]["exposeCompose"]["isolation"].is_null());
}

#[test]
fn update_effect_when_serializes_the_transition_prestate_condition() {
    let source = SOURCE
        .replace("update Summary where order = this.id", "update Summary when this.refreshed = false and this.owner = actor where order = this.id");
    let expected = load_str(&source, Form::A).unwrap_or_else(|errors| panic!("A: {errors:?}")).execution;
    let mut definition = h_definition();
    definition["resources"]["Order"]["transitions"]["refresh"]["effects"][0]["when"] = json!("this.refreshed = false and this.owner = actor");
    for (form, encoded) in [
        (Form::ETs, format!("import {{ aip }} from '@aip/define';\nexport default aip`{source}`;")),
        (Form::EPy, format!("from aip.define import aip\nSPEC = aip('''{source}''')")),
        (Form::HTs, format!("import {{ define }} from '@aip/define';\nexport const spec = define({definition});")),
        (Form::HPy, format!("from aip.define import define\nSPEC = define({})", python_literal(&definition))),
    ] {
        let facts = load_str(&encoded, form).unwrap_or_else(|errors| panic!("{form:?}: {errors:?}")).execution;
        assert_eq!(first_difference(&facts, &expected, "$"), None, "{form:?}");
    }
    let effect = &expected["resources"]["Order"]["transitions"]["refresh"]["effects"][0];
    assert_eq!(effect["when"]["and"].as_array().map(Vec::len), Some(2));

    let non_boolean = source.replace("when this.refreshed = false", "when this.created");
    let errors = load_str(&non_boolean, Form::A).err().expect("non-boolean effect condition rejected");
    assert!(errors.iter().any(|error| error.code == "DEF.TYPE_MISMATCH"), "{errors:?}");
}

#[test]
fn latest_is_restricted_to_effect_values_and_requires_alias_paths() {
    let policy = SOURCE.replace(
        "allow owner = actor",
        "allow created = latest Item candidate where candidate.order = this.id and candidate.active = true order by candidate.id select candidate.created",
    );
    assert!(load_str(&policy, Form::A).is_err(), "latest in policy was accepted");

    let bad_alias = SOURCE.replace("order by candidate.created", "order by created");
    assert!(load_str(&bad_alias, Form::A).is_err(), "unbound order path was accepted");

    let bad_type = SOURCE.replace("otherwise this.created", "otherwise this.refreshed");
    assert!(load_str(&bad_type, Form::A).is_err(), "fallback type mismatch was accepted");

    let masked = SOURCE.replace("resource Item { fields", "resource Item { field label read when false; fields");
    assert!(load_str(&masked, Form::A).is_err(), "field-read-masked values were allowed into an effect");
}

#[test]
fn latest_cannot_read_the_resource_being_updated() {
    let source = r#"
resource Order {
  fields { id: Id; refreshed: Bool }
  transition refresh {
    from refreshed = false
    to refreshed = true
    allow true
    update Item where order = this.id {
      active = latest Item candidate where candidate.order = this.id and candidate.active = true order by candidate.id select candidate.active
    }
  }
}
resource Item { fields { id: Id; order: Order; active: Bool } }
"#;
    let errors = load_str(source, Form::A).err().expect("same-resource source rejected");
    assert!(errors.iter().any(|error| error.code == "DEF.UNSUPPORTED_EFFECT"), "{errors:?}");
}

#[test]
fn latest_effect_facts_match_in_all_five_definition_forms() {
    let expected = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("A: {errors:?}")).execution;
    let definition = h_definition();
    for (form, source) in [
        (Form::ETs, format!("import {{ aip }} from '@aip/define';\nexport default aip`{SOURCE}`;")),
        (Form::EPy, format!("from aip.define import aip\nSPEC = aip('''{SOURCE}''')")),
        (Form::HTs, format!("import {{ define }} from '@aip/define';\nexport const spec = define({definition});")),
        (Form::HPy, format!("from aip.define import define\nSPEC = define({})", python_literal(&definition))),
    ] {
        let actual = load_str(&source, form).unwrap_or_else(|errors| panic!("{form:?}: {errors:?}")).execution;
        assert_eq!(first_difference(&actual, &expected, "$"), None, "{form:?}");
    }
}
