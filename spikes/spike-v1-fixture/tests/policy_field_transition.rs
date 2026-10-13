//! Delegated decision CD-4 (2026-10-13, security audit O-02): a transition's `from`, the value side of `to`, and
//! `repeat unchanged` on a hidden field answer the caller through `WIRE.INVALID_STATE`, `changed`/`unchanged` or by copying
//! the value into a visible field. A field with `field x read when ...` therefore cannot appear there.
//! `allow` is the author's explicit authorization rule and a write to a hidden field without `repeat unchanged`
//! reveals nothing, so both stay legal.
use spike_v1_fixture::{load_str, Form};

fn load(resource_body: &str) -> Result<(), Vec<spike_v1_fixture::diag::Diag>> {
    let src = format!(
        "resource Member {{ fields {{ id: Id; level: Int }} }}\nactor Member\n\
         resource Folder {{ fields {{ id: Id; secret: Text }} rows read when true field secret read when actor.level > 5 }}\n\
         resource Doc {{\n  fields {{ id: Id; folder: Folder; status: Text; label: Text; hidden: Text; done: Bool }}\n  rows read when true\n  field hidden read when actor.level > 5\n{resource_body}\n}}\n"
    );
    load_str(&src, Form::A).map(|_| ())
}

fn rejected(body: &str) -> String {
    let errs = load(body).expect_err(body);
    let d = errs.iter().find(|d| d.code == "DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION").unwrap_or_else(|| panic!("{body}: {errs:?}"));
    d.msg.clone()
}

#[test]
fn a_hidden_field_cannot_be_read_by_from_or_the_value_side_of_to() {
    assert!(rejected("transition t { allow true; from hidden = \"x\"; to done = true }").contains("hidden"));
    // through a reference hop to another resource's hidden field
    rejected("transition t { allow true; from folder.secret = \"x\"; to done = true }");
    // copying the hidden value into a visible field would publish it
    rejected("transition t { allow true; from done = false; to label = hidden }");
    // inside exists the inner row is Folder, whose `secret` is hidden
    rejected("transition t { allow true; from exists Folder where secret = \"x\"; to done = true }");
}

#[test]
fn repeat_unchanged_on_a_hidden_target_field_is_an_equality_oracle() {
    rejected("transition t { allow true; from done = false; to hidden = \"x\"; repeat unchanged }");
}

#[test]
fn legal_uses_of_hidden_fields_and_visible_fields_still_compile() {
    // a blind write to a hidden field is fine: the default repeat is reject, nothing is revealed
    load("transition t { allow true; from done = false; to hidden = \"x\" }").expect("blind write");
    // allow is the author's authorization rule
    load("transition t { allow hidden = \"x\"; from done = false; to done = true }").expect("allow may read a hidden field");
    load("transition t { allow true; from status = \"a\" and folder.id = folder.id; to status = \"b\"; repeat unchanged }").expect("visible fields");
}
