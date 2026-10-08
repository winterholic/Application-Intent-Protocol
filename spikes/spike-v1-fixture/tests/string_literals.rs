use serde_json::{json, Value};
use spike_v1_fixture::extract::{extract, Host};
use spike_v1_fixture::{load_str, Form};
use std::process::Command;

const A: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Item {
  fields { id: Id; owner: Member; name: Text }
  rows read when owner = actor
  transition rename {
    from name = "plain"
    to name = "A\"B\\C\n\r\t\b\f\/\uD83D\uDE00"
    allow owner = actor
  }
  expose apply rename { target id; bulk maxRows 1 }
}
"#;

fn python_literal(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(v) => if *v { "True" } else { "False" }.into(),
        Value::Array(values) => format!("[{}]", values.iter().map(python_literal).collect::<Vec<_>>().join(",")),
        Value::Object(values) => {
            format!("{{{}}}", values.iter().map(|(key, value)| format!("{}:{}", json!(key), python_literal(value))).collect::<Vec<_>>().join(","))
        }
        _ => value.to_string(),
    }
}

fn host_body(value: &str) -> String {
    let json = serde_json::to_string(value).unwrap();
    json[1..json.len() - 1].to_string()
}

fn h_definition() -> Value {
    json!({
        "actor": "Member",
        "resources": {
            "Member": {"fields": {"id": "Id"}},
            "Item": {
                "fields": {"id": "Id", "owner": "Member", "name": "Text"},
                "rows": "owner = actor",
                "transitions": {"rename": {
                    "from": "name = \"plain\"",
                    "to": r#"name = "A\"B\\C\n\r\t\b\f\/\uD83D\uDE00""#,
                    "allow": "owner = actor"
                }},
                "exposeApply": {"rename": {"target": ["id"], "bulkMaxRows": 1}}
            }
        }
    })
}

#[test]
fn five_forms_decode_two_string_layers_to_identical_facts() {
    let expected = load_str(A, Form::A).unwrap_or_else(|errors| panic!("A: {errors:?}")).execution;
    let body = host_body(A);
    let definition = h_definition();
    let variants = [
        (Form::ETs, format!("import {{ aip }} from '@aip/define';\nexport default aip`{body}`;")),
        (Form::EPy, format!("from aip.define import aip\nSPEC = aip(\"\"\"{body}\"\"\")")),
        (Form::HTs, format!("import {{ define }} from '@aip/define';\nexport const spec = define({definition});")),
        (Form::HPy, format!("from aip.define import define\nSPEC = define({})", python_literal(&definition))),
    ];
    for (form, source) in variants {
        let actual = load_str(&source, form).unwrap_or_else(|errors| panic!("{form:?}: {errors:?}")).execution;
        assert_eq!(actual, expected, "{form:?}");
    }
    assert_eq!(expected["resources"]["Item"]["transitions"]["rename"]["to"]["name"]["lit"], "A\"B\\C\n\r\t\u{0008}\u{000C}/😀");
}

#[test]
fn extracted_e_blocks_match_actual_node_and_python_cooked_strings() {
    let body = host_body(A);
    let ts = format!("import {{ aip }} from '@aip/define';\nexport default aip`{body}`;");
    let py = format!("from aip.define import aip\nSPEC = aip(\"\"\"{body}\"\"\")");
    let node = Command::new("node")
        .args(["-e", &format!("process.stdout.write(JSON.stringify(`{body}`))")])
        .output()
        .expect("Node.js is required for host literal parity");
    assert!(node.status.success(), "{}", String::from_utf8_lossy(&node.stderr));
    let python = Command::new("python3")
        .args(["-c", &format!("import json,sys\nsys.stdout.write(json.dumps(\"\"\"{body}\"\"\", ensure_ascii=False))")])
        .output()
        .expect("Python 3 is required for host literal parity");
    assert!(python.status.success(), "{}", String::from_utf8_lossy(&python.stderr));
    let cooked_ts: String = serde_json::from_slice(&node.stdout).unwrap();
    let cooked_py: String = serde_json::from_slice(&python.stdout).unwrap();
    assert_eq!(extract(&ts, Host::Ts).unwrap().text, cooked_ts);
    assert_eq!(extract(&py, Host::Py).unwrap().text, cooked_py);
    assert_eq!(cooked_ts, A);
    assert_eq!(cooked_py, A);
}

#[test]
fn invalid_escapes_nul_and_unpaired_surrogates_are_rejected() {
    for bad in [r#"\q"#, r#"\u12"#, r#"\uD800"#, r#"\uDE00"#, r#"\u0000"#] {
        let source = A.replace(r#""plain""#, &format!("\"{bad}\""));
        let errors = load_str(&source, Form::A).err().expect("invalid AIP escape must fail");
        assert_eq!(errors[0].code, "LEX_BAD_STRING", "{bad}: {errors:?}");
    }
    for invalid in [r#"\q"#, r#"\/"#, r#"\uD800"#, r#"\uD83D\uDE00"#, r#"\u0000"#] {
        let dsl = format!("resource Member {{ fields {{ id: Id }} docs {{ summary \"bad{invalid}\" }} }} actor Member");
        for (form, wrapper) in [
            (Form::ETs, format!("import {{ aip }} from '@aip/define';\nexport default aip`{dsl}`;")),
            (Form::EPy, format!("from aip.define import aip\nSPEC = aip(\"\"\"{dsl}\"\"\")")),
        ] {
            let errors = load_str(&wrapper, form).err().expect("invalid host escape must fail");
            assert_eq!(errors[0].code, "ESCAPE_UNSUPPORTED", "{form:?} {invalid}: {errors:?}");
        }
        for (form, wrapper) in [
            (Form::HTs, format!("import {{ define }} from '@aip/define';\nexport const spec = define({{actor:\"Member\",resources:{{Member:{{fields:{{id:\"Id\"}},docs:{{summary:\"bad{invalid}\"}}}}}}}});")),
            (Form::HPy, format!("from aip.define import define\nSPEC = define({{\"actor\":\"Member\",\"resources\":{{\"Member\":{{\"fields\":{{\"id\":\"Id\"}},\"docs\":{{\"summary\":\"bad{invalid}\"}}}}}}}})")),
        ] {
            assert!(load_str(&wrapper, form).is_err(), "accepted {form:?} host escape {invalid}");
        }
    }
    let raw_nul = A.replace(r#""plain""#, "\"bad\0value\"");
    assert!(load_str(&raw_nul, Form::A).is_err());
}

#[test]
fn legacy_unescaped_tab_and_unicode_keep_their_values() {
    let source = A.replace(r#""plain""#, "\"한글\t값\"").replace(r#""A\"B\\C\n\r\t\b\f\/\uD83D\uDE00""#, "\"그대로\"");
    let facts = load_str(&source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    assert!(facts.to_string().contains(&serde_json::to_string("한글\t값").unwrap()));
}

#[test]
fn host_raw_carriage_return_in_string_follows_runtime_line_normalization() {
    let source = "resource Member { fields { id: Id } } actor Member resource Item { fields { id: Id; name: Text } transition rename { from name = \"a\rb\"; to name = \"done\"; allow true } }";
    for (form, wrapper) in [
        (Form::ETs, format!("import {{ aip }} from '@aip/define';\nexport default aip`{source}`;")),
        (Form::EPy, format!("from aip.define import aip\nSPEC = aip(\"\"\"{source}\"\"\")")),
    ] {
        assert!(load_str(&wrapper, form).is_err(), "accepted raw CR in {form:?} cooked AIP string");
    }
    let definition = h_definition();
    let ts = format!("import {{ define }} from '@aip/define';\nexport const spec = define({definition});").replace("plain", "a\rb");
    let py = format!("from aip.define import define\nSPEC = define({})", python_literal(&definition)).replace("plain", "a\rb");
    assert!(load_str(&ts, Form::HTs).is_err(), "accepted raw CR inside TS quoted string");
    assert!(load_str(&py, Form::HPy).is_err(), "accepted raw CR inside Python quoted string");
}

#[test]
fn host_unicode_quote_is_data_after_lexical_string_boundaries() {
    let aip = r#"resource Member { fields { id: Id } docs { summary "A\"B" } } actor Member"#;
    let expected = load_str(aip, Form::A).unwrap().metadata;
    let body = r#"resource Member { fields { id: Id } docs { summary \u0022A\\\"B\u0022 } } actor Member"#;
    let ts = format!("import {{ aip }} from '@aip/define';\nexport default aip`{body}`;");
    let py = format!("from aip.define import aip\nSPEC = aip(\"\"\"{body}\"\"\")");
    assert_eq!(load_str(&ts, Form::ETs).unwrap().metadata, expected);
    assert_eq!(load_str(&py, Form::EPy).unwrap().metadata, expected);
    let h_ts = r#"import { define } from '@aip/define'; export const spec = define({actor:"Member",resources:{Member:{fields:{id:"Id"},docs:{summary:"A\u0022B"}}}});"#;
    let h_py = r#"from aip.define import define
SPEC = define({"actor":"Member","resources":{"Member":{"fields":{"id":"Id"},"docs":{"summary":"A\u0022B"}}}})"#;
    assert_eq!(load_str(h_ts, Form::HTs).unwrap().metadata, expected);
    assert_eq!(load_str(h_py, Form::HPy).unwrap().metadata, expected);
}

#[test]
fn host_crlf_line_endings_preserve_facts() {
    let simple = A.replace(r#""A\"B\\C\n\r\t\b\f\/\uD83D\uDE00""#, r#""done""#);
    let expected = load_str(&simple, Form::A).unwrap().execution;
    for (form, wrapper) in [
        (Form::ETs, format!("import {{ aip }} from '@aip/define';\nexport default aip`{simple}`;")),
        (Form::EPy, format!("from aip.define import aip\nSPEC = aip(\"\"\"{simple}\"\"\")")),
    ] {
        let crlf = wrapper.replace('\n', "\r\n");
        assert_eq!(load_str(&crlf, form).unwrap().execution, expected);
    }
}
