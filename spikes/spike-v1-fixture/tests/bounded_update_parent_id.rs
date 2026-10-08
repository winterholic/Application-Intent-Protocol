use spike_v1_fixture::{load_str, Form};

#[test]
fn bounded_update_rejects_transition_that_changes_parent_id() {
    let source = r#"
resource Member { fields { id: Id } }
actor Member
resource SalesOrder {
  fields { id: Id; successor: SalesOrder.Id }
  transition cancel {
    from true
    to id = successor
    allow true
    update many Allocation maxRows 3 where order = this.id { active = false }
  }
  expose apply cancel { target id; bulk maxRows 1 }
}
resource Allocation { fields { id: Id; order: SalesOrder; active: Bool } }
"#;
    let errors = match load_str(source, Form::A) {
        Ok(_) => panic!("bounded effect cannot follow a changed parent ID"),
        Err(errors) => errors,
    };
    assert!(errors.iter().any(|error| error.code == "UNSUPPORTED_EFFECT"), "{errors:?}");
}
