//! PostgreSQL deployment of the V1 execution facts. All database changes are explicit CLI operations.
pub mod adopt;
pub mod external;
pub mod mysql_catalog;
pub mod mysql_external;
#[cfg(feature = "mysql")]
pub mod mysql_adopt;
#[cfg(feature = "mysql")]
pub mod mysql_backend;
#[cfg(feature = "mysql")]
pub mod mysql_driver;
#[cfg(feature = "mysql")]
pub mod mysql_write;
use serde_json::{Value, json};
use spike_v1_fixture::digest;
use spike_v2_read::{id_wire::IdWire, sqlgen};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_postgres::GenericClient;

const VERSION: i32 = 1;
const CATALOG: &str = include_str!("../../../prototype/src/schema_catalog.sql");
const META: &str = "aip_migrate_meta";
const JOURNAL: &str = "aip_migrate_journal";
const EXECUTION_TABLES: &[&str] = &["aip_jobs", "aip_files", "aip_events", "aip_event_streams", "aip_event_keys", "aip_effects", "aip_messages"];
static NEXT_SHADOW: AtomicU64 = AtomicU64::new(0);

fn err(code: &str, message: &str) -> Value {
    json!({"ok":false,"code":code,"message":message})
}

fn valid_ident(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty() && bytes.len() <= 63 && bytes[0].is_ascii_alphabetic() && bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

fn valid_field_type(ty: &str) -> bool {
    let base = ty.trim_end_matches('?');
    if matches!(base, "Text" | "Email" | "Url" | "Time" | "Date" | "Int" | "Bool") {
        return true;
    }
    if decimal_type(base).is_some() {
        return true;
    }
    for prefix in ["Id<", "Ref<", "Enum<"] {
        if let Some(inner) = base.strip_prefix(prefix).and_then(|s| s.strip_suffix('>')) {
            return valid_ident(inner);
        }
    }
    false
}

fn decimal_type(ty: &str) -> Option<(u8, u8)> {
    let inner = ty.strip_prefix("Decimal<")?.strip_suffix('>')?;
    let (precision, scale) = inner.split_once(',')?;
    let canonical_uint = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'));
    if !canonical_uint(precision) || !canonical_uint(scale) {
        return None;
    }
    let (precision, scale) = (precision.parse::<u8>().ok()?, scale.parse::<u8>().ok()?);
    ((1..=38).contains(&precision) && scale <= precision).then_some((precision, scale))
}

fn validate_facts(facts: &Value) -> Result<(), Value> {
    let resources = facts["resources"].as_object().ok_or_else(|| err("BAD_FACTS", "resources 객체 누락"))?;
    let enums = facts["enums"].as_object().ok_or_else(|| err("BAD_FACTS", "enums 객체 누락"))?;
    let mut tables = BTreeSet::new();
    for (name, values) in enums {
        if !valid_ident(name) || !values.as_array().is_some_and(|a| a.iter().all(|v| v.as_str().is_some_and(valid_ident))) {
            return Err(err("BAD_FACTS", "enum 이름/값 오류"));
        }
    }
    for (name, resource) in resources {
        let table = sqlgen::snake(name);
        if !valid_ident(name) || !valid_ident(&table) || !tables.insert(table) {
            return Err(err("BAD_FACTS", "resource 식별자 오류"));
        }
        if !resource["idKind"].is_null() && resource["idKind"] != "uuid" {
            return Err(err("BAD_FACTS", "지원하지 않는 Id 종류"));
        }
        let fields = resource["fields"].as_object().ok_or_else(|| err("BAD_FACTS", "fields 객체 누락"))?;
        if !resource["unique"].is_array() || !resource["invariants"].is_object() {
            return Err(err("BAD_FACTS", "unique/invariants 형식 오류"));
        }
        let mut columns = BTreeSet::new();
        for (field, desc) in fields {
            let ty = desc["ty"].as_str().ok_or_else(|| err("BAD_FACTS", "필드 타입 누락"))?;
            let base = ty.trim_end_matches('?');
            if let Some(target) = base.strip_prefix("Ref<").and_then(|s| s.strip_suffix('>'))
                && !resources.contains_key(target)
            {
                return Err(err("BAD_FACTS", "Ref 대상 resource 누락"));
            }
            if let Some(target) = base.strip_prefix("Enum<").and_then(|s| s.strip_suffix('>'))
                && !enums.contains_key(target)
            {
                return Err(err("BAD_FACTS", "Enum 선언 누락"));
            }
            let col = if ty.starts_with("Ref<") { format!("{}_id", sqlgen::snake(field)) } else { sqlgen::snake(field) };
            if !valid_ident(field) || !valid_ident(&col) || !columns.insert(col) || !valid_field_type(ty) {
                return Err(err("BAD_FACTS", "필드 이름/타입 오류"));
            }
            if !desc["range"].is_null() {
                let Some(range) = desc["range"].as_array() else {
                    return Err(err("BAD_FACTS", "range 형식 오류"));
                };
                if range.len() != 2 || range[0].as_i64().is_none() || range[1].as_i64().is_none() || range[0].as_i64() > range[1].as_i64() {
                    return Err(err("BAD_FACTS", "range 정수 오류"));
                }
            }
        }
        for item in resource["unique"].as_array().into_iter().flatten() {
            if !item.as_array().is_some_and(|a| a.iter().all(|v| v.as_str().is_some_and(|s| fields.contains_key(s)))) {
                return Err(err("BAD_FACTS", "unique 필드 오류"));
            }
        }
        for key in resource["invariants"].as_object().into_iter().flat_map(|m| m.keys()) {
            if !valid_ident(key) {
                return Err(err("BAD_FACTS", "invariant 이름 오류"));
            }
        }
    }
    if let Some(operations) = facts["operations"].as_object() {
        for operation in operations.values() {
            if let Some(accesses) = operation["access"].as_object() {
                for (access, enabled) in accesses {
                    if let Some(queue) = access.strip_prefix("messages.")
                        && (operation["kind"] != "job"
                            || operation["effect"] != "managed"
                            || *enabled != Value::Bool(true)
                            || !valid_message_queue(queue))
                    {
                        return Err(err("BAD_FACTS", "messages capability는 managed job의 안전한 queue 이름이어야 함"));
                    }
                }
            }
        }
    }
    let mut pending = vec![facts];
    let mut visited = 0usize;
    while let Some(node) = pending.pop() {
        visited += 1;
        if visited > 100_000 {
            return Err(err("BAD_FACTS", "정의 노드 수 초과"));
        }
        match node {
            Value::Array(values) => pending.extend(values),
            Value::Object(values) => {
                if let Some(op) = values.get("cmp")
                    && !op.as_str().is_some_and(|s| matches!(s, "=" | "!=" | "<" | "<=" | ">" | ">="))
                {
                    return Err(err("BAD_FACTS", "정책 비교 연산자 오류"));
                }
                if let Some(target) = values.get("exists")
                    && !target.as_str().is_some_and(|s| resources.contains_key(s))
                {
                    return Err(err("BAD_FACTS", "정책 exists 대상 오류"));
                }
                pending.extend(values.values());
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn configure_schema(schema: &str) -> Result<(), Value> {
    let b = schema.as_bytes();
    if b.len() > 63
        || !b.starts_with(b"aip_")
        || !b.get(4).is_some_and(u8::is_ascii_lowercase)
        || !b[5..].iter().all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || *x == b'_')
    {
        return Err(err("BAD_SCHEMA", "schema는 aip_로 시작하는 63바이트 이하 ASCII 식별자여야 함"));
    }
    Ok(())
}

pub fn creation(schema: &str, facts: &Value) -> Result<(Vec<String>, String), Value> {
    configure_schema(schema)?;
    validate_facts(facts)?;
    if facts["externalStorage"].is_null()
        && facts["resources"]
            .as_object()
            .is_some_and(|rs| rs.values().any(|r| r["fields"].as_object().is_some_and(|fs| fs.values().any(|f| f["databaseDefault"] == true))))
    {
        return Err(err("EXTERNAL_DEFAULT_REQUIRED", "default database는 외부 채택에만 사용한다"));
    }
    let ddl = sqlgen::create_ddl_in(schema, facts).map_err(|_| err("UNSUPPORTED_SCHEMA", "정의의 SQL 구조를 생성할 수 없음"))?;
    let hash = digest(&json!(ddl));
    Ok((ddl, hash))
}

fn valid_message_queue(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else { return false };
    value.len() <= 128 && (first.is_ascii_alphabetic() || first == b'_') && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn has_message_access(facts: &Value) -> bool {
    facts["operations"].as_object().is_some_and(|operations| {
        operations
            .values()
            .any(|operation| operation["access"].as_object().is_some_and(|access| access.keys().any(|name| name.starts_with("messages."))))
    })
}

async fn connect(db_url: &str) -> Result<spike_v2_read::OwnedConnection, Value> {
    tokio::time::timeout(std::time::Duration::from_secs(5), spike_v2_read::connect_owned_with_url(db_url))
        .await
        .map_err(|_| err("DB_CONNECT", "DB 연결 시간 초과"))?
        .map_err(|_| err("DB_CONNECT", "DB 연결 실패"))
}

async fn catalog(db: &impl GenericClient, schema: &str) -> Result<Value, Value> {
    db.batch_execute("SET LOCAL search_path TO pg_catalog").await.map_err(|_| err("DB_CATALOG", "catalog 설정 실패"))?;
    let row = db.query_one(CATALOG, &[&schema]).await.map_err(|_| err("DB_CATALOG", "catalog 조회 실패"))?;
    let text: String = row.get(0);
    let mut value: Value = serde_json::from_str(&text).map_err(|_| err("DB_CATALOG", "catalog 형식 오류"))?;
    let functions = db.query("SELECT p.proname,pg_get_function_identity_arguments(p.oid),pg_get_functiondef(p.oid) FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=$1 ORDER BY p.proname,pg_get_function_identity_arguments(p.oid)", &[&schema])
        .await.map_err(|_| err("DB_CATALOG", "함수 정의 조회 실패"))?;
    value["functions"] =
        Value::Array(functions.iter().map(|row| json!([row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2)])).collect());
    Ok(value)
}

async fn structure(db: &impl GenericClient, schema: &str) -> Result<String, Value> {
    Ok(digest(&json!({"format":"prototype-catalog-v1","catalog":catalog(db,schema).await?})))
}

fn normalized_catalog(mut value: Value, from: &str, to: &str) -> Value {
    if let Some(relations) = value["relations"].as_array_mut() {
        for relation in relations {
            if let Some(columns) = relation.get_mut(5).and_then(Value::as_array_mut) {
                for column in columns.iter_mut() {
                    if let Some(parts) = column.as_array_mut()
                        && parts.len() > 1
                    {
                        parts.remove(1);
                    }
                }
                columns.sort_by(|a, b| a[0].as_str().cmp(&b[0].as_str()));
            }
        }
    }
    fn rewrite(v: &mut Value, from: &str, to: &str) {
        match v {
            Value::String(s) => *s = s.replace(from, to),
            Value::Array(a) => {
                for x in a {
                    rewrite(x, from, to);
                }
            }
            Value::Object(o) => {
                for x in o.values_mut() {
                    rewrite(x, from, to);
                }
            }
            _ => {}
        }
    }
    rewrite(&mut value, from, to);
    value
}

async fn verify_target(db: &impl GenericClient, schema: &str, facts: &Value) -> Result<(), Value> {
    let shadow = format!("aip_migration_shadow_{}_{}", std::process::id(), NEXT_SHADOW.fetch_add(1, Ordering::Relaxed));
    if serde_json::to_string(facts).map_err(|_| err("TARGET_CHECK", "정의 직렬화 실패"))?.contains(&shadow) {
        return Err(err("TARGET_CHECK", "임시 schema 이름이 정의와 겹침"));
    }
    let (ddl, _) = creation(&shadow, facts)?;
    for sql in &ddl {
        db.batch_execute(sql).await.map_err(|_| err("TARGET_CHECK", "목표 구조 생성 실패"))?;
    }
    db.batch_execute(&metadata_sql(&shadow, facts)?).await.map_err(|_| err("TARGET_CHECK", "목표 metadata 생성 실패"))?;
    let expected = normalized_catalog(catalog(db, &shadow).await?, &shadow, schema);
    let actual = normalized_catalog(catalog(db, schema).await?, schema, schema);
    if expected != actual {
        return Err(err("TARGET_MISMATCH", "변경 후 DB 구조가 새 정의의 fresh schema와 다름"));
    }
    db.batch_execute(&format!("DROP SCHEMA {shadow} CASCADE")).await.map_err(|_| err("TARGET_CHECK", "목표 임시 schema 정리 실패"))?;
    Ok(())
}

fn metadata_sql(schema: &str, facts: &Value) -> Result<String, Value> {
    let actor = facts["actor"].as_str().ok_or_else(|| err("UNSUPPORTED_SCHEMA", "운영 actor resource가 필요함"))?;
    let managed = facts["actorMode"] == "principal" && actor == "principal" && !facts["resources"].get(actor).is_some_and(Value::is_object);
    if !managed && (!facts["actorMode"].is_null() || !facts["resources"].get(actor).is_some_and(Value::is_object)) {
        return Err(err("UNSUPPORTED_SCHEMA", "actor resource 누락"));
    }
    let uuid = spike_v2_read::id_wire::id_kind(facts, actor) == "uuid";
    let external = !facts["externalStorage"].is_null();
    let actor_table = if uuid {
        "aip_actor_identities".to_string()
    } else if managed || external {
        "aip_actors".to_string()
    } else {
        sqlgen::snake(actor)
    };
    let identities = if uuid {
        format!(
            "CREATE TABLE {schema}.aip_actor_identities(id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, resource_id uuid NOT NULL UNIQUE); CREATE FUNCTION {schema}.aip_reject_actor_resource_change() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.resource_id IS DISTINCT FROM OLD.resource_id THEN RAISE EXCEPTION 'actor resource identity is immutable' USING ERRCODE='23514'; END IF; RETURN NEW; END $$; CREATE TRIGGER aip_actor_resource_immutable BEFORE UPDATE OF resource_id ON {schema}.aip_actor_identities FOR EACH ROW EXECUTE FUNCTION {schema}.aip_reject_actor_resource_change(); "
        )
    } else if managed || external {
        format!("CREATE TABLE {schema}.aip_actors (id bigint PRIMARY KEY CHECK(id>0)); ")
    } else {
        String::new()
    };
    Ok(format!(
        "{identities}CREATE TABLE {schema}.{META} (singleton boolean PRIMARY KEY CHECK(singleton), facts jsonb NOT NULL, facts_digest text NOT NULL, ddl_digest text NOT NULL, structure_digest text NOT NULL, generator_version integer NOT NULL, runtime_wire text CHECK(runtime_wire IN ('safe','decimal'))); CREATE TABLE {schema}.{JOURNAL} (id bigint GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, plan_digest text NOT NULL, old_digest text, new_digest text NOT NULL, steps jsonb NOT NULL, approval text, applied_at timestamptz NOT NULL DEFAULT now()); CREATE TABLE {schema}.aip_principals (issuer text NOT NULL, subject text NOT NULL, actor_id bigint NOT NULL REFERENCES {schema}.{actor_table}(id), enabled boolean NOT NULL DEFAULT true, min_iat bigint NOT NULL DEFAULT 0, PRIMARY KEY (issuer,subject)); CREATE FUNCTION {schema}.aip_reject_principal_actor_change() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.actor_id IS DISTINCT FROM OLD.actor_id THEN RAISE EXCEPTION 'principal actor_id is immutable' USING ERRCODE='23514'; END IF; RETURN NEW; END $$; CREATE TRIGGER aip_principal_actor_immutable BEFORE UPDATE OF actor_id ON {schema}.aip_principals FOR EACH ROW EXECUTE FUNCTION {schema}.aip_reject_principal_actor_change(); CREATE TABLE {schema}.aip_idem (principal text NOT NULL, key text NOT NULL, request text NOT NULL, result jsonb NOT NULL, PRIMARY KEY (principal,key))"
    ))
}

pub async fn init(db_url: &str, schema: &str, facts: &Value) -> Result<Value, Value> {
    tokio::time::timeout(std::time::Duration::from_secs(25), init_inner(db_url, schema, facts, None))
        .await
        .map_err(|_| err("INIT_UNSETTLED", "초기화 기한 초과; journal과 catalog로 커밋 여부 확인 필요"))?
}

pub async fn init_with_wire(db_url: &str, schema: &str, facts: &Value, wire: &str) -> Result<Value, Value> {
    tokio::time::timeout(std::time::Duration::from_secs(25), init_inner(db_url, schema, facts, Some(wire)))
        .await
        .map_err(|_| err("INIT_UNSETTLED", "초기화 기한 초과; journal과 catalog로 커밋 여부 확인 필요"))?
}

fn wire_label(wire: &str) -> Result<&'static str, Value> {
    match wire {
        "safe" => Ok(IdWire::SafeNumber.label()),
        "decimal" => Ok(IdWire::DecimalString.label()),
        _ => Err(err("BAD_WIRE_MODE", "wire는 safe 또는 decimal이어야 함")),
    }
}

pub(crate) async fn validate_existing_wire(db: &impl GenericClient, schema: &str, wire: &str) -> Result<(), Value> {
    configure_schema(schema)?;
    let label = wire_label(wire)?;
    let pattern = format!("^(actor:-?[0-9]+|anonymous):ids:{label}$");
    let incompatible: bool = db
        .query_one(&format!("SELECT EXISTS(SELECT 1 FROM {schema}.aip_idem WHERE principal !~ $1)"), &[&pattern])
        .await
        .map_err(|_| err("DB_WIRE_BIND", "기존 멱등성 principal 확인 실패"))?
        .get(0);
    if incompatible {
        return Err(err("WIRE_MODE_MISMATCH", "기존 멱등성 기록에 다른 wire 또는 알 수 없는 principal이 있음"));
    }
    Ok(())
}

async fn init_inner(db_url: &str, schema: &str, facts: &Value, wire: Option<&str>) -> Result<Value, Value> {
    if let Some(wire) = wire {
        wire_label(wire)?;
    }
    let (ddl, ddl_digest) = creation(schema, facts)?;
    if facts["resources"]
        .as_object()
        .is_some_and(|rs| rs.values().any(|r| r["fields"].as_object().is_some_and(|fs| fs.values().any(|f| f["databaseDefault"] == true))))
    {
        return Err(err("EXTERNAL_DEFAULT_REQUIRED", "default database는 외부 채택에만 사용한다"));
    }
    let mut db = connect(db_url).await?;
    let tx = db.transaction().await.map_err(|_| err("DB_INIT", "초기화 트랜잭션 시작 실패"))?;
    tx.batch_execute("SET LOCAL lock_timeout='2s'; SET LOCAL statement_timeout='20s'").await.map_err(|_| err("DB_INIT", "초기화 기한 설정 실패"))?;
    for s in &ddl {
        tx.batch_execute(s).await.map_err(|_| err("DB_INIT", "초기화 SQL 실패; 트랜잭션 취소"))?;
    }
    tx.batch_execute(&metadata_sql(schema, facts)?).await.map_err(|_| err("DB_INIT", "metadata 생성 실패"))?;
    let structure_digest = structure(&tx, schema).await?;
    let facts_digest = digest(facts);
    tx.execute(
        &format!("INSERT INTO {schema}.{META} (singleton,facts,facts_digest,ddl_digest,structure_digest,generator_version,runtime_wire) VALUES (true,$1,$2,$3,$4,$5,$6)"),
        &[facts, &facts_digest, &ddl_digest, &structure_digest, &VERSION, &wire],
    )
    .await
    .map_err(|_| err("DB_INIT", "metadata 저장 실패"))?;
    tx.execute(
        &format!("INSERT INTO {schema}.{JOURNAL} (plan_digest,old_digest,new_digest,steps) VALUES ($1,NULL,$2,$3)"),
        &[&format!("init:{facts_digest}"), &facts_digest, &json!(ddl)],
    )
    .await
    .map_err(|_| err("DB_INIT", "journal 저장 실패"))?;
    tx.commit().await.map_err(|_| err("COMMIT_UNSETTLED", "초기화 커밋 결과 불명; preflight로 확인 필요"))?;
    Ok(json!({"ok":true,"schema":schema,"factsDigest":facts_digest,"ddlDigest":ddl_digest,"structureDigest":structure_digest}))
}

#[derive(Clone)]
struct Marker {
    facts: Value,
    facts_digest: String,
    ddl_digest: String,
    structure_digest: String,
    generator_version: i32,
}

async fn marker(db: &impl GenericClient, schema: &str) -> Result<Marker, Value> {
    let row = db
        .query_opt(&format!("SELECT facts,facts_digest,ddl_digest,structure_digest,generator_version FROM {schema}.{META} WHERE singleton=true"), &[])
        .await
        .map_err(|_| err("SCHEMA_NOT_READY", "migration metadata가 없는 schema; 기존 marker 자동 채택 거부"))?
        .ok_or_else(|| err("SCHEMA_NOT_READY", "migration metadata가 비어 있음"))?;
    let m =
        Marker { facts: row.get(0), facts_digest: row.get(1), ddl_digest: row.get(2), structure_digest: row.get(3), generator_version: row.get(4) };
    let journal = db
        .query_opt(&format!("SELECT new_digest FROM {schema}.{JOURNAL} ORDER BY id DESC LIMIT 1"), &[])
        .await
        .map_err(|_| err("SCHEMA_NOT_READY", "migration journal 조회 실패"))?
        .ok_or_else(|| err("SCHEMA_NOT_READY", "migration journal이 비어 있음"))?;
    let journal_digest: String = journal.get(0);
    if journal_digest != m.facts_digest {
        return Err(err("WIRE.SCHEMA_MISMATCH", "metadata와 journal의 마지막 정의가 다름"));
    }
    Ok(m)
}

async fn verified_marker(db: &impl GenericClient, schema: &str) -> Result<Marker, Value> {
    let m = marker(db, schema).await?;
    if m.generator_version != VERSION || m.facts_digest != digest(&m.facts) || m.ddl_digest != creation(schema, &m.facts)?.1 {
        return Err(err("WIRE.SCHEMA_MISMATCH", "metadata의 facts 또는 생성기 버전 불일치"));
    }
    if m.structure_digest != structure(db, schema).await? {
        return Err(err("WIRE.SCHEMA_MISMATCH", "실제 DB 구조가 journal 구조와 다름"));
    }
    Ok(m)
}

/// Recognizes only the durable-job DDL generated immediately before `aip_jobs.trace_id` was added.
/// The stored catalog digest still has to match exactly, so any other database drift remains fatal.
async fn verified_marker_for_migration(db: &impl GenericClient, schema: &str) -> Result<(Marker, bool), Value> {
    let m = marker(db, schema).await?;
    if m.generator_version != VERSION || m.facts_digest != digest(&m.facts) {
        return Err(err("WIRE.SCHEMA_MISMATCH", "metadata의 facts 또는 생성기 버전 불일치"));
    }
    let current_ddl_digest = creation(schema, &m.facts)?.1;
    let legacy_trace_digest = if m.ddl_digest != current_ddl_digest { Some(parent_job_ddl_digest(schema, &m.facts)?) } else { None };
    let legacy_trace_upgrade = legacy_trace_digest.as_deref() == Some(m.ddl_digest.as_str());
    if m.ddl_digest != current_ddl_digest && !legacy_trace_upgrade {
        return Err(err("WIRE.SCHEMA_MISMATCH", "metadata의 facts 또는 생성기 버전 불일치"));
    }
    if m.structure_digest != structure(db, schema).await? {
        return Err(err("WIRE.SCHEMA_MISMATCH", "실제 DB 구조가 journal 구조와 다름"));
    }
    if legacy_trace_upgrade {
        let trace_column_exists: bool = db
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_schema=$1 AND table_name='aip_jobs' AND column_name='trace_id')",
                &[&schema],
            )
            .await
            .map_err(|_| err("DB_CATALOG", "durable job catalog 조회 실패"))?
            .get(0);
        if trace_column_exists {
            return Err(err("WIRE.SCHEMA_MISMATCH", "이전 durable job DDL marker와 실제 trace_id 구조가 다름"));
        }
    }
    Ok((m, legacy_trace_upgrade))
}

fn parent_job_ddl_digest(schema: &str, facts: &Value) -> Result<String, Value> {
    if !facts["operations"].as_object().is_some_and(|operations| operations.values().any(|operation| operation["kind"] == "job")) {
        return Err(err("WIRE.SCHEMA_MISMATCH", "알 수 없는 migration DDL marker"));
    }
    let (mut ddl, _) = creation(schema, facts)?;
    let jobs = ddl
        .iter_mut()
        .find(|statement| statement.starts_with(&format!("CREATE TABLE {schema}.aip_jobs ")))
        .ok_or_else(|| err("WIRE.SCHEMA_MISMATCH", "알 수 없는 migration DDL marker"))?;
    if jobs.matches("trace_id text").count() != 1 {
        return Err(err("WIRE.SCHEMA_MISMATCH", "durable job DDL이 알려진 이전 버전과 다름"));
    }
    *jobs = jobs.replace("trace_id text,", "");
    Ok(digest(&json!(ddl)))
}

pub async fn preflight(db_url: &str, schema: &str, facts: &Value) -> Result<Value, Value> {
    tokio::time::timeout(std::time::Duration::from_secs(12), preflight_inner(db_url, schema, facts))
        .await
        .map_err(|_| err("DB_PREFLIGHT_TIMEOUT", "기동 검사 기한 초과"))?
}

async fn preflight_inner(db_url: &str, schema: &str, facts: &Value) -> Result<Value, Value> {
    configure_schema(schema)?;
    let mut db = connect(db_url).await?;
    let tx = db.transaction().await.map_err(|_| err("DB_PREFLIGHT", "트랜잭션 시작 실패"))?;
    tx.batch_execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY; SET LOCAL statement_timeout='10s'")
        .await
        .map_err(|_| err("DB_PREFLIGHT", "snapshot 설정 실패"))?;
    let m = verified_marker(&tx, schema).await?;
    if !spike_v2_read::external::matches(&tx, facts).await.map_err(|_| err("DB_CATALOG", "외부 catalog 조회 실패"))? {
        return Err(err("WIRE.SCHEMA_MISMATCH", "외부 업무 schema에 drift가 있음"));
    }
    let (_, ddl_digest) = creation(schema, facts)?;
    if m.facts_digest != digest(facts) || m.ddl_digest != ddl_digest {
        return Err(err("WIRE.SCHEMA_MISMATCH", "정의와 배포된 facts/DDL이 다름"));
    }
    tx.commit().await.map_err(|_| err("DB_PREFLIGHT", "snapshot 종료 실패"))?;
    Ok(json!({"ok":true,"schema":schema,"factsDigest":m.facts_digest,"structureDigest":m.structure_digest}))
}

pub async fn bind_wire(db_url: &str, schema: &str, wire: &str) -> Result<Value, Value> {
    tokio::time::timeout(std::time::Duration::from_secs(25), bind_wire_inner(db_url, schema, wire))
        .await
        .map_err(|_| err("WIRE_BIND_UNSETTLED", "wire 고정 기한 초과; metadata를 조회해 커밋 여부 확인 필요"))?
}

async fn bind_wire_inner(db_url: &str, schema: &str, wire: &str) -> Result<Value, Value> {
    configure_schema(schema)?;
    wire_label(wire)?;
    let mut db = connect(db_url).await?;
    let tx = db.transaction().await.map_err(|_| err("DB_WIRE_BIND", "wire 고정 트랜잭션 시작 실패"))?;
    tx.batch_execute("SET LOCAL lock_timeout='2s'; SET LOCAL statement_timeout='20s'")
        .await
        .map_err(|_| err("DB_WIRE_BIND", "wire 고정 기한 설정 실패"))?;
    tx.execute("SELECT pg_advisory_xact_lock(1095323725, hashtext($1))", &[&schema]).await.map_err(|_| err("DB_LOCK", "wire 고정 잠금 실패"))?;
    let external = !marker(&tx, schema).await?.facts["externalStorage"].is_null();
    if !external {
        lock_managed_tables(&tx, schema).await?;
    }
    verified_marker(&tx, schema).await?;
    let row = tx
        .query_one(&format!("SELECT runtime_wire FROM {schema}.{META} WHERE singleton=true FOR UPDATE"), &[])
        .await
        .map_err(|_| err("SCHEMA_NOT_READY", "wire metadata 조회 실패"))?;
    let bound: Option<String> = row.get(0);
    if let Some(bound) = bound {
        if bound != wire {
            return Err(err("WIRE_MODE_MISMATCH", "기존 배포 wire와 요청 wire가 다름"));
        }
        tx.commit().await.map_err(|_| err("COMMIT_UNSETTLED", "wire 재조회 종료 결과 불명"))?;
        return Ok(json!({"ok":true,"schema":schema,"wire":wire,"alreadyBound":true}));
    }
    if external {
        return Err(err("WIRE_MODE_MISMATCH", "외부 채택은 배포 시 wire가 고정되어야 함"));
    }
    validate_existing_wire(&tx, schema, wire).await?;
    tx.execute(&format!("UPDATE {schema}.{META} SET runtime_wire=$1 WHERE singleton=true"), &[&wire])
        .await
        .map_err(|_| err("DB_WIRE_BIND", "wire metadata 저장 실패"))?;
    tx.commit().await.map_err(|_| err("COMMIT_UNSETTLED", "wire 고정 커밋 결과 불명; metadata 조회 필요"))?;
    Ok(json!({"ok":true,"schema":schema,"wire":wire,"alreadyBound":false}))
}

#[derive(Clone)]
struct Step {
    sql: String,
    class: &'static str,
    value: Option<String>,
    reason: String,
}
impl Step {
    fn json(&self) -> Value {
        json!({"sql":self.sql,"class":self.class,"value":self.value,"reason":self.reason})
    }
}

fn split_top(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let (mut depth, mut single, mut double) = (0i32, false, false);
    for c in s.chars() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '(' if !single && !double => depth += 1,
            ')' if !single && !double => depth -= 1,
            ',' if depth == 0 && !single && !double => {
                out.push(current.trim().to_string());
                current.clear();
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

fn table_defs(ddl: &[String], schema: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for statement in ddl {
        let Some(rest) = statement.strip_prefix(&format!("CREATE TABLE {schema}.")) else { continue };
        let Some((name, body)) = rest.split_once(" (") else { continue };
        let body = body.strip_suffix(')').unwrap_or(body);
        let mut fields = BTreeMap::new();
        for entry in split_top(body) {
            if let Some((name, _)) = entry.split_once(' ') {
                fields.insert(name.to_string(), entry);
            }
        }
        out.insert(name.to_string(), fields);
    }
    out
}

#[derive(Default)]
struct Directives {
    backfill: BTreeMap<(String, String), Value>,
    convert: BTreeMap<(String, String), String>,
}

fn parse_directives(migration: &Option<Value>) -> Result<Directives, Value> {
    let Some(value) = migration else { return Ok(Directives::default()) };
    let obj = value.as_object().ok_or_else(|| err("BAD_MIGRATION", "migration은 객체여야 함"))?;
    if obj.keys().any(|k| k != "backfill" && k != "convert") {
        return Err(err("BAD_MIGRATION", "알 수 없는 migration 지시"));
    }
    let mut out = Directives::default();
    for (key, items) in obj {
        let items = items.as_array().ok_or_else(|| err("BAD_MIGRATION", "migration 지시는 배열이어야 함"))?;
        for item in items {
            let fields = item.as_object().ok_or_else(|| err("BAD_MIGRATION", "migration 항목은 객체여야 함"))?;
            let expected = if key == "backfill" { "value" } else { "to" };
            if fields.len() != 3 || fields.keys().any(|k| k != "resource" && k != "field" && k != expected) {
                return Err(err("BAD_MIGRATION", "migration 항목 필드 오류"));
            }
            let resource = item["resource"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| err("BAD_MIGRATION", "resource 이름 오류"))?;
            let field = item["field"].as_str().filter(|s| !s.is_empty()).ok_or_else(|| err("BAD_MIGRATION", "field 이름 오류"))?;
            let pair = (resource.to_string(), field.to_string());
            if key == "backfill" {
                if !(item["value"].is_string() || item["value"].is_number() || item["value"].is_boolean()) {
                    return Err(err("BAD_MIGRATION", "backfill 값은 정형 scalar여야 함"));
                }
                if out.backfill.insert(pair, item["value"].clone()).is_some() {
                    return Err(err("BAD_MIGRATION", "backfill 중복"));
                }
            } else {
                let to = item["to"].as_str().ok_or_else(|| err("BAD_MIGRATION", "convert 대상 타입 누락"))?;
                if out.convert.insert(pair, to.to_string()).is_some() {
                    return Err(err("BAD_MIGRATION", "convert 중복"));
                }
            }
        }
    }
    Ok(out)
}

fn typed_literal(facts: &Value, value: Value, ty: &str) -> Result<String, Value> {
    let base = ty.trim_end_matches('?');
    if decimal_type(base).is_some() {
        return spike_v2_read::scalar::parse(&json!({"enums":{}}), base, &value)
            .map(|(value, _)| value)
            .map_err(|_| err("BAD_MIGRATION", "backfill 값이 Decimal precision/scale과 맞지 않음"));
    }
    match base {
        "Text" | "Url" | "Time" => value.as_str().map(str::to_string),
        "Email" | "Date" => spike_v2_read::scalar::parse(&json!({"enums":{}}), base, &value).ok().map(|(value, _)| value),
        "Int" => value.as_i64().map(|n| n.to_string()),
        "Bool" => value.as_bool().map(|b| b.to_string()),
        t if t.starts_with("Enum<") && t.ends_with('>') => value.as_str().map(str::to_string),
        t if t.starts_with("Ref<") && t.ends_with('>') => {
            if spike_v2_read::id_wire::id_cast(facts, t) == "uuid" {
                value.as_str().and_then(|s| spike_v2_read::id_wire::parse_uuid(s).ok().map(|v| v.to_string()))
            } else {
                value.as_i64().map(|n| n.to_string())
            }
        }
        _ => None,
    }
    .ok_or_else(|| err("BAD_MIGRATION", "backfill 값의 JSON 타입이 필드와 다름"))
}

fn backfill_cast(facts: &Value, ty: &str) -> Option<String> {
    if let Some((precision, scale)) = decimal_type(ty.trim_end_matches('?')) {
        return Some(format!("numeric({precision},{scale})"));
    }
    match ty.trim_end_matches('?') {
        "Text" | "Email" | "Url" => Some("text".into()),
        "Int" => Some("bigint".into()),
        "Bool" => Some("boolean".into()),
        "Time" => Some("timestamptz".into()),
        "Date" => Some("date".into()),
        t if t.starts_with("Enum<") && t.ends_with('>') => Some("text".into()),
        t if (t.starts_with("Ref<") || t.starts_with("Id<")) && t.ends_with('>') => Some(spike_v2_read::id_wire::id_cast(facts, t).into()),
        _ => None,
    }
}

fn physical_type(facts: &Value, ty: &str) -> Option<String> {
    if let Some((precision, scale)) = decimal_type(ty.trim_end_matches('?')) {
        return Some(format!("numeric({precision},{scale})"));
    }
    match ty.trim_end_matches('?') {
        "Text" | "Email" | "Url" => Some("text".into()),
        "Int" => Some("bigint".into()),
        "Bool" => Some("boolean".into()),
        "Time" => Some("timestamptz".into()),
        "Date" => Some("date".into()),
        t if t.starts_with("Enum<") && t.ends_with('>') => Some("text".into()),
        t if (t.starts_with("Ref<") || t.starts_with("Id<")) && t.ends_with('>') => Some(spike_v2_read::id_wire::id_cast(facts, t).into()),
        _ => None,
    }
}

fn check_clause(def: &str) -> Option<String> {
    let at = def.find(" CHECK (")?;
    Some(def[at + 1..].strip_suffix(" NOT NULL").unwrap_or(&def[at + 1..]).to_string())
}

fn reference_clause(def: &str) -> Option<String> {
    let at = def.find(" REFERENCES ")?;
    Some(def[at + 1..].strip_suffix(" NOT NULL").unwrap_or(&def[at + 1..]).to_string())
}

fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

async fn old_column_constraint_names(db: &impl GenericClient, schema: &str, table: &str, col: &str, kind: char) -> Result<Vec<String>, Value> {
    let rows = db.query("SELECT c.conname FROM pg_constraint c JOIN pg_class t ON t.oid=c.conrelid JOIN pg_namespace n ON n.oid=t.relnamespace JOIN pg_attribute a ON a.attrelid=t.oid AND a.attnum=ANY(c.conkey) WHERE n.nspname=$1 AND t.relname=$2 AND a.attname=$3 AND c.contype::text=$4 ORDER BY c.conname", &[&schema,&table,&col,&kind.to_string()])
        .await.map_err(|_| err("DB_CATALOG", "열 제약 조회 실패"))?;
    Ok(rows.iter().map(|r| r.get(0)).collect())
}

fn extra_object(sql: &str, schema: &str) -> Option<(String, String)> {
    if sql.starts_with("CREATE UNIQUE INDEX ") || sql.starts_with("CREATE INDEX ") {
        let mut words = sql.split_whitespace();
        let _ = words.next();
        let second = words.next()?;
        let name = if second == "UNIQUE" {
            let _ = words.next();
            words.next()?
        } else {
            words.next()?
        };
        if !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            return None;
        }
        return Some((format!("index:{name}"), format!("DROP INDEX {schema}.{name}")));
    }
    if let Some(rest) = sql.strip_prefix(&format!("ALTER TABLE {schema}.")) {
        let (table, tail) = rest.split_once(" ADD CONSTRAINT ")?;
        let name = tail.split_whitespace().next()?;
        if !table.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            return None;
        }
        return Some((format!("constraint:{table}:{name}"), format!("ALTER TABLE {schema}.{table} DROP CONSTRAINT {name}")));
    }
    None
}

fn exposed_items(exposure: &Value, part: &str) -> BTreeSet<String> {
    if part == "select" {
        exposure[part].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default()
    } else {
        exposure[part].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
    }
}

fn classify_exposure(res: &str, old: &Value, new: &Value, changes: &mut Vec<Value>) {
    for part in ["select", "filter", "sort"] {
        let before = exposed_items(old, part);
        let after = exposed_items(new, part);
        for removed in before.difference(&after) {
            changes.push(json!({"class":"Breaking","reason":format!("{res} read {part} {removed} 제거")}));
        }
        for added in after.difference(&before) {
            changes.push(json!({"class":"SecurityReview","reason":format!("{res} read {part} {added} 추가")}));
        }
    }
    let mut before = old.clone();
    let mut after = new.clone();
    for part in ["select", "filter", "sort"] {
        if let Some(m) = before.as_object_mut() {
            m.remove(part);
        }
        if let Some(m) = after.as_object_mut() {
            m.remove(part);
        }
    }
    if before != after
        || old["select"]
            .as_object()
            .zip(new["select"].as_object())
            .is_some_and(|(a, b)| a.iter().any(|(k, v)| b.get(k).is_some_and(|next| next != v)))
    {
        changes.push(json!({"class":"SecurityReview","reason":format!("{res} read 관계/budget/표현 변경")}));
    }
}

async fn lock_managed_tables(db: &impl GenericClient, schema: &str) -> Result<(), Value> {
    let names = db.query("SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relkind IN ('r','p') ORDER BY c.relname", &[&schema])
        .await.map_err(|_| err("DB_LOCK", "관리 테이블 목록 조회 실패"))?;
    if names.is_empty() {
        return Err(err("SCHEMA_NOT_READY", "관리 테이블 없음"));
    }
    for row in names {
        let name: String = row.get(0);
        db.batch_execute(&format!("LOCK TABLE {schema}.{} IN ACCESS EXCLUSIVE MODE", quote_ident(&name)))
            .await
            .map_err(|_| err("DB_LOCK", "관리 테이블 잠금 실패"))?;
    }
    Ok(())
}

async fn classify_capacity_changes(
    db: &impl GenericClient,
    schema: &str,
    old_facts: &Value,
    new_facts: &Value,
    directives: &Directives,
    changes: &mut Vec<Value>,
    blocked: &mut bool,
) -> Result<(), Value> {
    for (res, new_resource) in new_facts["resources"].as_object().into_iter().flatten() {
        let Some(old_resource) = old_facts["resources"].get(res) else {
            // A resource added by this migration has no pre-existing rows to violate the new invariant.
            continue;
        };
        let old_invariants = old_facts["resources"][res]["invariants"].as_object();
        for (name, invariant) in new_resource["invariants"].as_object().into_iter().flatten() {
            let enforcement = &invariant["enforcement"];
            if enforcement["kind"] != "lockedCountCheck" || old_invariants.and_then(|invariants| invariants.get(name)) == Some(invariant) {
                continue;
            }
            changes.push(json!({"class":"SecurityReview","reason":format!("{res}.{name} 정원 제약 추가 또는 변경")}));
            let per = invariant["per"].as_str().ok_or_else(|| err("BAD_FACTS", "정원 그룹 필드 누락"))?;
            let col = sqlgen::column(new_facts, res, per).ok_or_else(|| err("BAD_FACTS", "정원 그룹 열 누락"))?;
            let max = enforcement["max"].as_i64().filter(|max| *max >= 1).ok_or_else(|| err("BAD_FACTS", "정원 상한 오류"))?;
            let mut cx = sqlgen::Ctx::new(new_facts, None, "");
            let env = sqlgen::Env { this: Some(("t".into(), res.clone())), ..Default::default() };
            let condition = cx.cond(&enforcement["where"], &env).map_err(|_| err("BAD_FACTS", "정원 조건 SQL 생성 실패"))?;
            let old_fields = old_resource["fields"].as_object().ok_or_else(|| err("BAD_FACTS", "기존 fields 객체 누락"))?;
            let new_fields = new_resource["fields"].as_object().ok_or_else(|| err("BAD_FACTS", "새 fields 객체 누락"))?;
            let mut projections = Vec::new();
            let mut params = cx.params.values.clone();
            for (field, descriptor) in new_fields {
                let new_col = sqlgen::column(new_facts, res, field).ok_or_else(|| err("BAD_FACTS", "새 열 이름 오류"))?;
                let new_type =
                    physical_type(new_facts, descriptor["ty"].as_str().unwrap_or("")).ok_or_else(|| err("BAD_FACTS", "정원 검사 물리 타입 오류"))?;
                let expression = if let Some(old_descriptor) = old_fields.get(field) {
                    let old_col = sqlgen::column(old_facts, res, field).ok_or_else(|| err("BAD_FACTS", "기존 열 이름 오류"))?;
                    let old_type = physical_type(old_facts, old_descriptor["ty"].as_str().unwrap_or(""))
                        .ok_or_else(|| err("BAD_FACTS", "기존 정원 검사 물리 타입 오류"))?;
                    if old_type == new_type { format!("src.{old_col}") } else { format!("src.{old_col}::text::{new_type}") }
                } else if let Some(value) = directives.backfill.get(&(res.clone(), field.clone())) {
                    let typed = typed_literal(new_facts, value.clone(), descriptor["ty"].as_str().unwrap_or(""))?;
                    params.push(Some(typed));
                    format!("${}::text::{new_type}", params.len())
                } else {
                    format!("NULL::{new_type}")
                };
                projections.push(format!("{expression} AS {new_col}"));
            }
            let table = format!("{schema}.{}", sqlgen::snake(res));
            let sql = format!(
                "SELECT EXISTS (SELECT 1 FROM (SELECT {} FROM {table} src) t WHERE t.{col} IS NOT NULL AND ({condition}) IS TRUE GROUP BY t.{col} HAVING count(*) > {max})",
                projections.join(", ")
            );
            let query_params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params.iter().map(|value| value as _).collect();
            let violated: bool = db.query_one(&sql, &query_params).await.map_err(|_| err("DB_CHECK", "기존 정원 데이터 검사 실패"))?.get(0);
            if violated {
                *blocked = true;
                changes.push(json!({"class":"Blocked","reason":format!("{res}.{name} 기존 데이터가 정원 제약을 초과함")}));
            }
        }
        for (name, old_invariant) in old_invariants.into_iter().flatten() {
            if old_invariant["enforcement"]["kind"] == "lockedCountCheck"
                && new_resource["invariants"].get(name) != Some(old_invariant)
                && new_resource["invariants"].get(name).is_none()
            {
                changes.push(json!({"class":"SecurityReview","reason":format!("{res}.{name} 정원 제약 제거로 정책 완화")}));
            }
        }
    }
    Ok(())
}

async fn derive(db: &impl GenericClient, schema: &str, new_facts: &Value, migration: &Option<Value>) -> Result<(Value, Vec<Step>), Value> {
    let mut directives = parse_directives(migration)?;
    let (old, legacy_trace_upgrade) = verified_marker_for_migration(db, schema).await?;
    if !old.facts["externalStorage"].is_null() {
        return Err(err("EXTERNAL_DDL_FORBIDDEN", "외부 배포에는 일반 DDL migration을 실행하지 않음"));
    }
    let (old_ddl, _) = creation(schema, &old.facts)?;
    let (new_ddl, new_ddl_digest) = creation(schema, new_facts)?;
    let old_tables = table_defs(&old_ddl, schema);
    let new_tables = table_defs(&new_ddl, schema);
    let mut steps = Vec::new();
    let mut changes = Vec::new();
    let mut blocked = false;
    if legacy_trace_upgrade {
        changes.push(json!({"class":"SecurityReview","reason":"기존 durable job 행을 유지하며 trace_id 컬럼을 nullable로 추가"}));
        steps.push(Step {
            sql: format!("ALTER TABLE {schema}.aip_jobs ADD COLUMN IF NOT EXISTS trace_id text"),
            class: "SecurityReview",
            value: None,
            reason: "검증된 이전 durable job schema에 trace_id 컬럼 추가".into(),
        });
    }
    let old_resources = old.facts["resources"].as_object().ok_or_else(|| err("BAD_FACTS", "기존 resources 오류"))?;
    let new_resources = new_facts["resources"].as_object().ok_or_else(|| err("BAD_FACTS", "새 resources 오류"))?;
    let old_uuid = old_resources.values().any(|r| r["idKind"] == "uuid");
    let new_uuid = new_resources.values().any(|r| r["idKind"] == "uuid");
    if !old_uuid && new_uuid {
        for col in ["recipient_id", "source_id"] {
            steps.push(Step {
                sql: format!("ALTER TABLE {schema}.aip_outbox ALTER COLUMN {col} TYPE text USING {col}::text"),
                class: "Breaking",
                value: None,
                reason: "UUID outbox identity는 손실 없는 text로 보존; 기존 worker 재시작 필요".into(),
            });
        }
        changes.push(json!({"class":"Breaking","reason":"UUID outbox storage 추가; owned migration 후 provision과 worker 재시작 필요"}));
    } else if old_uuid && !new_uuid {
        blocked = true;
        changes.push(json!({"class":"Unsupported","reason":"마지막 UUID resource 제거는 UUID outbox 이력 보존/정리 계획이 필요함"}));
    }
    let had_jobs = old.facts["operations"].as_object().is_some_and(|ops| ops.values().any(|op| op["kind"] == "job"));
    let has_jobs = new_facts["operations"].as_object().is_some_and(|ops| ops.values().any(|op| op["kind"] == "job"));
    if has_message_access(&old.facts) && !has_message_access(new_facts) {
        blocked = true;
        changes.push(
            json!({"class":"Unsupported","reason":"마지막 messages capability 제거는 큐 데이터 보존을 위해 차단됨; 명시적 보존/정리 계획이 필요함"}),
        );
    }
    if had_jobs && !has_jobs {
        blocked = true;
        changes
            .push(json!({"class":"Unsupported","reason":"마지막 durable job 제거는 저장소를 보존하기 위해 차단됨; 명시적 보존/정리 계획이 필요함"}));
    }
    if old.facts["actor"] != new_facts["actor"] || old.facts["actorMode"] != new_facts["actorMode"] {
        blocked = true;
        changes.push(json!({"class":"Unsupported","reason":"actor 변경은 principal FK 재매핑 계획이 필요함"}));
    }
    for (res, old_resource) in old.facts["resources"].as_object().unwrap() {
        if let Some(new_resource) = new_facts["resources"].get(res)
            && old_resource["idKind"] != new_resource["idKind"]
        {
            blocked = true;
            changes.push(json!({"class":"Unsupported","reason":"Id 종류 변경에는 별도 identity/FK 재매핑 계획이 필요함","resource":res}));
        }
    }
    let mut understood = old.facts.clone();
    understood["resources"] = new_facts["resources"].clone();
    if understood != *new_facts {
        changes.push(json!({"class":"SecurityReview","reason":"정책 또는 실행 facts 변경"}));
    }
    classify_capacity_changes(db, schema, &old.facts, new_facts, &directives, &mut changes, &mut blocked).await?;
    for (res, old_res) in old_resources {
        let name = sqlgen::snake(res);
        let Some(new_res) = new_resources.get(res) else {
            changes.push(json!({"class":"Destructive","reason":format!("resource {res} 제거")}));
            steps.push(Step {
                sql: format!("DROP TABLE {schema}.{name}"),
                class: "Destructive",
                value: None,
                reason: format!("resource {res} 제거"),
            });
            continue;
        };
        classify_exposure(res, &old_res["exposeRead"], &new_res["exposeRead"], &mut changes);
        let mut same_except_fields = old_res.clone();
        if let Some(m) = same_except_fields.as_object_mut() {
            m.remove("fields");
            m.remove("exposeRead");
        }
        let mut new_except_fields = new_res.clone();
        if let Some(m) = new_except_fields.as_object_mut() {
            m.remove("fields");
            m.remove("exposeRead");
        }
        if same_except_fields != new_except_fields {
            changes.push(json!({"class":"SecurityReview","reason":format!("{res} 정책/제약/쓰기 노출 변경")}));
        }
        let old_fields = old_res["fields"].as_object().ok_or_else(|| err("BAD_FACTS", "기존 fields 오류"))?;
        let new_fields = new_res["fields"].as_object().ok_or_else(|| err("BAD_FACTS", "새 fields 오류"))?;
        let old_cols = old_tables.get(&name).ok_or_else(|| err("BAD_FACTS", "기존 CREATE TABLE 누락"))?;
        let new_cols = new_tables.get(&name).ok_or_else(|| err("BAD_FACTS", "새 CREATE TABLE 누락"))?;
        for (field, fd) in old_fields {
            let col = sqlgen::column(&old.facts, res, field).ok_or_else(|| err("BAD_FACTS", "기존 열 이름 오류"))?;
            if !new_fields.contains_key(field) {
                changes.push(json!({"class":"Destructive","reason":format!("{res}.{field} 제거")}));
                steps.push(Step {
                    sql: format!("ALTER TABLE {schema}.{name} DROP COLUMN {col}"),
                    class: "Destructive",
                    value: None,
                    reason: format!("{res}.{field} 제거"),
                });
            } else {
                let new_fd = &new_fields[field];
                let new_col = sqlgen::column(new_facts, res, field).ok_or_else(|| err("BAD_FACTS", "새 열 이름 오류"))?;
                let old_def = old_cols.get(&col);
                let new_def = new_cols.get(&new_col);
                if fd != new_fd || old_def != new_def {
                    if old_def == new_def {
                        changes.push(json!({"class":"SecurityReview","reason":format!("{res}.{field} 의미 타입 변경")}));
                        if new_fd["ty"].as_str().is_some_and(|ty| ty.trim_end_matches('?') == "Email")
                            && fd["ty"].as_str().is_some_and(|ty| ty.trim_end_matches('?') != "Email")
                        {
                            let rows = db
                                .query(&format!("SELECT {col} FROM {schema}.{name} WHERE {col} IS NOT NULL"), &[])
                                .await
                                .map_err(|_| err("DB_CHECK", "Email 기존 값 검사 실패"))?;
                            let invalid = rows.iter().any(|row| {
                                let value: String = row.get(0);
                                spike_v2_read::scalar::parse(new_facts, "Email", &Value::String(value)).is_err()
                            });
                            if invalid {
                                blocked = true;
                                changes.push(json!({"class":"Blocked","reason":format!("{res}.{field} 기존 값이 Email 형식에 맞지 않음")}));
                            }
                        }
                    } else if let (Some(a), Some(b)) = (old_def, new_def) {
                        if a.replace(" NOT NULL", "") == b.replace(" NOT NULL", "") {
                            let adding = !a.contains(" NOT NULL") && b.contains(" NOT NULL");
                            if adding {
                                let n: i64 = db
                                    .query_one(&format!("SELECT count(*) FROM {schema}.{name} WHERE {col} IS NULL"), &[])
                                    .await
                                    .map_err(|_| err("DB_CHECK", "NULL 검사 실패"))?
                                    .get(0);
                                if n > 0 {
                                    blocked = true;
                                    changes.push(json!({"class":"Blocked","reason":format!("{res}.{field} NULL {n}행")}));
                                } else {
                                    changes.push(json!({"class":"Safe","reason":format!("{res}.{field} 필수화")}));
                                    steps.push(Step {
                                        sql: format!("ALTER TABLE {schema}.{name} ALTER COLUMN {col} SET NOT NULL"),
                                        class: "Safe",
                                        value: None,
                                        reason: format!("{res}.{field} 필수화"),
                                    });
                                }
                            } else {
                                changes.push(json!({"class":"Safe","reason":format!("{res}.{field} nullable 완화")}));
                                steps.push(Step {
                                    sql: format!("ALTER TABLE {schema}.{name} ALTER COLUMN {col} DROP NOT NULL"),
                                    class: "Safe",
                                    value: None,
                                    reason: format!("{res}.{field} nullable 완화"),
                                });
                            }
                        } else {
                            let old_ty = physical_type(&old.facts, fd["ty"].as_str().unwrap_or(""));
                            let new_ty = physical_type(new_facts, new_fd["ty"].as_str().unwrap_or(""));
                            if let (Some(old_ty), Some(new_ty)) = (old_ty, new_ty) {
                                let reason = format!("{res}.{field} 타입/범위/enum 변경");
                                changes.push(json!({"class":"Destructive","reason":reason}));
                                if check_clause(a).is_some() {
                                    for constraint in old_column_constraint_names(db, schema, &name, &col, 'c').await? {
                                        steps.push(Step {
                                            sql: format!("ALTER TABLE {schema}.{name} DROP CONSTRAINT {}", quote_ident(&constraint)),
                                            class: "Destructive",
                                            value: None,
                                            reason: reason.clone(),
                                        });
                                    }
                                }
                                if reference_clause(a).is_some() {
                                    for constraint in old_column_constraint_names(db, schema, &name, &col, 'f').await? {
                                        steps.push(Step {
                                            sql: format!("ALTER TABLE {schema}.{name} DROP CONSTRAINT {}", quote_ident(&constraint)),
                                            class: "Destructive",
                                            value: None,
                                            reason: reason.clone(),
                                        });
                                    }
                                }
                                if col != new_col {
                                    steps.push(Step {
                                        sql: format!("ALTER TABLE {schema}.{name} RENAME COLUMN {col} TO {new_col}"),
                                        class: "Destructive",
                                        value: None,
                                        reason: reason.clone(),
                                    });
                                }
                                if old_ty != new_ty {
                                    match directives.convert.remove(&(res.clone(), field.clone())) {
                                        Some(to) if to == new_fd["ty"].as_str().unwrap_or("") => {
                                            let target_ty = new_fd["ty"].as_str().unwrap_or("");
                                            let decimal_values_fit = if decimal_type(target_ty.trim_end_matches('?')).is_some() {
                                                let rows = db
                                                    .query(&format!("SELECT {new_col}::text FROM {schema}.{name} WHERE {new_col} IS NOT NULL"), &[])
                                                    .await
                                                    .map_err(|_| err("DB_CHECK", "Decimal 기존 값 검사 실패"))?;
                                                let invalid = rows.iter().any(|row| {
                                                    let value: String = row.get(0);
                                                    spike_v2_read::scalar::parse(new_facts, target_ty, &Value::String(value)).is_err()
                                                });
                                                if invalid {
                                                    blocked = true;
                                                    changes.push(json!({"class":"Blocked","reason":format!("{res}.{field} 기존 값이 {target_ty} precision/scale을 벗어남")}));
                                                }
                                                !invalid
                                            } else {
                                                true
                                            };
                                            if decimal_values_fit {
                                                steps.push(Step {
                                                    sql: format!(
                                                        "ALTER TABLE {schema}.{name} ALTER COLUMN {new_col} TYPE {new_ty} USING ({new_col}::text::{new_ty})"
                                                    ),
                                                    class: "Destructive",
                                                    value: None,
                                                    reason: reason.clone(),
                                                });
                                            }
                                        }
                                        _ => {
                                            blocked = true;
                                            changes.push(json!({"class":"Blocked","reason":format!("{res}.{field} 명시 convert 필요")}));
                                        }
                                    }
                                }
                                if let Some(reference) = reference_clause(b) {
                                    steps.push(Step {
                                        sql: format!("ALTER TABLE {schema}.{name} ADD FOREIGN KEY ({new_col}) {reference}"),
                                        class: "SecurityReview",
                                        value: None,
                                        reason: reason.clone(),
                                    });
                                }
                                if let Some(check) = check_clause(b) {
                                    steps.push(Step {
                                        sql: format!("ALTER TABLE {schema}.{name} ADD {check}"),
                                        class: "SecurityReview",
                                        value: None,
                                        reason: reason.clone(),
                                    });
                                }
                                if a.contains(" NOT NULL") != b.contains(" NOT NULL") {
                                    if b.contains(" NOT NULL") {
                                        let n: i64 = db
                                            .query_one(&format!("SELECT count(*) FROM {schema}.{name} WHERE {col} IS NULL"), &[])
                                            .await
                                            .map_err(|_| err("DB_CHECK", "NULL 검사 실패"))?
                                            .get(0);
                                        if n > 0 {
                                            blocked = true;
                                            changes.push(json!({"class":"Blocked","reason":format!("{res}.{field} NULL {n}행")}));
                                        } else {
                                            steps.push(Step {
                                                sql: format!("ALTER TABLE {schema}.{name} ALTER COLUMN {new_col} SET NOT NULL"),
                                                class: "Safe",
                                                value: None,
                                                reason: reason.clone(),
                                            });
                                        }
                                    } else {
                                        steps.push(Step {
                                            sql: format!("ALTER TABLE {schema}.{name} ALTER COLUMN {new_col} DROP NOT NULL"),
                                            class: "Safe",
                                            value: None,
                                            reason: reason.clone(),
                                        });
                                    }
                                }
                            } else {
                                blocked = true;
                                changes.push(json!({"class":"Unsupported","reason":format!("{res}.{field} 생성 타입 미지원")}));
                            }
                        }
                    } else {
                        blocked = true;
                        changes.push(json!({"class":"Unsupported","reason":format!("{res}.{field} 생성 SQL 불일치")}));
                    }
                }
            }
        }
        let has_rows: bool = if new_fields.keys().any(|f| !old_fields.contains_key(f)) {
            db.query_one(&format!("SELECT EXISTS(SELECT 1 FROM {schema}.{name} LIMIT 1)"), &[])
                .await
                .map_err(|_| err("DB_CHECK", "기존 행 검사 실패"))?
                .get(0)
        } else {
            false
        };
        for (field, fd) in new_fields {
            if old_fields.contains_key(field) {
                continue;
            }
            let col = sqlgen::column(new_facts, res, field).ok_or_else(|| err("BAD_FACTS", "새 열 이름 오류"))?;
            let def = new_cols.get(&col).ok_or_else(|| err("BAD_FACTS", "새 열 생성 SQL 누락"))?;
            let required = def.contains(" NOT NULL");
            let backfill = directives.backfill.remove(&(res.clone(), field.clone()));
            if required && has_rows && backfill.is_none() {
                blocked = true;
                changes.push(json!({"class":"Blocked","reason":format!("{res}.{field}: 기존 행에 backfill 필요")}));
                continue;
            }
            let class = if backfill.is_some() { "SecurityReview" } else { "Safe" };
            changes.push(json!({"class":class,"reason":format!("{res}.{field} 추가")}));
            if let Some(value) = backfill {
                // A constant backfill is a catalog-only change: ADD COLUMN ... DEFAULT <constant> stores the value
                // once (PostgreSQL 11+) and does not rewrite or update the rows, so the exclusive lock lasts
                // milliseconds on a table of any size. The old ADD / UPDATE / SET NOT NULL sequence rewrote every
                // row inside one transaction and could not finish within the 20s statement limit on large tables.
                let cast = backfill_cast(new_facts, fd["ty"].as_str().unwrap_or("")).ok_or_else(|| err("BAD_MIGRATION", "backfill 타입 미지원"))?;
                let value = typed_literal(new_facts, value, fd["ty"].as_str().unwrap_or(""))?;
                steps.push(Step {
                    sql: format!("ALTER TABLE {schema}.{name} ADD COLUMN {def} DEFAULT $1::text::{cast}"),
                    class,
                    value: Some(value),
                    reason: format!("{res}.{field} 추가와 backfill"),
                });
                steps.push(Step {
                    sql: format!("ALTER TABLE {schema}.{name} ALTER COLUMN {col} DROP DEFAULT"),
                    class,
                    value: None,
                    reason: format!("{res}.{field} backfill 기본값 제거"),
                });
                continue;
            }
            let nullable_def = if required { def.replace(" NOT NULL", "") } else { def.clone() };
            steps.push(Step {
                sql: format!("ALTER TABLE {schema}.{name} ADD COLUMN {nullable_def}"),
                class,
                value: None,
                reason: format!("{res}.{field} 추가"),
            });
            if required {
                steps.push(Step {
                    sql: format!("ALTER TABLE {schema}.{name} ALTER COLUMN {col} SET NOT NULL"),
                    class,
                    value: None,
                    reason: format!("{res}.{field} 필수화"),
                });
            }
        }
    }
    let mut new_table_steps = Vec::new();
    for ddl in &new_ddl {
        let Some(rest) = ddl.strip_prefix(&format!("CREATE TABLE {schema}.")) else { continue };
        let Some((name, _)) = rest.split_once(" (") else { continue };
        if let Some((res, _)) = new_resources.iter().find(|(r, _)| sqlgen::snake(r) == name) {
            if old_resources.contains_key(res) {
                continue;
            }
            changes.push(json!({"class":"SecurityReview","reason":format!("resource {res} 추가 및 새 노출 검토")}));
            new_table_steps.push(Step { sql: ddl.clone(), class: "SecurityReview", value: None, reason: format!("resource {res} 추가") });
        } else if has_jobs && EXECUTION_TABLES.contains(&name) && !old_tables.contains_key(name) {
            changes.push(json!({"class":"SecurityReview","reason":format!("durable execution table {name} 추가")}));
            new_table_steps.push(Step {
                sql: ddl.clone(),
                class: "SecurityReview",
                value: None,
                reason: format!("durable execution table {name} 추가"),
            });
        }
    }
    let old_extra: BTreeMap<_, _> = old_ddl
        .iter()
        .filter(|s| !s.starts_with("CREATE SCHEMA ") && !s.starts_with("CREATE TABLE "))
        .filter_map(|s| extra_object(s, schema).map(|(k, d)| (k, (s.clone(), d))))
        .collect();
    let new_extra: BTreeMap<_, _> = new_ddl
        .iter()
        .filter(|s| !s.starts_with("CREATE SCHEMA ") && !s.starts_with("CREATE TABLE "))
        .filter_map(|s| extra_object(s, schema).map(|(k, d)| (k, (s.clone(), d))))
        .collect();
    let old_extra_count = old_ddl.iter().filter(|s| !s.starts_with("CREATE SCHEMA ") && !s.starts_with("CREATE TABLE ")).count();
    let new_extra_count = new_ddl.iter().filter(|s| !s.starts_with("CREATE SCHEMA ") && !s.starts_with("CREATE TABLE ")).count();
    if old_extra.len() != old_extra_count || new_extra.len() != new_extra_count {
        blocked = true;
        changes.push(json!({"class":"Unsupported","reason":"알 수 없는 생성 DDL"}));
    }
    let mut early = Vec::new();
    for (key, (old_sql, drop_sql)) in &old_extra {
        if new_extra.get(key).is_none_or(|(new_sql, _)| new_sql != old_sql) {
            changes.push(json!({"class":"SecurityReview","reason":format!("{key} 제거 또는 변경")}));
            early.push(Step { sql: drop_sql.clone(), class: "SecurityReview", value: None, reason: format!("{key} 제거 또는 변경") });
        }
    }
    early.extend(new_table_steps);
    early.extend(steps.iter().filter(|s| !s.sql.starts_with("DROP TABLE ")).cloned());
    let mut table_drops: Vec<_> = steps.iter().filter(|s| s.sql.starts_with("DROP TABLE ")).cloned().collect();
    table_drops.sort_by_key(|s| old_ddl.iter().position(|ddl| ddl.starts_with(&s.sql.replace("DROP TABLE ", "CREATE TABLE "))).unwrap_or(0));
    table_drops.reverse();
    early.extend(table_drops);
    steps = early;
    for (key, (new_sql, _)) in &new_extra {
        if old_extra.get(key).is_none_or(|(old_sql, _)| old_sql != new_sql) {
            changes.push(json!({"class":"SecurityReview","reason":format!("{key} 생성 또는 변경")}));
            steps.push(Step { sql: new_sql.clone(), class: "SecurityReview", value: None, reason: format!("{key} 생성 또는 변경") });
        }
    }
    if !directives.backfill.is_empty() || !directives.convert.is_empty() {
        return Err(err("BAD_MIGRATION", "사용되지 않은 backfill/convert 지시"));
    }
    let need_review = changes.iter().any(|c| matches!(c["class"].as_str(), Some("Breaking" | "SecurityReview" | "Destructive")));
    let payload = json!({"version":VERSION,"schema":schema,"oldDigest":old.facts_digest,"oldStructureDigest":old.structure_digest,"newDigest":digest(new_facts),"newDdlDigest":new_ddl_digest,"migration":migration,"steps":steps.iter().map(Step::json).collect::<Vec<_>>(),"changes":changes});
    let plan_digest = digest(&payload);
    let report = json!({"ok":true,"digest":plan_digest,"blocked":blocked,"requiresReview":need_review,"steps":payload["steps"],"changes":payload["changes"],"oldDigest":payload["oldDigest"],"newDigest":payload["newDigest"]});
    Ok((report, steps))
}

pub async fn plan(db_url: &str, schema: &str, new_facts: &Value, migration: &Option<Value>) -> Result<Value, Value> {
    tokio::time::timeout(std::time::Duration::from_secs(12), plan_inner(db_url, schema, new_facts, migration))
        .await
        .map_err(|_| err("DB_PLAN_TIMEOUT", "계획 조회 기한 초과"))?
}

async fn plan_inner(db_url: &str, schema: &str, new_facts: &Value, migration: &Option<Value>) -> Result<Value, Value> {
    configure_schema(schema)?;
    let mut db = connect(db_url).await?;
    let tx = db.transaction().await.map_err(|_| err("DB_PLAN", "계획 snapshot 시작 실패"))?;
    tx.batch_execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY; SET LOCAL statement_timeout='10s'; SET LOCAL TimeZone TO 'UTC'")
        .await
        .map_err(|_| err("DB_PLAN", "계획 snapshot 설정 실패"))?;
    let (report, _) = derive(&tx, schema, new_facts, migration).await?;
    tx.commit().await.map_err(|_| err("DB_PLAN", "계획 snapshot 종료 실패"))?;
    Ok(report)
}

pub async fn apply(db_url: &str, schema: &str, new_facts: &Value, migration: &Option<Value>, acknowledgement: &str) -> Result<Value, Value> {
    tokio::time::timeout(std::time::Duration::from_secs(25), apply_inner(db_url, schema, new_facts, migration, acknowledgement))
        .await
        .map_err(|_| err("MIGRATION_UNSETTLED", "적용 기한 초과; journal과 catalog로 커밋 여부 확인 필요"))?
}

async fn apply_inner(db_url: &str, schema: &str, new_facts: &Value, migration: &Option<Value>, acknowledgement: &str) -> Result<Value, Value> {
    configure_schema(schema)?;
    parse_directives(migration)?;
    let mut db = connect(db_url).await?;
    let tx = db.transaction().await.map_err(|_| err("DB_APPLY", "적용 트랜잭션 시작 실패"))?;
    // UTC makes Time <-> Date conversions (they go through text) mean the same thing in every session that runs a migration.
    tx.batch_execute("SET LOCAL lock_timeout='2s'; SET LOCAL statement_timeout='20s'; SET LOCAL TimeZone TO 'UTC'")
        .await
        .map_err(|_| err("DB_APPLY", "적용 기한 설정 실패"))?;
    tx.execute("SELECT pg_advisory_xact_lock(1095323725, hashtext($1))", &[&schema]).await.map_err(|_| err("DB_LOCK", "migration 잠금 실패"))?;
    if !marker(&tx, schema).await?.facts["externalStorage"].is_null() {
        return Err(err("EXTERNAL_DDL_FORBIDDEN", "외부 배포에는 DDL을 실행하지 않음"));
    }
    lock_managed_tables(&tx, schema).await?;
    let latest = tx
        .query_opt(&format!("SELECT plan_digest,new_digest FROM {schema}.{JOURNAL} ORDER BY id DESC LIMIT 1"), &[])
        .await
        .map_err(|_| err("SCHEMA_NOT_READY", "migration journal 조회 실패"))?;
    if let Some(row) = latest {
        let prior_plan: String = row.get(0);
        let prior_facts: String = row.get(1);
        if prior_plan == acknowledgement {
            let m = verified_marker(&tx, schema).await?;
            if prior_facts == digest(new_facts) && m.facts_digest == prior_facts {
                tx.commit().await.map_err(|_| err("COMMIT_UNSETTLED", "재조회 종료 결과 불명"))?;
                return Ok(json!({"ok":true,"schema":schema,"alreadyApplied":true,"digest":acknowledgement,"factsDigest":prior_facts}));
            }
            return Err(err("PLAN_CHANGED", "같은 계획 식별자가 다른 정의를 가리킴"));
        }
    }
    let (report, steps) = derive(&tx, schema, new_facts, migration).await?;
    if report["blocked"] == true {
        return Err(err("MIGRATION_BLOCKED", "지원하지 않거나 기존 데이터와 충돌하는 변경"));
    }
    if report["digest"].as_str() != Some(acknowledgement) {
        return Err(err("PLAN_CHANGED", "검토한 계획과 현재 DB/정의가 다름"));
    }
    if steps.is_empty() && report["oldDigest"] == report["newDigest"] {
        tx.commit().await.map_err(|_| err("COMMIT_UNSETTLED", "무변경 확인 종료 결과 불명"))?;
        return Ok(json!({"ok":true,"schema":schema,"unchanged":true,"digest":acknowledgement,"steps":[]}));
    }
    // After the drift check above, which compares the catalog with the recorded structure, and before the new structure is recorded.
    // Deployments created before this change declared plan_digest UNIQUE, which made A -> B -> A -> B fail on the second
    // A -> B (same plan, same digest). The journal is ordered by id; the digest only has to identify the plan that ran.
    tx.batch_execute(&format!("ALTER TABLE {schema}.{JOURNAL} DROP CONSTRAINT IF EXISTS {JOURNAL}_plan_digest_key"))
        .await
        .map_err(|_| err("DB_APPLY", "journal 제약 정리 실패"))?;
    for step in &steps {
        if let Some(value) = &step.value {
            if step.sql.contains(" DEFAULT $1::text::") {
                // DDL cannot take bind parameters, so the server quotes the value as a literal.
                let literal: String = tx
                    .query_one("SELECT quote_literal($1::text)", &[value])
                    .await
                    .map_err(|_| err("MIGRATION_FAILED", "backfill 값 인용 실패; 트랜잭션 취소"))?
                    .get(0);
                tx.batch_execute(&step.sql.replace("$1::text::", &format!("{literal}::text::")))
                    .await
                    .map_err(|_| err("MIGRATION_FAILED", "backfill 실패; 트랜잭션 취소"))?;
            } else {
                tx.execute(&step.sql, &[value]).await.map_err(|_| err("MIGRATION_FAILED", "backfill 실패; 트랜잭션 취소"))?;
            }
        } else {
            tx.batch_execute(&step.sql).await.map_err(|_| err("MIGRATION_FAILED", "DDL 실패; 트랜잭션 취소"))?;
        }
    }
    verify_target(&tx, schema, new_facts).await?;
    let structure_digest = structure(&tx, schema).await?;
    let (_, ddl_digest) = creation(schema, new_facts)?;
    let new_digest = digest(new_facts);
    tx.execute(
        &format!("UPDATE {schema}.{META} SET facts=$1,facts_digest=$2,ddl_digest=$3,structure_digest=$4,generator_version=$5 WHERE singleton=true"),
        &[new_facts, &new_digest, &ddl_digest, &structure_digest, &VERSION],
    )
    .await
    .map_err(|_| err("MIGRATION_FAILED", "metadata 갱신 실패"))?;
    let old_digest = report["oldDigest"].as_str().ok_or_else(|| err("MIGRATION_FAILED", "기존 digest 누락"))?;
    let step_json = report["steps"].clone();
    tx.execute(
        &format!("INSERT INTO {schema}.{JOURNAL} (plan_digest,old_digest,new_digest,steps,approval) VALUES ($1,$2,$3,$4,$5)"),
        &[&acknowledgement, &old_digest, &new_digest, &step_json, &acknowledgement],
    )
    .await
    .map_err(|_| err("MIGRATION_FAILED", "journal 기록 실패"))?;
    tx.commit().await.map_err(|_| err("COMMIT_UNSETTLED", "커밋 결과 불명; journal과 catalog 조회 필요"))?;
    Ok(
        json!({"ok":true,"schema":schema,"digest":acknowledgement,"factsDigest":new_digest,"structureDigest":structure_digest,"steps":report["steps"]}),
    )
}

#[cfg(test)]
mod old_journal_tests {
    use super::*;

    /// A deployment created before the journal stopped requiring unique plan digests still has that constraint. It must be
    /// dropped by the next apply, together with a new structure digest, or A -> B -> A -> B keeps failing there.
    #[tokio::test]
    async fn a_deployment_with_the_old_unique_journal_digest_can_cycle() {
        let url = aip_test_support::test_database_url!();
        let schema = format!("aip_oldjournal_{}", std::process::id());
        let a = spike_v1_fixture::load_str(include_str!("../../../prototype/example/app.aip"), spike_v1_fixture::Form::A).unwrap().execution;
        let mut b = a.clone();
        b["resources"]["Member"]["fields"]["nickname"] = json!({"ty":"Text?"});
        init(url, &schema, &a).await.unwrap();
        let db = connect(url).await.unwrap();
        db.batch_execute(&format!("ALTER TABLE {schema}.{JOURNAL} ADD CONSTRAINT {JOURNAL}_plan_digest_key UNIQUE (plan_digest)")).await.unwrap();
        let old_structure = structure(&*db, &schema).await.unwrap();
        db.execute(&format!("UPDATE {schema}.{META} SET structure_digest=$1"), &[&old_structure]).await.unwrap();

        let mut outcome = Ok(());
        for target in [&b, &a, &b] {
            let step = async {
                let report = plan(url, &schema, target, &None).await?;
                apply(url, &schema, target, &None, report["digest"].as_str().unwrap()).await.map(|_| ())
            }
            .await;
            if step.is_err() {
                outcome = step;
                break;
            }
        }
        db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await.unwrap();
        assert!(outcome.is_ok(), "{outcome:?}");
    }
}

#[cfg(test)]
mod trace_id_upgrade_tests {
    use super::*;
    use std::{
        fs,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn database_url() -> String {
        aip_test_support::database_url().to_owned()
    }

    fn job_facts() -> Value {
        let source = include_str!("../../../spikes/spike-v6-transport/tests/fixtures/operations.aip").replace("operation read", "operation job");
        spike_v1_fixture::load_str(&source, spike_v1_fixture::Form::A).unwrap().execution
    }

    fn service_command(cli: &str, args: &[String]) -> Value {
        let output = Command::new(cli).args(args).output().unwrap();
        assert!(output.status.success(), "aip service command failed: {}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&output.stdout).unwrap()
    }

    async fn make_parent_job_schema(db_url: &str, schema: &str, facts: &Value) {
        init(db_url, schema, facts).await.unwrap();
        let db = connect(db_url).await.unwrap();
        db.batch_execute(&format!("ALTER TABLE {schema}.aip_jobs DROP COLUMN trace_id")).await.unwrap();
        let (mut parent_ddl, _) = creation(schema, facts).unwrap();
        let jobs = parent_ddl.iter_mut().find(|ddl| ddl.starts_with(&format!("CREATE TABLE {schema}.aip_jobs "))).unwrap();
        *jobs = jobs.replace("trace_id text,", "");
        let parent_digest = digest(&json!(parent_ddl));
        let structure_digest = structure(&*db, schema).await.unwrap();
        db.execute(&format!("UPDATE {schema}.{META} SET ddl_digest=$1,structure_digest=$2"), &[&parent_digest, &structure_digest]).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires a Run-owned isolated PostgreSQL database and a built aip service CLI"]
    async fn explicit_parent_job_schema_upgrade_adds_trace_id_and_rejects_drift() {
        let db_url = database_url();
        let suffix = NEXT.fetch_add(1, Ordering::Relaxed);
        let schema = format!("aip_trace_up_{}_{}", std::process::id(), suffix);
        let drift_schema = format!("aip_trace_drift_{}_{}", std::process::id(), suffix);
        let unknown_schema = format!("aip_trace_unknown_{}_{}", std::process::id(), suffix);
        let inconsistent_schema = format!("aip_trace_inconsistent_{}_{}", std::process::id(), suffix);
        let temp = std::env::temp_dir().join(format!("aip-trace-upgrade-cli-{}-{suffix}", std::process::id()));
        fs::create_dir_all(&temp).unwrap();
        let app = temp.join("jobs.aip");
        let config = temp.join("service.json");
        let app_source = include_str!("../../../spikes/spike-v6-transport/tests/fixtures/operations.aip").replace("operation read", "operation job");
        fs::write(&app, app_source).unwrap();
        fs::write(
            &config,
            serde_json::to_vec(&json!({
                "schema":schema.clone(),
                "database_url_env":"AIP_TRACE_UPGRADE_DATABASE_URL",
                "auth":{"issuer":"https://issuer.example","audience":"aip","jwks":{"kind":"file","path":"jwks.json"}}
            }))
            .unwrap(),
        )
        .unwrap();
        let cli = std::env::var("AIP_SERVICE_CLI").expect("AIP_SERVICE_CLI must point to the built aip CLI");
        let init =
            service_command(&cli, &["service".into(), "init".into(), app.display().to_string(), "--config".into(), config.display().to_string()]);
        assert_eq!(init["ok"], true, "{init}");
        let facts = job_facts();
        // Recreate the exact pre-trace column shape and its original DDL/catalog marker as test fixture setup.
        let db = connect(&db_url).await.unwrap();
        db.batch_execute(&format!("ALTER TABLE {schema}.aip_jobs DROP COLUMN trace_id")).await.unwrap();
        let (mut parent_ddl, _) = creation(&schema, &facts).unwrap();
        let jobs = parent_ddl.iter_mut().find(|ddl| ddl.starts_with(&format!("CREATE TABLE {schema}.aip_jobs "))).unwrap();
        *jobs = jobs.replace("trace_id text,", "");
        let parent_digest = digest(&json!(parent_ddl));
        let parent_structure = structure(&*db, &schema).await.unwrap();
        db.execute(&format!("UPDATE {schema}.{META} SET ddl_digest=$1,structure_digest=$2"), &[&parent_digest, &parent_structure]).await.unwrap();
        drop(db);
        make_parent_job_schema(&db_url, &drift_schema, &facts).await;
        make_parent_job_schema(&db_url, &unknown_schema, &facts).await;
        make_parent_job_schema(&db_url, &inconsistent_schema, &facts).await;

        let db = connect(&db_url).await.unwrap();
        db.batch_execute(&format!("ALTER TABLE {drift_schema}.aip_jobs ADD COLUMN unrecognized_drift text")).await.unwrap();
        assert_eq!(plan(&db_url, &drift_schema, &facts, &None).await.unwrap_err()["code"], "WIRE.SCHEMA_MISMATCH");
        db.execute(format!("UPDATE {unknown_schema}.{META} SET ddl_digest='unknown-parent-digest'").as_str(), &[]).await.unwrap();
        assert_eq!(plan(&db_url, &unknown_schema, &facts, &None).await.unwrap_err()["code"], "WIRE.SCHEMA_MISMATCH");
        db.batch_execute(&format!("ALTER TABLE {inconsistent_schema}.aip_jobs ADD COLUMN trace_id text")).await.unwrap();
        let inconsistent_structure = structure(&*db, &inconsistent_schema).await.unwrap();
        db.execute(&format!("UPDATE {inconsistent_schema}.{META} SET structure_digest=$1"), &[&inconsistent_structure]).await.unwrap();
        assert_eq!(plan(&db_url, &inconsistent_schema, &facts, &None).await.unwrap_err()["code"], "WIRE.SCHEMA_MISMATCH");

        db.execute(
            &format!("INSERT INTO {schema}.aip_jobs(id,owner,actor_id,grant_data,operation,contract_digest,deployment_digest,implementation_digest,input,input_digest,idempotency_key,state,max_attempts,timeout_ms,run_deadline,result_ttl_ms,initial_delay_ms) VALUES('00000000-0000-4000-8000-000000000001','https://issuer.example|subject-1',17,'{{}}','normalize','contract-a','deployment-a','sha256:impl-a','{{\"x\":1}}','legacy-input','legacy-key','QUEUED',3,30000,clock_timestamp()+interval '1 hour',60000,0)"),
            &[],
        )
        .await
        .unwrap();
        db.execute(&format!("INSERT INTO {schema}.aip_actors(id) VALUES(17)"), &[]).await.unwrap();
        db.execute(
            &format!("INSERT INTO {schema}.aip_principals(issuer,subject,actor_id,min_iat) VALUES('https://issuer.example','subject-1',17,123)"),
            &[],
        )
        .await
        .unwrap();
        db.execute(
            &format!(
                "INSERT INTO {schema}.aip_idem(principal,key,request,result) VALUES('actor:17:ids:decimal-string-v13','idem-key','idem-request',$1)"
            ),
            &[&json!({"kept":true})],
        )
        .await
        .unwrap();

        let plan_output =
            service_command(&cli, &["service".into(), "migrate".into(), app.display().to_string(), "--config".into(), config.display().to_string()]);
        let report = plan_output;
        assert_eq!(report["blocked"], false, "{report}");
        assert!(
            report["steps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|step| step["sql"].as_str().unwrap_or("").contains("aip_jobs") && step["sql"].as_str().unwrap_or("").contains("trace_id")),
            "{report}"
        );
        let result = service_command(
            &cli,
            &[
                "service".into(),
                "migrate".into(),
                app.display().to_string(),
                "--config".into(),
                config.display().to_string(),
                "--apply".into(),
                report["digest"].as_str().unwrap().into(),
            ],
        );
        assert_eq!(result["ok"], true, "{result}");
        let repeated = service_command(
            &cli,
            &[
                "service".into(),
                "migrate".into(),
                app.display().to_string(),
                "--config".into(),
                config.display().to_string(),
                "--apply".into(),
                report["digest"].as_str().unwrap().into(),
            ],
        );
        assert_eq!(repeated["alreadyApplied"], true, "{repeated}");
        assert_eq!(preflight(&db_url, &schema, &facts).await.unwrap()["ok"], true);
        assert_eq!(plan(&db_url, &schema, &facts, &None).await.unwrap()["steps"], json!([]));

        let store = aip_execution::Store::new(schema.clone()).unwrap();
        let legacy_lease = store.claim(&*db, "upgrade-worker").await.unwrap().unwrap();
        assert_eq!(legacy_lease.trace_id, None);
        assert!(store.retry(&*db, &legacy_lease, "TEMPORARY_TEST").await.unwrap());
        let legacy_trace: Option<String> =
            db.query_one(&format!("SELECT trace_id FROM {schema}.aip_jobs WHERE id=$1"), &[&legacy_lease.run_id]).await.unwrap().get(0);
        assert_eq!(legacy_trace, None);

        let mut run = aip_execution::NewRun {
            owner: "https://issuer.example|subject-1".into(),
            actor: 17,
            grant: json!({"scope":"operation:normalize"}),
            operation: "normalize".into(),
            contract: "contract-a".into(),
            deployment: "deployment-a".into(),
            implementation: "sha256:impl-a".into(),
            input: json!({"x":2}),
            key: "new-trace-key".into(),
            timeout_ms: 30_000,
            max_attempts: 3,
            ttl_ms: 60_000,
            delay_ms: 0,
            trace_id: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
        };
        let enqueued = store.enqueue(&*db, run.clone()).await.unwrap();
        run.trace_id = Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into());
        let replay = store.enqueue(&*db, run).await.unwrap();
        assert_eq!(enqueued.id, replay.id);
        let traced_lease = store.claim(&*db, "upgrade-worker").await.unwrap().unwrap();
        assert_eq!(traced_lease.trace_id.as_deref(), Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        assert!(store.retry(&*db, &traced_lease, "TEMPORARY_TEST").await.unwrap());
        let stored_trace: Option<String> =
            db.query_one(&format!("SELECT trace_id FROM {schema}.aip_jobs WHERE id=$1"), &[&traced_lease.run_id]).await.unwrap().get(0);
        assert_eq!(stored_trace.as_deref(), Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        let actor: i64 = db.query_one(&format!("SELECT id FROM {schema}.aip_actors WHERE id=17"), &[]).await.unwrap().get(0);
        assert_eq!(actor, 17);
        let principal = db.query_one(&format!("SELECT issuer,subject,actor_id,min_iat FROM {schema}.aip_principals"), &[]).await.unwrap();
        assert_eq!(principal.get::<_, String>(0), "https://issuer.example");
        assert_eq!(principal.get::<_, String>(1), "subject-1");
        assert_eq!(principal.get::<_, i64>(2), 17);
        assert_eq!(principal.get::<_, i64>(3), 123);
        let idem: Value = db.query_one(&format!("SELECT result FROM {schema}.aip_idem WHERE key='idem-key'"), &[]).await.unwrap().get(0);
        assert_eq!(idem, json!({"kept":true}));
        let metadata = db.query_one(&format!("SELECT facts,runtime_wire FROM {schema}.{META} WHERE singleton=true"), &[]).await.unwrap();
        assert_eq!(metadata.get::<_, Value>(0), facts);
        assert_eq!(metadata.get::<_, Option<String>>(1).as_deref(), Some("decimal"));

        db.batch_execute(&format!("DROP SCHEMA {drift_schema} CASCADE; DROP SCHEMA {unknown_schema} CASCADE; DROP SCHEMA {inconsistent_schema} CASCADE; DROP SCHEMA {schema} CASCADE"))
            .await
            .unwrap();
        fs::remove_dir_all(temp).unwrap();
    }
}
