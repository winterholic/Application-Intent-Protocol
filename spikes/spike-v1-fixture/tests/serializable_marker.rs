//! Delegated decision CD-5 (security audit O-26): the compile step marks the writes that must run in SERIALIZABLE.
//! A write is marked when its `allow`/`from` (or an `expose create` allow) needs another row to be ABSENT
//! (`not exists`, directly, nested or through a predicate), or when a commit-time `check` on a resource it writes
//! does. Conditions that need rows to be present (`exists`, a reference hop, the actor's row) stay on the current
//! isolation level because the write path already takes `FOR SHARE` on the rows they read.
use serde_json::Value;
use spike_v1_fixture::{load_str, Form};

fn facts(body: &str) -> Value {
    let src = format!(
        "resource Member {{ fields {{ id: Id; level: Int }} }}\nactor Member\n\
         resource Room {{ fields {{ id: Id; open: Bool }} rows read when true }}\n\
         predicate roomOpen(r: Room) = r.open = true\n\
         predicate roomFree(r: Room) = not exists Slot where room = r and taken = true\n\
         resource Slot {{\n  fields {{ id: Id; room: Room; owner: Member; taken: Bool; note: Text? }}\n  rows read when true\n{body}\n}}\n"
    );
    load_str(&src, Form::A).unwrap_or_else(|e| panic!("{body}: {e:?}")).execution
}

fn marker(f: &Value, path: &[&str]) -> Value {
    path.iter().fold(f["resources"]["Slot"].clone(), |v, k| v[*k].clone())
}

#[test]
fn only_writes_that_need_a_row_to_be_absent_are_marked() {
    let f = facts(
        r#"
  transition own { allow this.owner = actor; from taken = false; to taken = true }
  transition viaPredicate { allow roomFree(this.room); from taken = false; to taken = true }
  transition viaDirect { allow not exists Slot where taken = true; from taken = false; to taken = true }
  transition viaFromAbsence { allow true; from not exists Room where open = true; to taken = true }
  transition nested { allow exists Room where not exists Slot where taken = true; from taken = false; to taken = true }
  transition hop { allow this.room.open = true; from taken = false; to taken = true }
  transition actorRow { allow actor.level > 3; from taken = false; to taken = true }
  transition present { allow exists Room where open = true; from taken = false; to taken = true }
  transition presentPredicate { allow roomOpen(this.room); from taken = false; to taken = true }
  transition doubleNegation { allow not not exists Room where open = true; from taken = false; to taken = true }
  transition negatedAbsence { allow not roomFree(this.room); from taken = false; to taken = true }
"#,
    );
    let t = |name: &str| marker(&f, &["transitions", name, "isolation"]);
    for name in ["viaPredicate", "viaDirect", "viaFromAbsence", "nested"] {
        assert_eq!(t(name), "serializable", "{name} needs a row to be absent");
    }
    for name in ["own", "hop", "actorRow", "present", "presentPredicate", "doubleNegation", "negatedAbsence"] {
        assert_eq!(t(name), Value::Null, "{name} reads rows that FOR SHARE already protects");
    }
}

#[test]
fn a_commit_check_that_needs_absence_marks_every_write_to_its_resource() {
    let f = facts(
        r#"
  check noOtherTaken when not exists Slot where taken = true
  transition own { allow this.owner = actor; from taken = false; to taken = true }
  expose create { allow actor != null; fields room, owner, taken }
"#,
    );
    assert_eq!(marker(&f, &["transitions", "own", "isolation"]), "serializable");
    assert_eq!(marker(&f, &["exposeCreate", "isolation"]), "serializable");
}

#[test]
fn create_follows_the_same_rule() {
    let plain = facts("  expose create { allow actor != null; fields room, owner, taken }\n");
    assert_eq!(marker(&plain, &["exposeCreate", "isolation"]), Value::Null);
    let absent = facts("  expose create { allow roomFree(this.room); fields room, owner, taken }\n");
    assert_eq!(marker(&absent, &["exposeCreate", "isolation"]), "serializable");
}

#[test]
fn an_effect_guard_that_needs_absence_marks_its_transition_serializable() {
    let f = facts(
        r#"
  transition effectAbsence { allow this.owner = actor; from taken = false; to taken = true; update Room when roomFree(this.room) where id = this.room.id { open = false } }
  transition effectPresent { allow this.owner = actor; from taken = false; to taken = true; update Room when this.owner.level > 3 where id = this.room.id { open = false } }
"#,
    );
    assert_eq!(marker(&f, &["transitions", "effectAbsence", "isolation"]), "serializable");
    assert_eq!(marker(&f, &["transitions", "effectPresent", "isolation"]), Value::Null);
}
