use serde_json::{json, Value};
use spike_v1_fixture::{digest, load_str, Form};

const TEXT: &str = r#"
resource Member { fields { id: Id }; rows read when true; expose read { select id } }
actor Member
resource Post {
  fields { id: Id; title: Text; author: Member }
  rows read when true
  expose read {
    select id, title
    sort id
    traverse author { select id }
    traverse comments via Comment.post { select id, body; sort id desc; limit 20 }
    budget { rows 10; depth 2; deadline 2s; cost 1000; offset 500; cursor }
  }
}
resource Comment {
  fields { id: Id; post: Post; body: Text }
  rows read when true
  expose read { select id, body }
}
"#;

fn definition() -> Value {
    json!({
        "actor": "Member",
        "resources": {
            "Member": {"fields": {"id": "Id"}, "rows": "true", "read": {"select": ["id"]}},
            "Post": {
                "fields": {"id": "Id", "title": "Text", "author": "Member"},
                "rows": "true",
                "read": {
                    "select": ["id", "title"], "sort": ["id"],
                    "traverse": {
                        "author": {"select": ["id"]},
                        "comments": {"via": "Comment.post", "select": ["id", "body"], "sort": "id desc", "limit": 20}
                    },
                    "budget": {"rows": 10, "depth": 2, "deadline": "2s", "cost": 1000, "offset": 500, "cursor": true}
                }
            },
            "Comment": {"fields": {"id": "Id", "post": "Post", "body": "Text"}, "rows": "true", "read": {"select": ["id", "body"]}}
        }
    })
}

fn source(value: &Value, form: Form) -> String {
    let data = serde_json::to_string(value).expect("JSON definition");
    match form {
        Form::HTs => format!("import {{ define }} from '@aip/define';\nexport const spec = define({data});"),
        Form::HPy => format!("from aip.define import define\nSPEC = define({})", data.replace(":true", ":True").replace(":false", ":False")),
        _ => panic!("object form required"),
    }
}

fn facts(value: &Value, form: Form) -> Value {
    load_str(&source(value, form), form).unwrap_or_else(|errors| panic!("{form:?}: {errors:?}")).execution
}

#[test]
fn recent_read_declarations_produce_identical_facts_in_all_five_forms() {
    let expected = load_str(TEXT, Form::A).expect("text definition").execution;
    for form in [Form::ETs, Form::EPy, Form::HTs, Form::HPy] {
        let actual = match form {
            Form::ETs => load_str(&format!("import {{ aip }} from '@aip/define';\nexport default aip`{TEXT}`;"), form).expect("TS block").execution,
            Form::EPy => load_str(&format!("from aip.define import aip\nSPEC = aip('''{TEXT}''')"), form).expect("Python block").execution,
            _ => facts(&definition(), form),
        };
        assert_eq!(actual, expected, "{form:?}");
        assert_eq!(digest(&actual), digest(&expected));
    }
}

#[test]
fn omitted_options_and_false_cursor_preserve_existing_facts() {
    for form in [Form::HTs, Form::HPy] {
        let mut value = definition();
        let budget = value["resources"]["Post"]["read"]["budget"].as_object_mut().expect("budget");
        budget.remove("offset");
        budget.remove("cursor");
        let without = facts(&value, form);
        value["resources"]["Post"]["read"]["budget"]["cursor"] = json!(false);
        assert_eq!(facts(&value, form), without);
        let read = &without["resources"]["Post"]["exposeRead"]["budget"];
        assert!(read.get("offset").is_none());
        assert!(read.get("cursor").is_none());
    }
}

#[test]
fn object_shapes_and_semantic_limits_are_checked() {
    let cases = [
        ("/resources/Post/read/budget/cursor", json!("true"), "H_SHAPE"),
        ("/resources/Post/read/budget/offset", json!(true), "H_SHAPE"),
        ("/resources/Post/read/budget/offset", json!(-1), "BAD_BUDGET"),
        ("/resources/Post/read/traverse/comments/via", json!("Comment.post.extra"), "H_SHAPE"),
        ("/resources/Post/read/traverse/comments/via", json!("Comment.body"), "TRAVERSE_VIA_NOT_REF"),
        ("/resources/Post/read/traverse/comments/sort", json!("id sideways"), "H_SHAPE"),
        ("/resources/Post/read/traverse/comments/limit", json!(1001), "BAD_LIMIT"),
    ];
    for form in [Form::HTs, Form::HPy] {
        for (pointer, bad, code) in &cases {
            let mut value = definition();
            *value.pointer_mut(pointer).expect("existing mutation target") = bad.clone();
            let errors = load_str(&source(&value, form), form).err().expect("invalid definition must fail");
            assert!(errors.iter().any(|error| error.code == *code), "{form:?} {pointer}: {errors:?}");
        }
        for key in ["offsets", "cursors"] {
            let mut value = definition();
            value["resources"]["Post"]["read"]["budget"][key] = json!(1);
            let errors = load_str(&source(&value, form), form).err().expect("unknown budget option");
            assert!(errors.iter().any(|error| error.code == "UNKNOWN_KEY"), "{errors:?}");
        }
        let mut value = definition();
        value["resources"]["Post"]["read"]["traverse"]["author"]["limit"] = json!(2);
        let errors = load_str(&source(&value, form), form).err().expect("single relation must reject list options");
        assert!(errors.iter().any(|error| error.code == "UNKNOWN_KEY"), "{errors:?}");
    }
}
