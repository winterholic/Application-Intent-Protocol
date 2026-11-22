//! PostgreSQL identifier quoting must keep two promises at once: names that are PostgreSQL keywords still work in the migration
//! SQL, and deployments created before keywords were quoted keep verifying because their stored DDL digest is unchanged.
use aip_migrate::{apply, creation, init, plan, preflight};
use serde_json::{Value, json};
use spike_v1_fixture::{Form, load_str};

const DB: &str = aip_test_support::test_database_url!();

fn facts(source: &str) -> Value {
    load_str(source, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution
}

async fn connect() -> tokio_postgres::Client {
    let (db, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    db
}

/// SHA-256 of the DDL the release before keyword quoting generated for this fixture under `aip_ref_baseline`. Every deployment
/// created by that release stores it in `aip_migrate_meta.ddl_digest` and the server compares it with the current generator on
/// every start, so changing it makes all of those deployments fail with `WIRE.SCHEMA_MISMATCH`.
const LEGACY_FIXTURE_DDL_DIGEST: &str = "1c7684a2ac974f1f05de5a0749f5586129557ae3eb27d8bdaf63a2ad633b1eba";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deployment_stored_by_the_release_before_keyword_quoting_still_verifies() {
    let schema = "aip_ref_baseline";
    let facts = facts(include_str!("../../../spikes/spike-v1-fixture/fixture/recruitment.aip"));
    let db = connect().await;
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await.unwrap();

    assert_eq!(creation(schema, &facts).unwrap().1, LEGACY_FIXTURE_DDL_DIGEST, "the generator output for unquoted-safe names must not change");
    init(DB, schema, &facts).await.unwrap();
    // The marker an older release wrote is exactly this digest; the live catalog is identical because quoting a lower-case name
    // never changes what PostgreSQL stores.
    db.execute(&format!("UPDATE {schema}.aip_migrate_meta SET ddl_digest = $1"), &[&LEGACY_FIXTURE_DDL_DIGEST]).await.unwrap();

    let preflight = preflight(DB, schema, &facts).await;
    let report = plan(DB, schema, &facts, &None).await;
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await.unwrap();
    assert_eq!(preflight.expect("existing deployment must still verify")["ok"], true);
    assert_eq!(report.expect("existing deployment must still plan")["blocked"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keyword_named_columns_can_be_added_backfilled_required_and_dropped() {
    let schema = format!("aip_kw_migrate_{}", std::process::id());
    let old = facts(
        r#"
resource Member { fields { id: Id; order: Text?; group: Text? } }
actor Member
"#,
    );
    let new = facts(
        r#"
resource Member { fields { id: Id; group: Text; user: Text } }
actor Member
"#,
    );
    let db = connect().await;
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await.unwrap();
    init(DB, &schema, &old).await.unwrap();
    db.batch_execute(&format!(r#"INSERT INTO {schema}.member(id, "order", "group") VALUES (1, 'o', 'g')"#)).await.unwrap();

    let migration = Some(json!({"backfill":[{"resource":"Member","field":"user","value":"u"}]}));
    let report = plan(DB, &schema, &new, &migration).await;
    let applied = match &report {
        Ok(report) => apply(DB, &schema, &new, &migration, report["digest"].as_str().unwrap()).await,
        Err(error) => Err(error.clone()),
    };
    let rows = db.query(&format!(r#"SELECT "group", "user" FROM {schema}.member"#), &[]).await;
    let columns =
        db.query("SELECT column_name FROM information_schema.columns WHERE table_schema = $1 AND table_name = 'member' ORDER BY 1", &[&schema]).await;
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await.unwrap();

    let report = report.expect("plan");
    assert_eq!(report["blocked"], false, "{report}");
    applied.expect("apply");
    let rows = rows.expect("keyword columns are readable after the migration");
    assert_eq!((rows[0].get::<_, String>(0), rows[0].get::<_, String>(1)), ("g".into(), "u".into()));
    let names: Vec<String> = columns.unwrap().iter().map(|row| row.get(0)).collect();
    assert_eq!(names, ["group", "id", "user"], "`order` is dropped, `group` and `user` remain");
}
