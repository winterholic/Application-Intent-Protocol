//! PostgreSQL deployment of the V1 execution facts. All database changes are explicit CLI operations.
pub mod adopt;
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
    let ddl = sqlgen::create_ddl_in(schema, facts).map_err(|_| err("UNSUPPORTED_SCHEMA", "정의의 SQL 구조를 생성할 수 없음"))?;
    let hash = digest(&json!(ddl));
    Ok((ddl, hash))
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
    if !facts["resources"].get(actor).is_some_and(Value::is_object) {
        return Err(err("UNSUPPORTED_SCHEMA", "actor resource 누락"));
    }
    let actor_table = sqlgen::snake(actor);
    Ok(format!(
        "CREATE TABLE {schema}.{META} (singleton boolean PRIMARY KEY CHECK(singleton), facts jsonb NOT NULL, facts_digest text NOT NULL, ddl_digest text NOT NULL, structure_digest text NOT NULL, generator_version integer NOT NULL, runtime_wire text CHECK(runtime_wire IN ('safe','decimal'))); CREATE TABLE {schema}.{JOURNAL} (id bigint GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, plan_digest text NOT NULL UNIQUE, old_digest text, new_digest text NOT NULL, steps jsonb NOT NULL, approval text, applied_at timestamptz NOT NULL DEFAULT now()); CREATE TABLE {schema}.aip_principals (issuer text NOT NULL, subject text NOT NULL, actor_id bigint NOT NULL REFERENCES {schema}.{actor_table}(id), enabled boolean NOT NULL DEFAULT true, min_iat bigint NOT NULL DEFAULT 0, PRIMARY KEY (issuer,subject)); CREATE FUNCTION {schema}.aip_reject_principal_actor_change() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.actor_id IS DISTINCT FROM OLD.actor_id THEN RAISE EXCEPTION 'principal actor_id is immutable' USING ERRCODE='23514'; END IF; RETURN NEW; END $$; CREATE TRIGGER aip_principal_actor_immutable BEFORE UPDATE OF actor_id ON {schema}.aip_principals FOR EACH ROW EXECUTE FUNCTION {schema}.aip_reject_principal_actor_change(); CREATE TABLE {schema}.aip_idem (principal text NOT NULL, key text NOT NULL, request text NOT NULL, result jsonb NOT NULL, PRIMARY KEY (principal,key))"
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
        return Err(err("SCHEMA_MISMATCH", "metadata와 journal의 마지막 정의가 다름"));
    }
    Ok(m)
}

async fn verified_marker(db: &impl GenericClient, schema: &str) -> Result<Marker, Value> {
    let m = marker(db, schema).await?;
    if m.generator_version != VERSION || m.facts_digest != digest(&m.facts) || m.ddl_digest != creation(schema, &m.facts)?.1 {
        return Err(err("SCHEMA_MISMATCH", "metadata의 facts 또는 생성기 버전 불일치"));
    }
    if m.structure_digest != structure(db, schema).await? {
        return Err(err("SCHEMA_MISMATCH", "실제 DB 구조가 journal 구조와 다름"));
    }
    Ok(m)
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
    let (_, ddl_digest) = creation(schema, facts)?;
    if m.facts_digest != digest(facts) || m.ddl_digest != ddl_digest {
        return Err(err("SCHEMA_MISMATCH", "정의와 배포된 facts/DDL이 다름"));
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
    lock_managed_tables(&tx, schema).await?;
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

fn typed_literal(value: Value, ty: &str) -> Result<String, Value> {
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
        t if t.starts_with("Ref<") && t.ends_with('>') => value.as_i64().map(|n| n.to_string()),
        _ => None,
    }
    .ok_or_else(|| err("BAD_MIGRATION", "backfill 값의 JSON 타입이 필드와 다름"))
}

fn backfill_cast(ty: &str) -> Option<String> {
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
        t if (t.starts_with("Ref<") || t.starts_with("Id<")) && t.ends_with('>') => Some("bigint".into()),
        _ => None,
    }
}

fn physical_type(ty: &str) -> Option<String> {
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
        t if (t.starts_with("Ref<") || t.starts_with("Id<")) && t.ends_with('>') => Some("bigint".into()),
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
                let new_type = physical_type(descriptor["ty"].as_str().unwrap_or("")).ok_or_else(|| err("BAD_FACTS", "정원 검사 물리 타입 오류"))?;
                let expression = if let Some(old_descriptor) = old_fields.get(field) {
                    let old_col = sqlgen::column(old_facts, res, field).ok_or_else(|| err("BAD_FACTS", "기존 열 이름 오류"))?;
                    let old_type = physical_type(old_descriptor["ty"].as_str().unwrap_or(""))
                        .ok_or_else(|| err("BAD_FACTS", "기존 정원 검사 물리 타입 오류"))?;
                    if old_type == new_type { format!("src.{old_col}") } else { format!("src.{old_col}::text::{new_type}") }
                } else if let Some(value) = directives.backfill.get(&(res.clone(), field.clone())) {
                    let typed = typed_literal(value.clone(), descriptor["ty"].as_str().unwrap_or(""))?;
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
    let old = verified_marker(db, schema).await?;
    let (old_ddl, _) = creation(schema, &old.facts)?;
    let (new_ddl, new_ddl_digest) = creation(schema, new_facts)?;
    let old_tables = table_defs(&old_ddl, schema);
    let new_tables = table_defs(&new_ddl, schema);
    let mut steps = Vec::new();
    let mut changes = Vec::new();
    let mut blocked = false;
    let old_resources = old.facts["resources"].as_object().ok_or_else(|| err("BAD_FACTS", "기존 resources 오류"))?;
    let new_resources = new_facts["resources"].as_object().ok_or_else(|| err("BAD_FACTS", "새 resources 오류"))?;
    if old.facts["actor"] != new_facts["actor"] {
        blocked = true;
        changes.push(json!({"class":"Unsupported","reason":"actor 변경은 principal FK 재매핑 계획이 필요함"}));
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
                            let old_ty = physical_type(fd["ty"].as_str().unwrap_or(""));
                            let new_ty = physical_type(new_fd["ty"].as_str().unwrap_or(""));
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
            let nullable_def = if required { def.replace(" NOT NULL", "") } else { def.clone() };
            steps.push(Step {
                sql: format!("ALTER TABLE {schema}.{name} ADD COLUMN {nullable_def}"),
                class,
                value: None,
                reason: format!("{res}.{field} 추가"),
            });
            if let Some(value) = backfill {
                let cast = backfill_cast(fd["ty"].as_str().unwrap_or("")).ok_or_else(|| err("BAD_MIGRATION", "backfill 타입 미지원"))?;
                let value = typed_literal(value, fd["ty"].as_str().unwrap_or(""))?;
                steps.push(Step {
                    sql: format!("UPDATE {schema}.{name} SET {col}=$1::text::{cast} WHERE {col} IS NULL"),
                    class: "SecurityReview",
                    value: Some(value),
                    reason: format!("{res}.{field} backfill"),
                });
            }
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
        let Some((res, _)) = new_resources.iter().find(|(r, _)| sqlgen::snake(r) == name) else { continue };
        if old_resources.contains_key(res) {
            continue;
        }
        changes.push(json!({"class":"SecurityReview","reason":format!("resource {res} 추가 및 새 노출 검토")}));
        new_table_steps.push(Step { sql: ddl.clone(), class: "SecurityReview", value: None, reason: format!("resource {res} 추가") });
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
    tx.batch_execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY; SET LOCAL statement_timeout='10s'")
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
    tx.batch_execute("SET LOCAL lock_timeout='2s'; SET LOCAL statement_timeout='20s'").await.map_err(|_| err("DB_APPLY", "적용 기한 설정 실패"))?;
    tx.execute("SELECT pg_advisory_xact_lock(1095323725, hashtext($1))", &[&schema]).await.map_err(|_| err("DB_LOCK", "migration 잠금 실패"))?;
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
    for step in &steps {
        if let Some(value) = &step.value {
            tx.execute(&step.sql, &[value]).await.map_err(|_| err("MIGRATION_FAILED", "backfill 실패; 트랜잭션 취소"))?;
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
