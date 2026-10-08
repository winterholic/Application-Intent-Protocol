use serde_json::Value;
use spike_v1_fixture::{load_str, Form};
use std::process::Command;

#[test]
fn accepted_host_literals_match_both_language_runtimes_and_all_definition_forms() {
    let literals = [
        r#""""#,
        r#""A\"B""#,
        r#""C:\\notes\\file""#,
        r#""\n\r\t\b\f""#,
        r#""\u0061\uD7FF\uE000\uFFFF""#,
        r#""한글 😀""#,
        r#""O'Brien; SELECT 'literal'; --""#,
        "\"raw\ttab\"",
    ];
    let list = format!("[{}]", literals.join(","));
    let node = Command::new("node").args(["-e", &format!("process.stdout.write(JSON.stringify({list}))")]).output().unwrap();
    let python =
        Command::new("python3").args(["-c", &format!("import json,sys\nsys.stdout.write(json.dumps({list},ensure_ascii=False))")]).output().unwrap();
    assert!(node.status.success(), "{}", String::from_utf8_lossy(&node.stderr));
    assert!(python.status.success(), "{}", String::from_utf8_lossy(&python.stderr));
    let js_values: Vec<Value> = serde_json::from_slice(&node.stdout).unwrap();
    let py_values: Vec<Value> = serde_json::from_slice(&python.stdout).unwrap();
    assert_eq!(js_values, py_values);
    for (literal, expected) in literals.into_iter().zip(js_values) {
        let host_object =
            format!("{{\"actor\":\"Member\",\"resources\":{{\"Member\":{{\"fields\":{{\"id\":\"Id\"}},\"docs\":{{\"summary\":{literal}}}}}}}}}");
        let h_ts = format!("import {{ define }} from '@aip/define'; export const spec = define({host_object});");
        let h_py = format!("from aip.define import define\nSPEC = define({host_object})");
        let aip = format!("resource Member {{ fields {{ id: Id }} docs {{ summary {expected} }} }} actor Member");
        let encoded = serde_json::to_string(&aip).unwrap();
        let body = &encoded[1..encoded.len() - 1];
        let e_ts = format!("import {{ aip }} from '@aip/define'; export default aip`{body}`;");
        let e_py = format!("from aip.define import aip\nSPEC = aip(\"\"\"{body}\"\"\")");
        let canonical = load_str(&aip, Form::A).unwrap_or_else(|errors| panic!("A {literal}: {errors:?}"));
        assert_eq!(canonical.metadata["resource:Member"]["summary"], expected, "AIP decoder must preserve the measured host value");
        for (form, source) in [(Form::HTs, h_ts), (Form::HPy, h_py), (Form::ETs, e_ts), (Form::EPy, e_py)] {
            let result = load_str(&source, form).unwrap_or_else(|errors| panic!("{form:?} {literal}: {errors:?}"));
            assert_eq!(result.metadata["resource:Member"]["summary"], expected, "{form:?} must preserve the measured host value");
            assert_eq!(result.metadata, canonical.metadata, "{form:?} {literal}: expected {expected}");
            assert_eq!(result.execution, canonical.execution, "{form:?} {literal}");
        }
    }
}
