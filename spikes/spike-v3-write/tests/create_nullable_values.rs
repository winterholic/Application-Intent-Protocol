use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{bundle, compose, Knobs};

const SOURCE: &str = r#"
enum ReviewStatus { PENDING, APPROVED }
resource Member { fields { id: Id } }
actor Member
resource Item {
  fields { id: Id; title: Text; archived: Bool?; reviewer: Member?; status: ReviewStatus?;
    count: Int?; memo: Text?; email: Email?; openedAt: Time?; day: Date?; amount: Decimal(9,2)?; legacy: Member.Id? }
  expose create {
    allow actor != null and archived = null and reviewer = null and status = null and count = null
      and memo = null and email = null and openedAt = null and day = null and amount = null and legacy = null
    fields title, archived, reviewer, status, count, memo, email, openedAt, day, amount, legacy
  }
}
resource Empty { fields { id: Id; note: Text? } expose create { allow actor != null and note = null; fields note } }
resource IdentityOnly { fields { id: Id } expose create { allow actor != null } }
resource Draft {
  fields { id: Id; title: Text }
  rows read when true
  expose compose { bulk maxRows 1; create Item from title }
}
"#;

fn caller(id: Option<i64>) -> Caller {
    Caller { actor_id: id, now: "2026-10-09T00:00:00Z".into() }
}

fn create(resource: &str, values: Value) -> Value {
    json!({"atomic":[{"create":resource,"values":values}]})
}

#[tokio::test]
async fn omitted_and_explicit_null_values_have_the_same_create_policy_meaning() {
    let facts = load_str(SOURCE, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let schema =
        format!("aip_create_null_{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url("host=localhost dbname=postgres").await.unwrap();
    for statement in sqlgen::create_ddl_in(&schema, &facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.draft(id,title) VALUES(10,'composed')"))
        .await
        .unwrap();
    let knobs = Knobs::default();
    let mut failures = vec![];
    for (name, request) in [
        ("omitted nullable fields", create("Item", json!({"title":"omitted"}))),
        (
            "explicit null fields",
            create(
                "Item",
                json!({"title":"null","archived":null,"reviewer":null,"status":null,"count":null,"memo":null,"email":null,"openedAt":null,"day":null,"amount":null,"legacy":null}),
            ),
        ),
        ("empty nullable values", create("Empty", json!({}))),
        ("identity-only values", create("IdentityOnly", json!({}))),
    ] {
        match bundle(&mut db, &facts, &request, &caller(Some(1)), &knobs).await {
            Ok(result) => assert_eq!(result.created, 1),
            Err(error) => failures.push(format!("{name}: {error:?}")),
        }
    }
    let request = json!({"compose":"Draft","targets":{"ids":["10"]},"steps":[{"create":"Item","values":{"title":{"item":"title"}}}]});
    match compose(&mut db, &facts, &request, &caller(Some(1)), &knobs).await {
        Ok(result) => assert_eq!(result.created, 1),
        Err(error) => failures.push(format!("W1 omitted nullable fields: {error:?}")),
    }
    if !failures.is_empty() {
        db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
        panic!("{}", failures.join("\n"));
    }
    let all_null: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.item WHERE archived IS NULL AND reviewer_id IS NULL AND status IS NULL AND count IS NULL AND memo IS NULL AND email IS NULL AND opened_at IS NULL AND day IS NULL AND amount IS NULL AND legacy IS NULL"), &[]).await.unwrap().get(0);
    assert_eq!(all_null, 3);
    for (values, code) in [
        (json!({"title":"denied","archived":true}), "FORBIDDEN"),
        (json!({"title":"denied","reviewer":"1"}), "FORBIDDEN"),
        (json!({"title":null}), "BAD_VALUE"),
        (json!({}), "BAD_VALUE"),
    ] {
        assert_eq!(bundle(&mut db, &facts, &create("Item", values), &caller(Some(1)), &knobs).await.unwrap_err().code, code);
    }
    assert_eq!(bundle(&mut db, &facts, &create("Item", json!({"title":"anonymous"})), &caller(None), &knobs).await.unwrap_err().code, "FORBIDDEN");
    let rollback =
        json!({"atomic":[{"create":"Item","values":{"title":"rolled back"}},{"create":"Item","values":{"title":"denied","archived":true}}]});
    assert_eq!(bundle(&mut db, &facts, &rollback, &caller(Some(1)), &knobs).await.unwrap_err().code, "FORBIDDEN");
    let count: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.item"), &[]).await.unwrap().get(0);
    assert_eq!(count, 3, "denials and a later failed operation leave no partial rows");
    let empty = db.query_one(&format!("SELECT id,note FROM {schema}.empty"), &[]).await.unwrap();
    assert!(empty.get::<_, i64>(0) > 0);
    assert_eq!(empty.get::<_, Option<String>>(1), None);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
