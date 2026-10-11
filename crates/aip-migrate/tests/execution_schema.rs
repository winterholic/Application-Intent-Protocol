use serde_json::json;
use spike_v1_fixture::{Form, load_str};

#[test]
fn job_declarations_add_managed_storage_without_changing_sync_schemas() {
    let src = r#"actor principal operation read count { input {} output { count: Int }
       allow actor != null effect none deadline 1s implementation "compute.count" }"#;
    let sync = load_str(src, Form::A).unwrap().execution;
    let job = load_str(&src.replace("operation read", "operation job"), Form::A).unwrap().execution;
    let (sync_ddl, _) = aip_migrate::creation("aip_execution_schema", &sync).unwrap();
    let (ddl, _) = aip_migrate::creation("aip_execution_schema", &job).unwrap();
    assert!(!sync_ddl.join("\n").contains("aip_jobs"));
    for table in ["aip_jobs", "aip_files", "aip_events", "aip_effects"] {
        assert!(ddl.iter().any(|sql| sql.starts_with(&format!("CREATE TABLE aip_execution_schema.{table} "))), "{table}: {}", json!(ddl));
    }
}
