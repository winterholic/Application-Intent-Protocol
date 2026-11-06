use spike_v1_fixture::{load_str, Form};

const SOURCE: &str = r#"
actor Member
resource Member { fields { id: Id } }
resource Topic {
  fields { id: Uuid; owner: Member; hidden: Bool }
  rows read when owner = actor
  transition unhide {
    from hidden = true
    to hidden = false
    allow owner = actor
    update Post where id = latest FirstPost link where link.topic = this order by link.id select link.post { hidden = false }
  }
}
resource FirstPost {
  fields { id: Id; topic: Topic; post: Post }
  unique topic
  rows read when topic.owner = actor
}
resource Post { fields { id: Uuid; hidden: Bool } rows read when true }
"#;

#[test]
fn latest_nullable_selection_can_match_a_required_effect_target() {
    let loaded = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("latest target selection: {errors:?}"));
    let effect = &loaded.execution["resources"]["Topic"]["transitions"]["unhide"]["effects"][0];
    assert_eq!(effect["match"]["id"]["latest"]["resource"], "FirstPost");
    assert_eq!(effect["match"]["id"]["ty"], "Ref<Post>?");
    assert_eq!(loaded.execution["resources"]["Topic"]["transitions"]["unhide"]["isolation"], "serializable");
}

#[test]
fn nullable_assignment_and_wrong_match_type_still_refuse() {
    let nullable_assignment =
        SOURCE.replace("{ hidden = false }", "{ id = latest FirstPost link where link.topic = this order by link.id select link.post }");
    assert!(load_str(&nullable_assignment, Form::A).is_err());
    let wrong_type = SOURCE.replace("select link.post {", "select link.topic {");
    assert!(load_str(&wrong_type, Form::A).is_err());
}

#[test]
fn active_flaskbb_definition_accepts_selected_first_post_and_snapshot_guards() {
    let source = include_str!("../../../porting/flaskbb/app.aip");
    let loaded = load_str(source, Form::A).unwrap_or_else(|errors| panic!("active FlaskBB: {errors:?}"));
    assert_eq!(loaded.execution["resources"]["Topic"]["transitions"]["hide"]["isolation"], "serializable");
}

#[test]
fn only_same_resource_integer_id_references_support_order_comparisons() {
    let source = "actor Entry resource Entry { fields { id: Id } } predicate later(left: Entry, right: Entry?) = left > right";
    load_str(source, Form::A).unwrap_or_else(|errors| panic!("native integer identity ordering: {errors:?}"));
    let rejects_type = |source: &str| {
        let errors = load_str(source, Form::A).err().expect("invalid ordering must refuse");
        assert!(errors.iter().any(|error| error.code == "DEF.TYPE_MISMATCH"), "expected type rejection, got {errors:?}");
    };
    rejects_type(&source.replace("id: Id", "id: Uuid"));
    rejects_type(&source.replace("left > right", "left > null"));
    let other = "actor Entry resource Entry { fields { id: Id } } resource Other { fields { id: Id } } predicate later(left: Entry, right: Other) = left > right";
    rejects_type(other);
}
