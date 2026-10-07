use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};

const TEXT: &str = r#"
enum State { DRAFT, LIVE }
resource Member { fields { id: Id } }
actor Member
limit oneLive on Post = atMost 1 where state = LIVE
resource Post {
    fields { id: Id; member: Member; slug: Text; state: State; score: Int }
    unique member, slug
    check nonnegative when score >= 0
    invariant oneLive per member deferred
}
"#;

fn definition() -> Value {
    json!({
        "enums": {"State": ["DRAFT", "LIVE"]}, "actor": "Member",
        "limits": {"oneLive": {"on": "Post", "atMost": 1, "where": "state = LIVE"}},
        "resources": {
            "Member": {"fields": {"id": "Id"}},
            "Post": {
                "fields": {"id": "Id", "member": "Member", "slug": "Text", "state": "State", "score": "Int"},
                "unique": [["member", "slug"]],
                "checks": {"nonnegative": "score >= 0"},
                "invariants": ["oneLive per member deferred"]
            }
        }
    })
}

fn source(value: &Value, form: Form) -> String {
    match form {
        Form::HTs => format!("import {{ define }} from '@aip/define';\nexport const spec = define({value});"),
        Form::HPy => format!("from aip.define import define\nSPEC = define({})", value.to_string().replace(":true", ":True")),
        _ => panic!("object form"),
    }
}

#[test]
fn unique_checks_and_deferred_invariants_share_the_text_semantics() {
    let expected = load_str(TEXT, Form::A).expect("text definition").execution;
    for form in [Form::HTs, Form::HPy] {
        let actual = load_str(&source(&definition(), form), form).unwrap_or_else(|errors| panic!("{form:?}: {errors:?}")).execution;
        assert_eq!(actual, expected, "{form:?}");
    }
}

#[test]
fn malformed_host_constraints_and_unknown_fields_fail() {
    for form in [Form::HTs, Form::HPy] {
        for (key, value, code) in [
            ("unique", json!(["member"]), "H_SHAPE"),
            ("unique", json!([["missing"]]), "UNKNOWN_FIELD"),
            ("checks", json!({"nonnegative": true}), "H_SHAPE"),
            ("checks", json!({"nonnegative": "missing >= 0"}), "UNRESOLVED_NAME"),
            ("invariants", json!(["oneLive per member later"]), "H_SHAPE"),
        ] {
            let mut object = definition();
            object["resources"]["Post"][key] = value;
            let errors = load_str(&source(&object, form), form).err().expect("malformed constraint must fail");
            assert!(errors.iter().any(|error| error.code == code), "{form:?} {key}: {errors:?}");
        }
    }
}
