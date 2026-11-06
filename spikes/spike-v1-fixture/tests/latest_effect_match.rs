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
    assert_eq!(loaded.execution["resources"]["Forum"]["fields"]["last_post"]["onDelete"], "SET NULL");
    assert_eq!(loaded.execution["resources"]["PostOrder"]["fields"]["post"]["onDelete"], "CASCADE");
}

#[test]
fn reference_delete_policies_parse_and_set_null_requires_an_optional_reference() {
    let source = "actor Parent resource Parent { fields { id: Id } } resource Child { fields { id: Id; parent: Parent? on delete set null } }";
    let loaded = load_str(source, Form::A).unwrap_or_else(|errors| panic!("reference delete policy: {errors:?}"));
    assert_eq!(loaded.execution["resources"]["Child"]["fields"]["parent"]["onDelete"], "SET NULL");

    let invalid = source.replace("Parent? on delete set null", "Parent on delete set null");
    let errors = load_str(&invalid, Form::A).err().expect("SET NULL must not be accepted for a required reference");
    assert!(errors.iter().any(|error| error.code == "DEF.TYPE_MISMATCH"), "unexpected diagnostics: {errors:?}");
}

#[test]
fn latest_null_fallback_is_typed_nullable_and_preserves_the_selected_projection() {
    let source = r#"
resource Member { fields { id: Id } }
actor Member
resource Topic {
  fields { id: Id; owner: Member; hidden: Bool }
  transition unhide {
    from hidden = true
    to hidden = false
    allow owner = actor
    update Summary where topic = this.id {
      post = latest Post candidate where candidate.topic = this.id order by candidate.id select candidate.id otherwise null
      label = latest Post candidate where candidate.topic = this.id order by candidate.id select candidate.label otherwise null
    }
  }
}
resource Summary { fields { id: Id; topic: Topic; post: Post?; label: Text? } }
resource Post { fields { id: Id; topic: Topic; label: Text? } }
"#;
    let loaded = load_str(&source, Form::A).unwrap_or_else(|errors| panic!("latest null fallback: {errors:?}"));
    let values = &loaded.execution["resources"]["Topic"]["transitions"]["unhide"]["effects"][0]["values"];
    let post = &values["post"];
    assert_eq!(post["ty"], "Ref<Post>?");
    assert_eq!(post["latest"]["select"]["path"]["root"], "var.candidate");
    assert_eq!(post["latest"]["select"]["path"]["segs"], serde_json::json!([]));
    assert_eq!(post["latest"]["select"]["ty"], "Ref<Post>");
    assert!(post["latest"]["otherwise"]["lit"].is_null());
    let label = &values["label"];
    assert_eq!(label["ty"], "Text?");
    assert_eq!(label["latest"]["select"]["path"]["segs"], serde_json::json!(["label"]));
    assert!(label["latest"]["otherwise"]["lit"].is_null());

    let required_target = source.replace("post: Post?; label", "post: Post; label");
    let errors = load_str(&required_target, Form::A).err().expect("nullable latest result cannot fill a required reference");
    assert!(errors.iter().any(|error| error.code == "DEF.TYPE_MISMATCH"), "unexpected diagnostics: {errors:?}");

    let wrong_fallback = source.replace("select candidate.id otherwise null", "select candidate.id otherwise candidate.label");
    let errors = load_str(&wrong_fallback, Form::A).err().expect("incompatible non-null fallback must be refused");
    assert!(errors.iter().any(|error| error.code == "DEF.TYPE_MISMATCH"), "unexpected diagnostics: {errors:?}");
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
