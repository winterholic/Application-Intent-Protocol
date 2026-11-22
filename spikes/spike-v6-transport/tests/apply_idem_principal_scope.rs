//! Idempotency keys belong to the principal that sent them (invariant: another principal never gets a stored result replayed).
//! The journal key is `(principal, key)`: two callers may use the same key string, each runs its own write once, and each replay
//! returns only that caller's own result. A caller who reuses its key for a different request is refused, while another
//! caller's identical key is unaffected.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, id_wire::IdWire, plan::Caller, sqlgen};

const DB: &str = aip_test_support::test_database_url!();
const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Counter {
  fields { id: Id; owner: Member; hits: Int(0..1000000) }
  rows read when true
  transition bump { from true; to hits = hits + 1; allow actor != null }
  expose apply bump { target id; bulk maxRows 1 }
}
"#;

async fn apply(db: &mut tokio_postgres::Client, facts: &Value, actor: i64, key: &str, id: &str) -> Value {
    let body = json!({"key":key,"request":{"apply":"Counter.bump","target":{"ids":[id]}}});
    let caller = Caller { actor_id: Some(actor), now: "2026-11-22T00:00:00Z".into() };
    spike_v6_transport::handle(db, facts, "/apply", &body, &caller, IdWire::Legacy).await
}

#[tokio::test]
async fn the_same_key_from_two_principals_runs_twice_and_replays_only_to_its_owner() {
    let schema = format!("aip_idem_principal_{}", std::process::id());
    sqlgen::set_schema(&schema);
    let facts = load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let setup = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        setup.batch_execute(&statement).await.unwrap();
    }
    spike_v6_transport::prepare(&setup).await;
    setup
        .batch_execute(&format!(
            "INSERT INTO {schema}.member(id) VALUES (1),(2); INSERT INTO {schema}.counter(id,owner_id,hits) VALUES (7,1,0),(8,1,0)"
        ))
        .await
        .unwrap();
    let hits = |id: i64| {
        let setup = &setup;
        let schema = &schema;
        async move { setup.query_one(format!("SELECT hits FROM {schema}.counter WHERE id={id}").as_str(), &[]).await.unwrap().get::<_, i64>(0) }
    };
    let mut db = connect_with_url(DB).await.unwrap();

    let first = apply(&mut db, &facts, 1, "shared-key", "7").await;
    assert_eq!(first["ok"], true, "{first}");
    assert!(first["replayed"].is_null(), "{first}");
    // Another principal with the same key string and the same request must execute, not receive actor 1's stored result.
    let second = apply(&mut db, &facts, 2, "shared-key", "7").await;
    assert_eq!(second["ok"], true, "{second}");
    assert!(second["replayed"].is_null(), "another principal's key must not replay actor 1's result: {second}");
    assert_eq!(hits(7).await, 2, "each principal's request ran once");
    // Another principal with the same key string and a different request is not a mismatch of actor 1's key.
    let other_target = apply(&mut db, &facts, 2, "other-key", "8").await;
    assert_eq!(other_target["ok"], true, "{other_target}");
    let cross = apply(&mut db, &facts, 1, "other-key", "8").await;
    assert!(cross["replayed"].is_null(), "{cross}");
    assert_eq!(hits(8).await, 2);

    // Each owner's replay returns its own stored result and writes nothing.
    for actor in [1, 2] {
        let replay = apply(&mut db, &facts, actor, "shared-key", "7").await;
        assert_eq!(replay["replayed"], true, "actor {actor}: {replay}");
    }
    assert_eq!(hits(7).await, 2, "replays do not run the write again");
    // The same principal reusing its key for a different request is refused and writes nothing.
    let reused = apply(&mut db, &facts, 1, "shared-key", "8").await;
    assert_eq!(reused["code"], "WIRE.IDEMPOTENCY_MISMATCH", "{reused}");
    assert_eq!(hits(8).await, 2);

    setup.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
