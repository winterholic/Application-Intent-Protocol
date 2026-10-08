use aip_migrate::{apply, init, plan};
use spike_v1_fixture::{Form, load_str};

const DB: &str = "host=localhost dbname=postgres";

#[tokio::test]
async fn migration_adds_nullable_typed_id_as_plain_bigint() {
    let before =
        load_str("resource Member { fields { id: Id } } actor Member resource Item { fields { id: Id; owner: Member } }", Form::A).unwrap().execution;
    let after =
        load_str("resource Member { fields { id: Id } } actor Member resource Item { fields { id: Id; owner: Member; next: Item.Id? } }", Form::A)
            .unwrap()
            .execution;
    let schema = format!(
        "aip_typed_id_migrate_{}_{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    );
    init(DB, &schema, &before).await.unwrap();
    let (db, connection) = tokio_postgres::connect(DB, tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES(1); INSERT INTO {schema}.item(id,owner_id) VALUES(10,1)")).await.unwrap();
    let report = plan(DB, &schema, &after, &None).await.unwrap();
    assert_eq!(report["blocked"], false, "{report}");
    let digest = report["digest"].as_str().unwrap();
    apply(DB, &schema, &after, &None, digest).await.unwrap();
    let column: String = db
        .query_one("SELECT data_type FROM information_schema.columns WHERE table_schema=$1 AND table_name='item' AND column_name='next'", &[&schema])
        .await
        .unwrap()
        .get(0);
    assert_eq!(column, "bigint");
    let old_value: Option<i64> = db.query_one(&format!("SELECT next FROM {schema}.item WHERE id=10"), &[]).await.unwrap().get(0);
    assert_eq!(old_value, None);
    db.execute(&format!("UPDATE {schema}.item SET next=999 WHERE id=10"), &[]).await.unwrap();
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
