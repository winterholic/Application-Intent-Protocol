//! External storage is never mutated by this module. Snapshots require only catalog SELECT.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio_postgres::GenericClient;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub schema: String,
    pub tables: BTreeMap<String, Table>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Table {
    pub table: String,
    pub columns: BTreeMap<String, String>,
    /// Permit the legacy shared-PK shape only when the source model intentionally has no physical FK.
    #[serde(default)]
    pub shared_pk_without_fk: bool,
}
pub fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}
pub fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 63 && !s.contains('\0')
}
pub async fn snapshot(db: &impl GenericClient, schema: &str) -> Result<Value, tokio_postgres::Error> {
    let row = db.query_one(include_str!("../../../prototype/src/schema_catalog.sql"), &[&schema]).await?;
    let text: String = row.get(0);
    let mut v: Value = serde_json::from_str(&text).expect("catalog generates JSON");
    let funcs=db.query("SELECT p.proname,pg_get_function_identity_arguments(p.oid),pg_get_functiondef(p.oid) FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=$1 AND p.prokind='f' ORDER BY p.proname,pg_get_function_identity_arguments(p.oid)",&[&schema]).await?;
    v["functions"] = json!(funcs.iter().map(|r| json!([r.get::<_, String>(0), r.get::<_, String>(1), r.get::<_, String>(2)])).collect::<Vec<_>>());
    Ok(v)
}
/// Fail closed on catalog failures and any external DDL drift. Called before each product request.
pub async fn matches(db: &impl GenericClient, facts: &Value) -> Result<bool, tokio_postgres::Error> {
    if facts["externalStorage"].is_null() {
        return Ok(true);
    }
    let storage = &facts["externalStorage"];
    let Some(schema) = storage["mapping"]["schema"].as_str() else { return Ok(false) };
    Ok(storage["structureDigest"] == spike_v1_fixture::digest(&snapshot(db, schema).await?))
}
