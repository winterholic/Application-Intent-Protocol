//! O-25: commit-time `check` and capacity enforcement must only look at resources the transaction changed.
//! The scan is `xmin = pg_current_xact_id()`, which no index can serve, so running it for every resource on every
//! `/apply` made unrelated writes pay for the size of the largest checked table.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, Knobs};
use std::time::{SystemTime, UNIX_EPOCH};

const DB: &str = "host=localhost dbname=postgres";
const SOURCE: &str = r#"
enum TicketStatus { OPEN, CLOSED }
enum AllocationStatus { ACTIVE, RELEASED }
resource Member { fields { id: Id } }
actor Member
resource Ticket {
  fields { id: Id; owner: Member; status: TicketStatus }
  rows read when owner = actor
  transition close { from status = OPEN; to status = CLOSED; allow owner = actor }
  transition closeAndRelease {
    from status = OPEN
    to status = CLOSED
    allow owner = actor
    update many Allocation maxRows 5 where ticket = this.id and status = ACTIVE { status = RELEASED }
  }
  expose apply close { target id; bulk maxRows 10 }
  expose apply closeAndRelease { target id; bulk maxRows 10 }
}
limit maxReleased on Allocation = atMost 3 where status = RELEASED
resource Allocation {
  fields { id: Id; ticket: Ticket; owner: Member; status: AllocationStatus; note: Text? }
  rows read when owner = actor
  transition release { from status = ACTIVE; to status = RELEASED; allow owner = actor }
  expose apply release { target id; bulk maxRows 1 }
  invariant maxReleased per owner
  check releasedNeedsNote when status != RELEASED or note != null
}
"#;

fn facts() -> Value {
    load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution
}

fn who(id: i64) -> Caller {
    Caller { actor_id: Some(id), now: "2026-10-07T00:00:00Z".into() }
}

fn request(transition: &str, id: i64) -> Value {
    json!({"apply":transition,"target":{"ids":[id.to_string()]}})
}

async fn outcome(db: &mut tokio_postgres::Client, facts: &Value, actor: i64, transition: &str, id: i64) -> &'static str {
    match apply(db, facts, &request(transition, id), &who(actor), &Knobs::default()).await {
        Ok(_) => "OK",
        Err(error) => error.code,
    }
}

/// Sequential scans of one table, read after forcing this session's pending statistics out.
async fn seq_scans(db: &tokio_postgres::Client, schema: &str, table: &str) -> i64 {
    db.batch_execute("SELECT pg_stat_force_next_flush(); SELECT pg_stat_clear_snapshot()").await.unwrap();
    db.query_one("SELECT coalesce(seq_scan,0) FROM pg_stat_user_tables WHERE schemaname=$1 AND relname=$2", &[&schema, &table]).await.unwrap().get(0)
}

async fn apply_flushing(db: &mut tokio_postgres::Client, facts: &Value, actor: i64, transition: &str, id: i64) -> &'static str {
    // The statistics of the transaction that is about to end are published at its commit.
    db.batch_execute("SELECT pg_stat_force_next_flush()").await.unwrap();
    outcome(db, facts, actor, transition, id).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unrelated_writes_do_not_scan_checked_tables_and_related_writes_are_still_checked() {
    let schema = format!("aip_check_scope_{}_{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos());
    sqlgen::set_schema(&schema);
    let facts = facts();
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl_in(&schema, &facts).unwrap() {
        db.batch_execute(&statement).await.unwrap_or_else(|error| panic!("DDL `{statement}`: {error}"));
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1),(2); \
         INSERT INTO {schema}.ticket(id,owner_id,status) VALUES (10,1,'OPEN'),(11,1,'OPEN'),(12,1,'OPEN'),(13,1,'OPEN'),(14,1,'OPEN'); \
         INSERT INTO {schema}.allocation(id,ticket_id,owner_id,status,note) VALUES \
           (20,11,1,'ACTIVE',NULL),(21,12,1,'ACTIVE','ok'),(22,12,1,'ACTIVE','ok'),(23,13,1,'RELEASED','ok'),(24,13,1,'RELEASED','ok'),(25,13,1,'ACTIVE','ok'),(26,14,1,'ACTIVE','ok'),(27,14,2,'ACTIVE','ok')"
    ))
    .await
    .unwrap();

    // Control: a write to the checked table itself must scan it, otherwise this test would not be measuring anything.
    let before = seq_scans(&db, &schema, "allocation").await;
    assert_eq!(apply_flushing(&mut db, &facts, 2, "Allocation.release", 27).await, "OK");
    let related = seq_scans(&db, &schema, "allocation").await - before;
    assert!(related > 0, "measurement control: a write to Allocation should have scanned it (delta {related})");

    // O-25: closing a ticket without touching Allocation must not scan Allocation for the check or the capacity limit.
    let before = seq_scans(&db, &schema, "allocation").await;
    assert_eq!(apply_flushing(&mut db, &facts, 1, "Ticket.close", 10).await, "OK");
    let unrelated = seq_scans(&db, &schema, "allocation").await - before;
    assert_eq!(unrelated, 0, "an unrelated /apply scanned the checked table {unrelated} time(s)");

    // Behavior is unchanged where Allocation is written, directly or through an effect of another resource.
    // check through an effect: ticket 11's effect releases allocation 20, which has no note; Allocation is not the apply target.
    assert_eq!(outcome(&mut db, &facts, 1, "Ticket.closeAndRelease", 11).await, "CHECK_FAILED");
    // capacity through an effect: ticket 12's two allocations plus the two already released by owner 1 exceed atMost 3.
    assert_eq!(outcome(&mut db, &facts, 1, "Ticket.closeAndRelease", 12).await, "INVARIANT_VIOLATED");
    // capacity directly: the third release fits, the fourth does not.
    assert_eq!(outcome(&mut db, &facts, 1, "Allocation.release", 25).await, "OK");
    assert_eq!(outcome(&mut db, &facts, 1, "Allocation.release", 26).await, "INVARIANT_VIOLATED");
    // Refused requests leave nothing behind: owner 1 has its two preloaded rows plus allocation 25.
    let released: i64 =
        db.query_one(&format!("SELECT count(*) FROM {schema}.allocation WHERE owner_id=1 AND status='RELEASED'"), &[]).await.unwrap().get(0);
    assert_eq!(released, 3);
    let open: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.ticket WHERE status='OPEN'"), &[]).await.unwrap().get(0);
    assert_eq!(open, 4, "only ticket 10 was closed");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
