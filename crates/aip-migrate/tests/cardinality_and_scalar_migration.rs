use aip_migrate::{apply, creation, init, plan, preflight};
use serde_json::{Value, json};
use spike_v1_fixture::{Form, load_str};
use std::sync::atomic::{AtomicU64, Ordering};

const DB: &str = "host=localhost dbname=postgres";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct OwnedSchema(String);
impl OwnedSchema {
    fn new() -> Self {
        Self(format!("aip_capacity_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
    }
}
impl Drop for OwnedSchema {
    fn drop(&mut self) {
        let schema = self.0.clone();
        std::thread::spawn(move || {
            let _ =
                std::process::Command::new("psql").args(["-X", "-q", "-d", DB, "-c", &format!("DROP SCHEMA IF EXISTS {schema} CASCADE")]).output();
        })
        .join()
        .ok();
    }
}

const BASE: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Item { fields { id: Id; owner: Member; active: Bool } }
"#;

const AT_MOST_TWO: &str = r#"
resource Member { fields { id: Id } }
actor Member
limit maxItems on Item = atMost 2 where active = true
resource Item {
  fields { id: Id; owner: Member; active: Bool }
  invariant maxItems per owner
}
"#;

fn facts(source: &str) -> Value {
    load_str(source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adding_capacity_that_existing_rows_violate_is_blocked() {
    let schema = OwnedSchema::new();
    let old = facts(BASE);
    let new = facts(AT_MOST_TWO);
    init(DB, &schema.0, &old).await.unwrap();
    let (db, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    db.batch_execute(&format!(
        "INSERT INTO {}.member(id) VALUES(1); INSERT INTO {}.item(id,owner_id,active) VALUES(1,1,true),(2,1,true),(3,1,true)",
        schema.0, schema.0
    ))
    .await
    .unwrap();

    let report = plan(DB, &schema.0, &new, &None).await.unwrap();
    assert_eq!(report["blocked"], true, "{report}");
    assert!(
        report["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| { change["class"] == "Blocked" && change["reason"].as_str().unwrap_or("").contains("maxItems") }),
        "{report}"
    );
}

#[test]
fn at_most_one_keeps_its_existing_partial_unique_facts_and_ddl() {
    let source = r#"
resource Member { fields { id: Id } }
actor Member
limit maxItems on Item = atMost 1 where active = true
resource Item {
  fields { id: Id; owner: Member; active: Bool }
  invariant maxItems per owner
}
"#;
    let facts = facts(source);
    assert_eq!(
        facts["resources"]["Item"]["invariants"]["maxItems"]["enforcement"],
        json!({"kind":"partialUniqueIndex","columns":["owner"],"where":{"cmp":"=","l":{"path":{"root":"this","segs":["active"]},"ty":"Bool"},"r":{"lit":true}},"deferred":false})
    );
    let (ddl, _) = creation("aip_n1_compat", &facts).unwrap();
    assert!(
        ddl.iter().any(|statement| statement == "CREATE UNIQUE INDEX item_max_items ON aip_n1_compat.item (owner_id) WHERE (active = true)"),
        "{ddl:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_capacity_change_updates_product_facts_without_runtime_ddl() {
    let schema = OwnedSchema::new();
    let old = facts(BASE);
    let new = facts(AT_MOST_TWO);
    init(DB, &schema.0, &old).await.unwrap();
    let (db, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    db.batch_execute(&format!(
        "INSERT INTO {}.member(id) VALUES(1); INSERT INTO {}.item(id,owner_id,active) VALUES(1,1,true),(2,1,false)",
        schema.0, schema.0
    ))
    .await
    .unwrap();

    let report = plan(DB, &schema.0, &new, &None).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    assert_eq!(report["requiresReview"], true, "{report}");
    assert!(
        report["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| { change["class"] == "SecurityReview" && change["reason"].as_str().unwrap_or("").contains("정원") }),
        "{report}"
    );
    assert!(
        report["steps"].as_array().unwrap().iter().all(|step| {
            let sql = step["sql"].as_str().unwrap_or("");
            !sql.contains("lockedCountCheck") && !sql.contains("CREATE UNIQUE INDEX")
        }),
        "{report}"
    );
    apply(DB, &schema.0, &new, &None, report["digest"].as_str().unwrap()).await.unwrap();
    assert_eq!(preflight(DB, &schema.0, &new).await.unwrap()["ok"], true);

    let changed = facts(&AT_MOST_TWO.replace("atMost 2", "atMost 3"));
    let changed_report = plan(DB, &schema.0, &changed, &None).await.unwrap();
    assert_eq!(changed_report["blocked"], false, "{changed_report}");
    assert_eq!(changed_report["requiresReview"], true, "{changed_report}");
    apply(DB, &schema.0, &changed, &None, changed_report["digest"].as_str().unwrap()).await.unwrap();
    assert_eq!(preflight(DB, &schema.0, &changed).await.unwrap()["ok"], true);

    let removed = plan(DB, &schema.0, &facts(BASE), &None).await.unwrap();
    assert_eq!(removed["requiresReview"], true, "{removed}");
    assert!(
        removed["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| { change["class"] == "SecurityReview" && change["reason"].as_str().unwrap_or("").contains("제거") }),
        "{removed}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn email_and_date_fields_can_be_backfilled_in_the_product_migration() {
    let schema = OwnedSchema::new();
    let old = facts(BASE);
    let after_source = BASE.replace("fields { id: Id }", "fields { id: Id; email: Email?; birthday: Date? }");
    let new = facts(&after_source);
    init(DB, &schema.0, &old).await.unwrap();
    let (db, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    db.execute(&format!("INSERT INTO {}.member(id) VALUES(1)", schema.0), &[]).await.unwrap();

    let migration = Some(json!({"backfill":[
        {"resource":"Member","field":"email","value":"user@example.test"},
        {"resource":"Member","field":"birthday","value":"2024-02-29"}
    ]}));
    let report = plan(DB, &schema.0, &new, &migration).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    assert_eq!(report["requiresReview"], true, "{report}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_to_email_with_invalid_existing_values_is_blocked() {
    let schema = OwnedSchema::new();
    let before_source = BASE.replace("fields { id: Id }", "fields { id: Id; email: Text? }");
    let after_source = BASE.replace("fields { id: Id }", "fields { id: Id; email: Email? }");
    let old = facts(&before_source);
    let new = facts(&after_source);
    init(DB, &schema.0, &old).await.unwrap();
    let (db, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    db.execute(&format!("INSERT INTO {}.member(id,email) VALUES(1,'not-an-email')", schema.0), &[]).await.unwrap();

    let report = plan(DB, &schema.0, &new, &None).await.unwrap();
    assert_eq!(report["blocked"], true, "{report}");
    assert!(
        report["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|change| { change["class"] == "Blocked" && change["reason"].as_str().unwrap_or("").contains("Email") }),
        "{report}"
    );
}
