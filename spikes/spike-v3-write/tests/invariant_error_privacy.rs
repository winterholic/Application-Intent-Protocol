//! Delegated decision CD-4 (2026-10-13, security audit O-27): a unique or capacity violation may tell the caller that a
//! colliding row exists (that is what the invariant means), but the error body must never carry the id or any field
//! value of a row the caller cannot read. This test plants distinctive tokens in the hidden rows and checks every
//! serialized error for them.
use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, bundle, Knobs};

const SOURCE: &str = r#"
resource Member { fields { id: Id } }
actor Member
limit maxHeld on Seat = atMost 1 where held = true
resource Seat {
  fields { id: Id; owner: Member?; code: Text; note: Text; held: Bool }
  rows read when owner = actor
  unique code
  transition hold { allow true; from held = false; to held = true, owner = actor }
  expose apply hold { target id; bulk maxRows 1 }
  expose create { allow actor != null; fields code, note, held }
  invariant maxHeld per owner
}
"#;

const SECRET_ID: &str = "987650";
const SECRET_CODE: &str = "HIDDEN-CODE-ZX81";
const SECRET_NOTE: &str = "HIDDEN-NOTE-QQ42";

fn caller(id: i64) -> Caller {
    Caller { actor_id: Some(id), now: "2026-10-13T00:00:00Z".into() }
}

fn assert_clean(what: &str, error: &spike_v2_read::plan::Reject) {
    let body = json!({ "ok": false, "code": error.code, "msg": error.msg }).to_string();
    for token in [SECRET_ID, SECRET_CODE, SECRET_NOTE, "987651"] {
        assert!(!body.contains(token), "{what}: error body leaks `{token}`: {body}");
    }
}

#[tokio::test]
async fn invariant_errors_admit_existence_but_never_carry_a_hidden_rows_id_or_values() {
    let facts = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let schema =
        format!("aip_o27_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url("host=localhost dbname=postgres").await.unwrap();
    for statement in sqlgen::create_ddl_in(&schema, &facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    // member 2 owns two hidden seats: one holds the unique code, one is held already. Member 1 holds seat 5 and owns the free seat 6.
    db.batch_execute(&format!(
        "INSERT INTO {schema}.member(id) VALUES (1),(2); \
         INSERT INTO {schema}.seat(id,owner_id,code,note,held) VALUES ({SECRET_ID},2,'{SECRET_CODE}','{SECRET_NOTE}',true),(987651,2,'other','{SECRET_NOTE}',false),(5,1,'mine',  'ok',true),(6,1,'free','ok',false)"
    ))
    .await
    .unwrap();
    let knobs = Knobs::default();

    // unique collision on create with a row member 1 cannot read: allowed to fail, never to explain with the hidden row
    let created = bundle(
        &mut db,
        &facts,
        &json!({"atomic":[{"create":"Seat","values":{"code":SECRET_CODE,"note":"x","held":false}}]}),
        &caller(1),
        &knobs,
    )
    .await;
    let error = created.expect_err("a duplicate code must be rejected");
    assert_eq!(error.code, "ALREADY_EXISTS", "{error:?}");
    assert_clean("unique create", &error);

    // capacity: member 1 already holds seat 5, so holding seat 6 breaks atMost 1
    let held = apply(&mut db, &facts, &json!({"apply":"Seat.hold","target":{"ids":["6"]}}), &caller(1), &knobs).await;
    let error = held.expect_err("second held seat must be rejected");
    // atMost 1 is a partial unique index, so it surfaces as ALREADY_EXISTS; larger limits are counted at commit.
    assert!(["ALREADY_EXISTS", "INVARIANT_VIOLATED"].contains(&error.code), "{error:?}");
    assert_clean("capacity", &error);

    // a hidden target is the same MISSING_TARGET as a missing one and names neither id
    let hidden = apply(&mut db, &facts, &json!({"apply":"Seat.hold","target":{"ids":[SECRET_ID]}}), &caller(1), &knobs).await;
    let missing = apply(&mut db, &facts, &json!({"apply":"Seat.hold","target":{"ids":["424242"]}}), &caller(1), &knobs).await;
    let (hidden, missing) = (hidden.unwrap_err(), missing.unwrap_err());
    assert_eq!((hidden.code, hidden.msg.as_str()), (missing.code, missing.msg.as_str()), "hidden and missing targets look the same");
    assert_clean("hidden target", &hidden);

    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
