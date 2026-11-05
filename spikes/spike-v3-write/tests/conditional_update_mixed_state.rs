use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, bundle, compose, Knobs};
use std::time::{SystemTime, UNIX_EPOCH};

const DB: &str = aip_test_support::test_database_url!();
fn schema() -> String {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("aip_latest_effect_{}_{}", std::process::id(), nonce)
}

#[tokio::test]
async fn poststate_create_and_this_target_coexist_with_prestate_conditional_update() {
    let schema = schema();
    sqlgen::set_schema(&schema);
    let source = r#"
enum Status { PENDING, APPROVED }
resource Member { fields { id: Id } }
actor Member
predicate approved(s: Status) = s = APPROVED
resource Audit {
  fields { id: Id; order: SalesOrder; accepted: Bool }
  rows read when order.owner = actor
}
resource SalesOrder {
  fields { id: Id; owner: Member; status: Status; phase: Text; enabled: Bool; refreshed: Bool }
  rows read when owner = actor
  transition refresh {
    allow owner = actor
    from status = PENDING and phase = "open" and refreshed = false
    to status = APPROVED, phase = "done", refreshed = true, enabled = false
    create Audit { order = this.id; accepted = approved(status) }
    update Summary when this.enabled = true where order = this.id and phase = this.phase { refreshed = this.enabled }
  }
  expose apply refresh { target id; bulk maxRows 1 }
  expose compose { bulk maxRows 1; transitions refresh }
}
resource Summary {
  fields { id: Id; order: SalesOrder; phase: Text; refreshed: Bool }
  rows read when order.owner = actor
}
"#;
    let facts = load_str(source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap_or_else(|error| panic!("DDL `{statement}`: {error}"));
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1); \
         INSERT INTO {schema}.sales_order(id,owner_id,status,phase,enabled,refreshed) VALUES \
         (100,1,'PENDING','open',true,false),(101,1,'PENDING','open',true,false),(102,1,'PENDING','open',true,false); \
         INSERT INTO {schema}.summary(id,order_id,phase,refreshed) VALUES \
         (300,100,'open',false),(301,100,'done',false), \
         (302,101,'open',false),(303,101,'done',false), \
         (304,102,'open',false),(305,102,'done',false)"
    ))
    .await
    .unwrap();
    let caller = Caller { actor_id: Some(1), now: "2026-11-02T00:00:00Z".into() };

    apply(&mut db, &facts, &json!({"apply":"SalesOrder.refresh","target":{"ids":["100"]}}), &caller, &Knobs::default()).await.unwrap();
    bundle(&mut db, &facts, &json!({"atomic":[{"apply":"SalesOrder.refresh","target":{"ids":["101"]}}]}), &caller, &Knobs::default()).await.unwrap();
    compose(
        &mut db,
        &facts,
        &json!({"compose":"SalesOrder","targets":{"ids":["102"]},"steps":[{"transition":"refresh"}]}),
        &caller,
        &Knobs::default(),
    )
    .await
    .unwrap();

    for (order_id, open_summary_id, done_summary_id) in [(100_i64, 300_i64, 301_i64), (101, 302, 303), (102, 304, 305)] {
        let audit = db.query_one(&format!("SELECT accepted FROM {schema}.audit WHERE order_id={order_id}"), &[]).await.unwrap();
        assert!(audit.get::<_, bool>(0), "create Audit reads post-transition APPROVED state");
        let open_row = db.query_one(&format!("SELECT refreshed FROM {schema}.summary WHERE id={open_summary_id}"), &[]).await.unwrap();
        assert!(!open_row.get::<_, bool>(0), "pre-transition open target is not selected");
        let done_row = db.query_one(&format!("SELECT refreshed FROM {schema}.summary WHERE id={done_summary_id}"), &[]).await.unwrap();
        assert!(done_row.get::<_, bool>(0), "post-transition done target receives pre-transition enabled=true");
        let parent =
            db.query_one(&format!("SELECT status, phase, enabled, refreshed FROM {schema}.sales_order WHERE id={order_id}"), &[]).await.unwrap();
        assert_eq!(parent.get::<_, String>(0), "APPROVED");
        assert_eq!(parent.get::<_, String>(1), "done");
        assert!(!parent.get::<_, bool>(2));
        assert!(parent.get::<_, bool>(3));
    }

    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
