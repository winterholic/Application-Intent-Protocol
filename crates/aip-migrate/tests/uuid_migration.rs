use serde_json::json;
use spike_v1_fixture::{Form, load_str};
const URL: &str = aip_test_support::test_database_url!();
#[tokio::test]
async fn an_owned_numeric_app_can_add_uuid_resources_and_backfill_uuid_refs() {
    let schema = format!("aip_uuid_migrate_{}", std::process::id());
    let base = "actor Member resource Member {fields {id: Id}}";
    let with_note = format!("{base} resource Note {{fields {{id: Uuid; title: Text}}}}");
    let with_ref = with_note.replace("title: Text", "title: Text; original: Note");
    let a = load_str(base, Form::A).unwrap().execution;
    let b = load_str(&with_note, Form::A).unwrap().execution;
    let c = load_str(&with_ref, Form::A).unwrap().execution;
    let db = spike_v2_read::connect_owned_with_url(URL).await.unwrap();
    let result = async {
        aip_migrate::init_with_wire(URL, &schema, &a, "decimal").await?;
        let p = aip_migrate::plan(URL, &schema, &b, &None).await?;
        assert_eq!(p["blocked"], false, "{p}");
        aip_migrate::apply(URL, &schema, &b, &None, p["digest"].as_str().unwrap()).await?;
        let id = "00000000-0000-4000-8000-000000000001";
        db.execute(&format!("INSERT INTO {schema}.note(id,title) VALUES($1::text::uuid,'seed')"), &[&id]).await.unwrap();
        let directive = Some(json!({"backfill":[{"resource":"Note","field":"original","value":id}]}));
        let p = aip_migrate::plan(URL, &schema, &c, &directive).await?;
        aip_migrate::apply(URL, &schema, &c, &directive, p["digest"].as_str().unwrap()).await?;
        let original: String = db.query_one(&format!("SELECT original_id::text FROM {schema}.note"), &[]).await.unwrap().get(0);
        assert_eq!(original, id);
        aip_migrate::preflight(URL, &schema, &c).await?;
        Ok::<_, serde_json::Value>(())
    }
    .await;
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await.unwrap();
    result.unwrap();
}
