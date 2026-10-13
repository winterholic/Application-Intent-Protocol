//! O-30: a definition may not name a resource, field, transition, operation or parameter `__proto__`. In JavaScript
//! the name is the prototype setter, not an own key, so generated descriptors and SDK objects silently lose it
//! (every call that uses the name is then refused). The definition checker rejects the name instead.
use spike_v1_fixture::{load_str, Form};

const BASE: &str = "actor Member\nresource Member { fields { id: Id } }\n";

fn load(extra: &str) -> Result<(), Vec<spike_v1_fixture::diag::Diag>> {
    load_str(&format!("{BASE}{extra}"), Form::A).map(|_| ())
}

fn assert_reserved(case: &str, source: &str) {
    let errs = load(source).expect_err(case);
    let d = errs.iter().find(|d| d.code == "DEF.RESERVED_NAME").unwrap_or_else(|| panic!("{case}: {errs:?}"));
    assert!(d.msg.contains("__proto__"), "{case}: the message names the rejected name: {}", d.msg);
}

#[test]
fn proto_is_rejected_wherever_a_definition_names_something() {
    assert_reserved("field", "resource Note { fields { id: Id; __proto__: Text } }\n");
    assert_reserved("resource", "resource __proto__ { fields { id: Id } }\n");
    assert_reserved("enum", "enum __proto__ { A, B }\nresource Note { fields { id: Id } }\n");
    assert_reserved(
        "transition",
        "resource Note { fields { id: Id; done: Bool }\n  transition __proto__ { allow actor != null; from done = false; to done = true }\n  expose apply __proto__ { target id; bulk maxRows 1 } }\n",
    );
    assert_reserved(
        "operation",
        "operation read __proto__ {\n  input { text: Text }\n  output { length: Int }\n  allow actor != null\n  effect none\n  deadline 1s\n  implementation \"values.echo\"\n}\n",
    );
    assert_reserved(
        "operation parameter",
        "operation read measure {\n  input { __proto__: Text }\n  output { length: Int }\n  allow actor != null\n  effect none\n  deadline 1s\n  implementation \"values.echo\"\n}\n",
    );
}

#[test]
fn similar_names_are_still_fine() {
    load("resource Note { fields { id: Id; proto: Text; _proto: Text; __proto: Text; proto__: Text; constructor_: Text } }\n")
        .expect("only the exact name is reserved");
}
