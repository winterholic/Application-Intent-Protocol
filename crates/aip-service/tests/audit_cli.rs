//! `aip service audit` lets an operator read the business audit rows (who changed which row, through which path,
//! with the before and after values of the written fields) without writing SQL against the infrastructure schema.
use aip_service::{Audit, AuditQuery, Command};
use serde_json::{Value, json};

const DB: &str = "host=localhost dbname=postgres";
const DB_ENV: &str = "AIP_AUDIT_CLI_TEST_DATABASE";

struct Cleanup(Vec<String>, std::path::PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for schema in &self.0 {
            let _ =
                std::process::Command::new("psql").args(["-X", "-q", "-d", DB, "-c", &format!("DROP SCHEMA IF EXISTS {schema} CASCADE")]).output();
        }
        let _ = std::fs::remove_file(&self.1);
    }
}

async fn cli(config: &std::path::Path, command: Audit) -> Result<Value, Value> {
    aip_service::run(Command::Audit { config: config.to_path_buf(), command }).await
}

fn list() -> Audit {
    Audit::List(AuditQuery::default())
}

fn query(filter: AuditQuery) -> Audit {
    Audit::List(filter)
}

#[tokio::test]
async fn operator_reads_audit_rows_newest_first_with_filters_and_a_cursor() {
    let schema = format!("aip_audit_cli_{}", std::process::id());
    let infra = aip_execution::infrastructure_schema(&schema).unwrap();
    let config = std::env::temp_dir().join(format!("{schema}.json"));
    let _cleanup = Cleanup(vec![schema.clone(), infra.clone()], config.clone());
    let facts = spike_v1_fixture::load_str(include_str!("../../../spikes/spike-v6-transport/tests/fixtures/audit.aip"), spike_v1_fixture::Form::A)
        .unwrap()
        .execution;
    aip_migrate::init_with_wire(DB, &schema, &facts, "safe").await.unwrap();
    // SAFETY: this test binary sets one variable no other test reads, before any thread reads it.
    unsafe { std::env::set_var(DB_ENV, DB) };
    std::fs::write(
        &config,
        json!({"schema":schema,"database_url_env":DB_ENV,"auth":{"issuer":"test","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}}})
            .to_string(),
    )
    .unwrap();

    // Before any server has run there is no audit table: say so instead of failing on a missing relation.
    let missing = cli(&config, list()).await.unwrap_err();
    assert_eq!(missing["code"], "AUDIT_UNAVAILABLE", "{missing}");

    let db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    // The table the server installs at start (same columns as `spike_v3_write::audit_ddl`).
    db.batch_execute(&format!(
        "CREATE SCHEMA {infra}; CREATE TABLE {infra}.aip_audit(id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, recorded_at timestamptz NOT NULL DEFAULT now(), \
         actor_id bigint, via text NOT NULL, request_key text, resource text NOT NULL, transition text NOT NULL, target_id bigint NOT NULL, \
         before jsonb NOT NULL, after jsonb NOT NULL); \
         INSERT INTO {infra}.aip_audit(recorded_at,actor_id,via,request_key,resource,transition,target_id,before,after) VALUES \
          ('2026-10-01T00:00:00Z',1,'apply','k-1','Order','pay',11,'{{\"paid\":false}}','{{\"paid\":true}}'), \
          ('2026-10-02T00:00:00Z',1,'extension',NULL,'Order','pay',12,'{{\"paid\":false}}','{{\"paid\":true}}'), \
          ('2026-10-03T00:00:00Z',2,'job','run-1:effect-a','Order','pay',11,'{{\"paid\":true}}','{{\"paid\":false}}'), \
          ('2026-10-04T00:00:00Z',1,'apply','k-2','Member','rename',7,'{{\"name\":\"a\"}}','{{\"name\":\"b\"}}')"
    ))
    .await
    .unwrap();

    let all = cli(&config, list()).await.unwrap();
    let rows = all["audit"].as_array().unwrap();
    assert_eq!(rows.len(), 4, "{all}");
    assert_eq!(rows[0]["resource"], "Member", "newest first: {all}");
    assert_eq!(rows[0]["recordedAt"], "2026-10-04T00:00:00.000000Z");
    assert_eq!((rows[0]["actorId"].clone(), rows[0]["via"].clone(), rows[0]["requestKey"].clone()), (json!(1), json!("apply"), json!("k-2")));
    assert_eq!((rows[0]["before"].clone(), rows[0]["after"].clone()), (json!({"name":"a"}), json!({"name":"b"})));
    assert!(rows[2]["requestKey"].is_null() && rows[2]["via"] == "extension", "an extension write has no request key: {all}");

    let history =
        cli(&config, query(AuditQuery { resource: Some("Order".into()), target_id: Some("11".into()), ..Default::default() })).await.unwrap();
    let transitions: Vec<_> = history["audit"].as_array().unwrap().iter().map(|r| (r["via"].clone(), r["after"].clone())).collect();
    assert_eq!(transitions, vec![(json!("job"), json!({"paid":false})), (json!("apply"), json!({"paid":true}))], "{history}");

    let by_actor = cli(&config, query(AuditQuery { actor: Some(2), ..Default::default() })).await.unwrap();
    assert_eq!(by_actor["audit"].as_array().unwrap().len(), 1, "{by_actor}");
    let by_key = cli(&config, query(AuditQuery { request_key: Some("run-1:effect-a".into()), ..Default::default() })).await.unwrap();
    assert_eq!(by_key["audit"][0]["via"], "job", "{by_key}");
    let since = cli(&config, query(AuditQuery { since: Some("2026-10-03T00:00:00Z".into()), ..Default::default() })).await.unwrap();
    assert_eq!(since["audit"].as_array().unwrap().len(), 2, "{since}");

    let first = cli(&config, query(AuditQuery { limit: 3, ..Default::default() })).await.unwrap();
    assert_eq!(first["audit"].as_array().unwrap().len(), 3, "{first}");
    let cursor = first["nextBeforeId"].as_i64().expect("a full page names the cursor of the next one");
    let rest = cli(&config, query(AuditQuery { limit: 3, before_id: Some(cursor), ..Default::default() })).await.unwrap();
    assert_eq!(rest["audit"].as_array().unwrap().len(), 1, "{rest}");
    assert!(rest["nextBeforeId"].is_null(), "the last page has no cursor: {rest}");

    for bad in [
        query(AuditQuery { via: Some("sql".into()), ..Default::default() }),
        query(AuditQuery { limit: 0, ..Default::default() }),
        query(AuditQuery { limit: 501, ..Default::default() }),
        query(AuditQuery { since: Some("yesterday".into()), ..Default::default() }),
    ] {
        let refused = cli(&config, bad).await.unwrap_err();
        assert_eq!(refused["code"], "BAD_AUDIT_QUERY", "{refused}");
    }
}
