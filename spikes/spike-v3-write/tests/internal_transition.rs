//! `expose apply ... internal`: a transition only an extension or a job may apply. /apply (`apply`), its atomic bundles (`bundle`) and
//! `compose` refuse it as if it were not exposed, however correct the target, the values and the expected version are; the same request
//! with the internal knob (what the extension and job hosts set) runs through the transition's own checks.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::Caller;
use spike_v2_read::{connect, sqlgen};
use spike_v3_write::{apply, bundle, Knobs};

const DEFINITION: &str = "resource Member { fields { id: Id } }
actor Member
resource Tally {
  fields { id: Id; owner: Member; total: Int(0..1000000); version: Int(0..1000000) }
  rows read when owner = actor
  transition add {
    allow owner = actor
    from true
    patch total
    expect version
    to version = this.version + 1
  }
  transition reset { allow owner = actor; from true; to total = 0 }
  expose apply add { target id; bulk maxRows 1; internal }
  expose apply reset { target id; bulk maxRows 1 }
}";

async fn state(db: &mut tokio_postgres::Client, schema: &str) -> (i64, i64) {
    let row = db.query_one(&format!("SELECT total, version FROM {schema}.tally WHERE id = 1"), &[]).await.unwrap();
    (row.get(0), row.get(1))
}

#[tokio::test]
async fn an_internal_transition_is_refused_on_every_public_path_and_runs_with_the_internal_knob() {
    let facts: Value = load_str(DEFINITION, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    assert_eq!(facts["resources"]["Tally"]["exposeApply"]["add"]["internal"], true);
    assert_eq!(facts["resources"]["Tally"]["exposeApply"]["reset"]["internal"], false);
    let schema = format!("aip_internal_tr_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let mut db = connect().await;
    for statement in sqlgen::create_ddl_in(&schema, &facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1), (2); INSERT INTO {schema}.tally(id, owner_id, total, version) VALUES (1, 1, 5, 7)"
    ))
    .await
    .unwrap();
    let caller = Caller { actor_id: Some(1), now: "2026-10-22T00:00:00Z".into() };
    let request = json!({"apply": "Tally.add", "target": {"ids": ["1"]}, "values": {"total": 999}, "expect": {"version": 7}});
    let public = Knobs { audit_via: Some("apply"), ..Knobs::default() };
    let direct = apply(&mut db, &facts, &request, &caller, &public).await.unwrap_err();
    assert_eq!(direct.code, "WIRE.NOT_EXPOSED", "{direct:?}");
    let bundled = bundle(&mut db, &facts, &json!({"atomic": [request.clone()]}), &caller, &public).await.unwrap_err();
    assert_eq!(bundled.code, "WIRE.NOT_EXPOSED", "{bundled:?}");
    let behind = json!({"atomic": [{"apply": "Tally.reset", "target": {"ids": ["1"]}}, request.clone()]});
    let behind = bundle(&mut db, &facts, &behind, &caller, &public).await.unwrap_err();
    assert_eq!(behind.code, "WIRE.NOT_EXPOSED", "{behind:?}");
    assert_eq!(state(&mut db, &schema).await, (5, 7), "a refused request, even one behind an allowed step, changes nothing");

    // The ordinary transition of the same resource is still public.
    let reset = apply(&mut db, &facts, &json!({"apply": "Tally.reset", "target": {"ids": ["1"]}}), &caller, &public).await.unwrap();
    assert_eq!(reset.changed, vec![1]);
    assert_eq!(state(&mut db, &schema).await, (0, 7));

    // The internal knob opens only the exposure: the transition's own checks (expected version, owner) still decide.
    let internal = Knobs { audit_via: Some("extension"), internal: true, ..Knobs::default() };
    let stale = json!({"apply": "Tally.add", "target": {"ids": ["1"]}, "values": {"total": 999}, "expect": {"version": 6}});
    assert_eq!(apply(&mut db, &facts, &stale, &caller, &internal).await.unwrap_err().code, "WIRE.CONFLICT");
    let stranger = Caller { actor_id: Some(2), now: caller.now.clone() };
    assert!(apply(&mut db, &facts, &request, &stranger, &internal).await.is_err(), "another owner is refused");
    assert_eq!(state(&mut db, &schema).await, (0, 7));
    let applied = apply(&mut db, &facts, &request, &caller, &internal).await.unwrap();
    assert_eq!(applied.changed, vec![1]);
    assert_eq!(state(&mut db, &schema).await, (999, 8));
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
