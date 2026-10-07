//! PostgreSQL execution probe for independent ticketing, booking, e-sign, and inventory rules.
//! The fixture is owned by this task and does not reuse the recruitment example.
use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, Knobs};
use std::time::{SystemTime, UNIX_EPOCH};

const DB: &str = "host=localhost dbname=postgres";
const SOURCE: &str = include_str!("../../spike-v1-fixture/fixture/oss-business-domains.aip");

fn facts() -> serde_json::Value {
    load_str(SOURCE, Form::A).unwrap_or_else(|diagnostics| panic!("independent OSS fixture rejected: {diagnostics:?}")).execution
}

fn caller() -> Caller {
    Caller { actor_id: Some(1), now: "2026-10-07T00:00:00Z".into() }
}

fn request(resource_action: &str, id: i64) -> serde_json::Value {
    json!({"apply":resource_action,"target":{"ids":[id.to_string()]}})
}

fn schema_name() -> String {
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("aip_oss_domains_{}_{}", std::process::id(), nonce)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn capacity_signer_uniqueness_booking_rollback_and_stock_floor_execute_in_postgres() {
    let schema = schema_name();
    sqlgen::set_schema(&schema);
    let facts = facts();
    let mut db = connect_with_url(DB).await.unwrap();
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap_or_else(|error| panic!("DDL `{statement}`: {error}"));
    }
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id,email) VALUES (1,'owner@example.test'); \
         INSERT INTO {schema}.event(id,name) VALUES (10,'Workshop'),(11,'Conference'); \
         INSERT INTO {schema}.ticket(id,event_id,status) VALUES (101,10,'SOLD'),(102,10,'AVAILABLE'),(103,10,'AVAILABLE'); \
         INSERT INTO {schema}.appointment(id,owner_id,starts_at,ends_at,status) VALUES \
           (201,1,'2026-10-10T10:00:00Z','2026-10-10T11:00:00Z','REQUESTED'), \
           (202,1,'2026-10-10T12:00:00Z','2026-10-10T12:00:00Z','REQUESTED'); \
         INSERT INTO {schema}.submission(id,event_id) VALUES (301,10),(302,11); \
         INSERT INTO {schema}.signer(id,submission_id,role,email) VALUES (401,301,'Buyer','a@example.test'); \
         INSERT INTO {schema}.stock_item(id,part,on_hand,allocated) VALUES (501,'bolt',2,1),(502,'nut',1,1)"
    ))
    .await
    .expect("seed independent domain rows");

    // Ticketing: the server's per-event capacity permits the second ticket and rolls
    // back the third transition, leaving exactly two sold tickets and one available.
    apply(&mut db, &facts, &request("Ticket.sell", 102), &caller(), &Knobs::default()).await.unwrap();
    let full = apply(&mut db, &facts, &request("Ticket.sell", 103), &caller(), &Knobs::default()).await.unwrap_err();
    assert_eq!(full.code, "INVARIANT_VIOLATED");
    let ticket_counts = db
        .query_one(
            &format!(
                "SELECT count(*) FILTER (WHERE status='SOLD'), count(*) FILTER (WHERE status='AVAILABLE') FROM {schema}.ticket WHERE event_id=10"
            ),
            &[],
        )
        .await
        .unwrap();
    assert_eq!((ticket_counts.get::<_, i64>(0), ticket_counts.get::<_, i64>(1)), (2, 1));

    // E-sign: AIP's composite unique declaration becomes a real database constraint.
    // Direct SQL intentionally probes the generated DDL boundary. The same role can be
    // used on another submission; duplicate role within one is rejected by PostgreSQL.
    db.execute(
        &format!("INSERT INTO {schema}.signer(id,submission_id,role,email) VALUES ($1,$2,$3,$4)"),
        &[&402_i64, &302_i64, &"Buyer", &"b@example.test"],
    )
    .await
    .unwrap();
    let duplicate_role = db
        .execute(
            &format!("INSERT INTO {schema}.signer(id,submission_id,role,email) VALUES ($1,$2,$3,$4)"),
            &[&403_i64, &301_i64, &"Buyer", &"c@example.test"],
        )
        .await
        .unwrap_err();
    assert_eq!(duplicate_role.code().unwrap().code(), "23505");
    let signer_count: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.signer WHERE submission_id=301"), &[]).await.unwrap().get(0);
    assert_eq!(signer_count, 1, "failed duplicate insert must leave the original signer intact");

    // Booking: cancellation commits one outbox effect. A malformed interval is rejected
    // by the commit check, and its status/effects both roll back.
    apply(&mut db, &facts, &request("Appointment.cancel", 201), &caller(), &Knobs::default()).await.unwrap();
    let invalid_booking = apply(&mut db, &facts, &request("Appointment.cancel", 202), &caller(), &Knobs::default()).await.unwrap_err();
    assert_eq!(invalid_booking.code, "CHECK_FAILED");
    let booking_states = db.query_one(
        &format!("SELECT (SELECT status FROM {schema}.appointment WHERE id=201), (SELECT status FROM {schema}.appointment WHERE id=202), (SELECT count(*) FROM {schema}.aip_outbox WHERE topic='booking.cancelled')"),
        &[],
    ).await.unwrap();
    assert_eq!(booking_states.get::<_, String>(0), "CANCELLED");
    assert_eq!(booking_states.get::<_, String>(1), "REQUESTED");
    assert_eq!(booking_states.get::<_, i64>(2), 1);

    // Inventory: one unit can be reserved; a second reservation beyond on_hand fails
    // the commit check and leaves the allocation count unchanged.
    apply(&mut db, &facts, &request("StockItem.reserve", 501), &caller(), &Knobs::default()).await.unwrap();
    let overallocated = apply(&mut db, &facts, &request("StockItem.reserve", 502), &caller(), &Knobs::default()).await.unwrap_err();
    assert_eq!(overallocated.code, "CHECK_FAILED");
    let stock = db
        .query_one(
            &format!("SELECT (SELECT allocated FROM {schema}.stock_item WHERE id=501), (SELECT allocated FROM {schema}.stock_item WHERE id=502)"),
            &[],
        )
        .await
        .unwrap();
    assert_eq!((stock.get::<_, i64>(0), stock.get::<_, i64>(1)), (2, 1));

    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
