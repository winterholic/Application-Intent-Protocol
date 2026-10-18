//! Explicit adoption of a MySQL 8.4 business schema that AIP does not own (WBS 7.2.1.1).
//!
//! Same shape as the PostgreSQL adoption: validate the mapping against the live catalog, show a plan digest, and on
//! `--apply <digest>` create a separate AIP schema holding the engine-named views, the identity tables and the deployment
//! marker. The business tables are never altered.
//!
//! MySQL DDL commits implicitly, so the apply cannot be one transaction. The marker is therefore written `MIGRATING` first
//! and flipped to `READY` together with the journal row only after every object exists; a crash in between leaves a marker
//! the runtime refuses, and running the same apply again converges.
use super::*;
use crate::mysql_catalog::{CatalogDefault, CatalogError, LogicalType};
use crate::mysql_driver::{META_TABLE, MysqlDriver, begin_meta_bootstrap, catalog_fingerprint_on_conn, validate_ident};
use crate::mysql_external::view_statements;
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Row, TxOpts, from_row_opt};
use spike_v2_read::external::Mapping;

const MODE: &str = "mysql-external-v1";
const JOURNAL_TABLE: &str = "aip_journal";

fn failure(msg: &str) -> Value {
    err("EXTERNAL_SCHEMA", msg)
}

fn unavailable(what: &str) -> Value {
    err("DB_CONNECT", what)
}

fn from_catalog(error: CatalogError) -> Value {
    err(&error.code, &error.message)
}

#[derive(Clone, Debug)]
struct ColumnInfo {
    name: String,
    data_type: String,
    column_type: String,
    /// Case preserved: ENUM members are case-sensitive values, unlike the type words compared through `column_type`.
    column_type_raw: String,
    nullable: bool,
    max_chars: Option<u64>,
    generated: bool,
    auto_increment: bool,
    has_default: bool,
}

async fn columns(conn: &mut Conn, schema: &str, table: &str) -> Result<Vec<ColumnInfo>, Value> {
    let rows: Vec<Row> = conn
        .exec(
            "SELECT COLUMN_NAME, DATA_TYPE, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, EXTRA, CHARACTER_MAXIMUM_LENGTH, COALESCE(GENERATION_EXPRESSION, '') \
             FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
            (schema, table),
        )
        .await
        .map_err(|_| failure("컬럼 catalog 조회 실패"))?;
    rows.into_iter()
        .map(|row| {
            from_row_opt::<(String, String, String, String, Option<String>, String, Option<u64>, String)>(row)
                .map(|(name, data_type, column_type, nullable, default, extra, max_chars, generation)| {
                    let extra = extra.to_ascii_lowercase();
                    ColumnInfo {
                        name,
                        data_type: data_type.to_ascii_lowercase(),
                        column_type: column_type.to_ascii_lowercase(),
                        column_type_raw: column_type,
                        nullable: nullable == "YES",
                        max_chars,
                        generated: !generation.is_empty() || crate::mysql_catalog::extra_is_generated(&extra),
                        auto_increment: extra.contains("auto_increment"),
                        has_default: default.is_some(),
                    }
                })
                .map_err(|_| failure("컬럼 catalog 행 형식 오류"))
        })
        .collect()
}

async fn engine(conn: &mut Conn, schema: &str, table: &str) -> Result<Option<String>, Value> {
    let row: Option<(Option<String>,)> = conn
        .exec_first(
            "SELECT ENGINE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND TABLE_TYPE = 'BASE TABLE'",
            (schema, table),
        )
        .await
        .map_err(|_| failure("테이블 catalog 조회 실패"))?;
    Ok(row.and_then(|(engine,)| engine))
}

const INTEGER_TYPES: [&str; 5] = ["smallint", "mediumint", "int", "integer", "bigint"];
const TEXT_TYPES: [&str; 6] = ["char", "varchar", "tinytext", "text", "mediumtext", "longtext"];

/// Why a physical column cannot carry a declared field, or `None` when it can. Every refusal names the mismatch; nothing is
/// coerced silently.
fn type_problem(facts: &Value, field: &Value, column: &ColumnInfo) -> Option<String> {
    let ty = field["ty"].as_str().unwrap_or("");
    let base = ty.trim_end_matches('?');
    let is_id_like = base.starts_with("Id<") || base.starts_with("Ref<");
    if is_id_like {
        return match spike_v2_read::id_wire::id_cast(facts, base) {
            "uuid" => {
                let fits = match column.data_type.as_str() {
                    "char" => column.max_chars == Some(36),
                    "varchar" => column.max_chars.is_some_and(|n| n >= 36),
                    _ => false,
                };
                (!fits).then(|| format!("UUID는 CHAR(36) 또는 VARCHAR(36 이상) 컬럼이어야 함(실제 {})", column.column_type))
            }
            _ => (!INTEGER_TYPES.contains(&column.data_type.as_str()) || column.column_type.contains("unsigned") && column.data_type == "bigint")
                .then(|| format!("숫자 Id는 부호 있는 정수 컬럼이어야 함(실제 {})", column.column_type)),
        };
    }
    if let Some(members) = base.strip_prefix("Enum<").and_then(|rest| rest.strip_suffix('>')) {
        let declared: Vec<&str> = facts["enums"][members].as_array()?.iter().filter_map(Value::as_str).collect();
        return match column.data_type.as_str() {
            "enum" => {
                let live = match crate::mysql_catalog::logical_type_of(&column.data_type, &column.column_type_raw) {
                    LogicalType::Enum(values) => values,
                    _ => return Some("ENUM 정의를 해석할 수 없음".into()),
                };
                let mut live_sorted: Vec<&str> = live.iter().map(String::as_str).collect();
                let mut declared_sorted = declared.clone();
                live_sorted.sort_unstable();
                declared_sorted.sort_unstable();
                (live_sorted != declared_sorted).then(|| format!("ENUM 값 집합이 AIP enum 선언과 다름(DB {live_sorted:?}, 선언 {declared_sorted:?})"))
            }
            "char" | "varchar" => {
                let longest = declared.iter().map(|value| value.chars().count() as u64).max().unwrap_or(0);
                column.max_chars.is_some_and(|n| n < longest).then(|| "문자열 컬럼이 AIP enum의 가장 긴 값보다 짧음".to_string())
            }
            _ => Some(format!("enum은 ENUM·CHAR·VARCHAR 컬럼이어야 함(실제 {})", column.column_type)),
        };
    }
    if let Some((precision, scale)) = decimal_of(base) {
        let expected = format!("decimal({precision},{scale})");
        return (column.column_type != expected).then(|| format!("{expected} 컬럼이어야 함(실제 {})", column.column_type));
    }
    match base {
        "Json" => (column.data_type != "json").then(|| format!("Json은 JSON 컬럼이어야 함(실제 {})", column.column_type)),
        "Text" | "Email" | "Url" => {
            if !TEXT_TYPES.contains(&column.data_type.as_str()) {
                return Some(format!("문자열 컬럼이어야 함(실제 {})", column.column_type));
            }
            // A declared maximum longer than the column would make writes fail (or truncate under a lax sql_mode).
            let declared_max = field["range"].as_array().and_then(|range| range.get(1)).and_then(Value::as_i64);
            match (declared_max, column.max_chars) {
                (Some(max), Some(actual)) if max as u64 > actual => Some(format!("AIP 최대 길이 {max}가 컬럼 길이 {actual}보다 큼")),
                _ => None,
            }
        }
        "Int" => (!INTEGER_TYPES.contains(&column.data_type.as_str()) || column.column_type.contains("unsigned") && column.data_type == "bigint")
            .then(|| format!("정수 컬럼이어야 함(실제 {})", column.column_type)),
        "Bool" => (!column.column_type.starts_with("tinyint(1)") || column.column_type.contains("unsigned"))
            .then(|| format!("Bool은 TINYINT(1) 컬럼이어야 함(실제 {})", column.column_type)),
        "Time" => (!matches!(column.data_type.as_str(), "datetime" | "timestamp"))
            .then(|| format!("Time은 DATETIME 또는 TIMESTAMP여야 함(실제 {})", column.column_type)),
        "Date" => (column.data_type != "date").then(|| format!("Date는 DATE 컬럼이어야 함(실제 {})", column.column_type)),
        other => Some(format!("타입 {other}은 MySQL 채택에서 지원하지 않음")),
    }
}

fn decimal_of(ty: &str) -> Option<(u8, u8)> {
    let inner = ty.strip_prefix("Decimal<")?.strip_suffix('>')?;
    let (precision, scale) = inner.split_once(',')?;
    Some((precision.parse().ok()?, scale.parse().ok()?))
}

async fn validate(
    conn: &mut Conn,
    meta_schema: &str,
    facts: &Value,
    mapping: &Mapping,
    catalog: &crate::mysql_catalog::CanonicalSchemaCatalog,
) -> Result<(), Value> {
    let resources = facts["resources"].as_object().ok_or_else(|| failure("facts에 resources가 없음"))?;
    if !spike_v2_read::external::valid_name(&mapping.schema) || mapping.schema == meta_schema || mapping.tables.len() != resources.len() {
        return Err(failure("원본/인프라 schema는 서로 달라야 하며 모든 resource 매핑이 필요함"));
    }
    let mut used = BTreeSet::new();
    for (res, rf) in resources {
        let table = mapping.tables.get(res).ok_or_else(|| failure("resource 테이블 매핑 누락"))?;
        if !spike_v2_read::external::valid_name(&table.table) || !used.insert(&table.table) || sqlgen::snake(res).starts_with("aip_") {
            return Err(failure("테이블 매핑 충돌·식별자 오류"));
        }
        let (manifest, foreign_keys) = catalog.get(&table.table).ok_or_else(|| failure(&format!("매핑 테이블이 원본 schema에 없음: {res}")))?;
        match engine(conn, &mapping.schema, &table.table).await?.as_deref() {
            Some(engine) if engine.eq_ignore_ascii_case("InnoDB") => {}
            _ => return Err(failure(&format!("InnoDB 테이블만 채택할 수 있음: {res}"))),
        }
        let live = columns(conn, &mapping.schema, &table.table).await?;
        let fields = rf["fields"].as_object().ok_or_else(|| failure("resource fields 누락"))?;
        if live.is_empty() || table.columns.len() != fields.len() {
            return Err(failure("테이블/필드 매핑 누락"));
        }
        let primary = manifest.indexes.iter().find(|index| index.primary).ok_or_else(|| failure("기본키 누락"))?;
        if primary.columns.len() != 1 || Some(&primary.columns[0]) != table.columns.get("id") {
            return Err(failure("매핑 id가 단일 기본키와 다름"));
        }
        let mut mapped = BTreeSet::new();
        for (field, fd) in fields {
            let physical = table.columns.get(field).ok_or_else(|| failure("컬럼 매핑 누락"))?;
            if !spike_v2_read::external::valid_name(physical) || !mapped.insert(physical.clone()) {
                return Err(failure("컬럼 매핑 중복·식별자 오류"));
            }
            let column = live.iter().find(|column| &column.name == physical).ok_or_else(|| failure("매핑 컬럼이 없음"))?;
            let ty = fd["ty"].as_str().unwrap_or("");
            if let Some(problem) = type_problem(facts, fd, column) {
                return Err(failure(&format!("{res}.{field}: {problem}")));
            }
            if column.nullable != ty.ends_with('?') {
                return Err(failure(&format!("nullable 불일치: {res}.{field}")));
            }
            if fd["databaseDefault"] == true && !column.has_default && !column.auto_increment && !column.generated {
                return Err(failure("default database 선언에 실제 DB default가 없음"));
            }
            let writes = rf["exposeCreate"]["fields"].as_array().is_some_and(|a| a.iter().any(|x| x == field))
                || rf["transitions"].as_object().is_some_and(|ts| {
                    ts.values().any(|tr| tr["to"].get(field).is_some() || tr["patch"].as_array().is_some_and(|a| a.iter().any(|x| x == field)))
                });
            if column.generated && writes {
                return Err(failure("generated 컬럼에는 쓰기를 공개할 수 없음"));
            }
            if let Some(target) = ty.trim_end_matches('?').strip_prefix("Ref<").and_then(|x| x.strip_suffix('>')) {
                let other = mapping.tables.get(target).ok_or_else(|| failure("참조 resource 매핑 누락"))?;
                let other_id = other.columns.get("id").ok_or_else(|| failure("참조 대상 id 매핑 누락"))?;
                let correct = foreign_keys.iter().any(|fk| {
                    fk.columns == [physical.clone()]
                        && fk.referenced_schema == mapping.schema
                        && fk.referenced_table == other.table
                        && fk.referenced_columns == [other_id.clone()]
                });
                // The explicit `id: User` shared-PK form is a narrow compatibility opt-in for legacy 1:1 tables
                // whose PK is the parent identity but whose original DDL omitted a physical FK (FaceReview DNA).
                // All ordinary Ref fields still require the catalog FK; shared-PK columns still must be the mapped PK.
                let shared_pk_without_fk = field == "id" && rf["sharedPrimaryKey"] == target && primary.columns == [physical.clone()];
                if !correct && !shared_pk_without_fk {
                    return Err(failure("Ref 매핑과 기존 FK가 다름"));
                }
            }
        }
        for unique in rf["unique"].as_array().into_iter().flatten() {
            let mut wanted: Vec<String> = unique
                .as_array()
                .ok_or_else(|| failure("unique 선언 형식 오류"))?
                .iter()
                .map(|f| f.as_str().and_then(|name| table.columns.get(name)).cloned().ok_or_else(|| failure("unique 필드 매핑 누락")))
                .collect::<Result<_, _>>()?;
            wanted.sort();
            let existing = manifest.indexes.iter().any(|index| {
                let mut columns = index.columns.clone();
                columns.sort();
                (index.unique || index.primary) && columns == wanted
            });
            if !existing {
                return Err(failure("AIP unique 선언에 대응하는 기존 PK/UNIQUE 제약이 없음; 원본 DBA가 관리해야 함"));
            }
        }
        if rf["invariants"].as_object().is_some_and(|xs| xs.values().any(|x| x["enforcement"]["kind"] == "partialUniqueIndex")) {
            return Err(failure("외부 schema의 AIP 부분 unique index 매핑은 아직 지원하지 않음"));
        }
        if !rf["exposeCreate"].is_null() {
            for column in &manifest.columns {
                if !mapped.contains(&column.name) && !column.nullable && !column.generated && matches!(column.default, CatalogDefault::NullOrAbsent) {
                    return Err(failure("생성에 필요한 기존 NOT NULL 컬럼이 매핑되지 않음"));
                }
            }
        }
    }
    Ok(())
}

struct Marker {
    facts: Value,
    facts_digest: String,
    status: String,
    runtime_wire: Option<String>,
}

/// The deployment marker, or `None` when the AIP schema or its marker row does not exist yet.
async fn marker(conn: &mut Conn, meta_schema: &str) -> Result<Option<Marker>, Value> {
    let exists: Option<(String,)> = conn
        .exec_first("SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?", (meta_schema, META_TABLE))
        .await
        .map_err(|_| failure("AIP schema 확인 실패"))?;
    if exists.is_none() {
        return Ok(None);
    }
    let row: Option<(String, String, Option<String>, Option<String>)> = conn
        .query_first(format!(
            "SELECT facts_digest, status, CAST(facts AS CHAR), runtime_wire FROM `{meta_schema}`.`{META_TABLE}` WHERE singleton = 1"
        ))
        .await
        .map_err(|_| failure("배포 marker 조회 실패"))?;
    let Some((facts_digest, status, facts, runtime_wire)) = row else { return Ok(None) };
    let facts = match facts {
        Some(text) => serde_json::from_str(&text).map_err(|_| err("WIRE.SCHEMA_MISMATCH", "marker facts를 읽을 수 없음"))?,
        None => Value::Null,
    };
    Ok(Some(Marker { facts, facts_digest, status, runtime_wire }))
}

struct Plan {
    value: Value,
    deployed: Value,
    structure: String,
    old: Option<Marker>,
}

async fn derive(conn: &mut Conn, driver_mariadb: bool, meta_schema: &str, facts: &Value, mapping: &Mapping, wire: &str) -> Result<Plan, Value> {
    let fingerprint = catalog_fingerprint_on_conn(conn, &mapping.schema, driver_mariadb).await.map_err(from_catalog)?;
    validate(conn, meta_schema, facts, mapping, &fingerprint.catalog).await?;
    let structure = fingerprint.digest;
    let mut deployed = facts.clone();
    deployed["externalStorage"] = json!({"mapping": mapping, "structureDigest": structure, "mode": MODE});
    // A marker that never reached READY is an interrupted adoption: it is not a deployment and is rebuilt from scratch.
    let old = marker(conn, meta_schema).await?.filter(|marker| marker.status == "READY");
    if let Some(old) = &old {
        let old_facts = &old.facts;
        if old_facts["externalStorage"]["mode"] != MODE
            || old_facts["externalStorage"]["mapping"] != json!(mapping)
            || old_facts["actor"] != facts["actor"]
            || old_facts["actorMode"] != facts["actorMode"]
        {
            return Err(failure("다른 매핑·actor로 만든 배포를 외부 채택으로 덮을 수 없음"));
        }
        let shapes = |v: &Value| {
            v["resources"].as_object().map(|resources| {
                resources.iter().map(|(name, r)| (name.clone(), json!({"fields": r["fields"], "idKind": r["idKind"]}))).collect::<BTreeMap<_, _>>()
            })
        };
        if shapes(old_facts) != shapes(facts) || old_facts["enums"] != facts["enums"] {
            return Err(failure("외부 resource/컬럼 타입·Id 종류 변경은 새 AIP schema의 매핑 채택이 필요함"));
        }
        if old.runtime_wire.as_deref() != Some(wire) {
            return Err(err("WIRE_MODE_MISMATCH", "외부 배포 wire 변경 거부"));
        }
    }
    let old_digest = old.as_ref().map(|marker| marker.facts_digest.clone());
    let plan_digest = digest(&json!({"schema": meta_schema, "facts": deployed, "wire": wire, "mode": MODE, "oldFactsDigest": old_digest}));
    let value = json!({
        "ok": true, "mode": "external", "schema": meta_schema, "sourceSchema": mapping.schema, "facts": deployed, "wire": wire,
        "digest": plan_digest, "businessDDL": [], "structureDigest": structure, "oldFactsDigest": old_digest, "factsDigest": digest(&deployed),
    });
    Ok(Plan { value, deployed, structure, old })
}

fn prepare(meta_schema: &str, facts: &Value, wire: &str) -> Result<&'static str, Value> {
    configure_schema(meta_schema)?;
    let label = wire_label(wire)?;
    validate_facts(facts)?;
    validate_ident("AIP schema", meta_schema).map_err(from_catalog)?;
    Ok(label)
}

async fn connect(driver: &MysqlDriver) -> Result<Conn, Value> {
    let mut conn = driver.get_conn_with_timeout().await.map_err(|_| unavailable("MySQL 연결 실패"))?;
    // Catalog statistics are cached by default; the plan digest must see the DDL that just ran.
    let _ = conn.query_drop("SET SESSION information_schema_stats_expiry = 0").await;
    Ok(conn)
}

pub async fn plan_external(url: &str, meta_schema: &str, facts: &Value, mapping: &Mapping, wire: &str) -> Result<Value, Value> {
    prepare(meta_schema, facts, wire)?;
    let driver = MysqlDriver::from_url(url).map_err(from_catalog)?;
    let mut conn = connect(&driver).await?;
    conn.query_drop("START TRANSACTION READ ONLY, WITH CONSISTENT SNAPSHOT").await.map_err(|_| failure("계획 snapshot 실패"))?;
    let plan = derive(&mut conn, driver.is_mariadb(), meta_schema, facts, mapping, wire).await?;
    let _ = conn.query_drop("ROLLBACK").await;
    Ok(plan.value)
}

pub async fn adopt_external(url: &str, meta_schema: &str, facts: &Value, mapping: &Mapping, wire: &str, ack: &str) -> Result<Value, Value> {
    prepare(meta_schema, facts, wire)?;
    let driver = MysqlDriver::from_url(url).map_err(from_catalog)?;
    let mut conn = connect(&driver).await?;
    // One adoption of a schema at a time, across processes.
    let lock = format!("aip_adopt:{meta_schema}");
    let locked: Option<Option<i64>> = conn.exec_first("SELECT GET_LOCK(?, 5)", (lock.as_str(),)).await.map_err(|_| failure("채택 잠금 실패"))?;
    if locked.flatten() != Some(1) {
        return Err(failure("다른 채택이 진행 중임"));
    }
    let result = apply_locked(&driver, &mut conn, meta_schema, facts, mapping, wire, ack).await;
    let _ = conn.exec_drop("SELECT RELEASE_LOCK(?)", (lock.as_str(),)).await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn apply_locked(
    driver: &MysqlDriver,
    conn: &mut Conn,
    meta_schema: &str,
    facts: &Value,
    mapping: &Mapping,
    wire: &str,
    ack: &str,
) -> Result<Value, Value> {
    let plan = derive(conn, driver.is_mariadb(), meta_schema, facts, mapping, wire).await?;
    let deployed = &plan.deployed;
    let facts_digest = digest(deployed);
    let statements = view_statements(meta_schema, deployed, mapping).map_err(from_catalog)?;
    let ddl_digest = digest(&json!(statements));
    if let Some(old) = &plan.old {
        let prior: Option<(String,)> = conn
            .query_first(format!("SELECT plan_digest FROM `{meta_schema}`.`{JOURNAL_TABLE}` ORDER BY id DESC LIMIT 1"))
            .await
            .map_err(|_| failure("채택 journal 조회 실패"))?;
        if prior.as_ref().is_some_and(|(digest,)| digest == ack) && old.facts == *deployed {
            return Ok(json!({"ok": true, "schema": meta_schema, "alreadyApplied": true, "digest": ack}));
        }
    }
    if plan.value["digest"] != ack {
        return Err(err("PLAN_CHANGED", "이전 배포·원본 구조·정의·매핑·wire가 계획 뒤 바뀜"));
    }
    let facts_text = serde_json::to_string(deployed).map_err(|_| failure("facts 직렬화 실패"))?;
    let steps =
        if plan.old.is_some() { json!({"mode": "external-policy", "businessDDL": []}) } else { json!({"mode": "external", "businessDDL": []}) };
    if let Some(old) = &plan.old {
        // Policy-only update: shapes are unchanged, so the views stay. Marker and journal change in one transaction.
        let mut tx = conn.start_transaction(TxOpts::default()).await.map_err(|_| failure("정책 갱신 transaction 실패"))?;
        tx.exec_drop(
            format!("UPDATE `{meta_schema}`.`{META_TABLE}` SET facts = ?, facts_digest = ?, ddl_digest = ? WHERE singleton = 1"),
            (facts_text.as_str(), facts_digest.as_str(), ddl_digest.as_str()),
        )
        .await
        .map_err(|_| failure("외부 정책 metadata 갱신 실패"))?;
        tx.exec_drop(
            format!("INSERT INTO `{meta_schema}`.`{JOURNAL_TABLE}` (plan_digest, old_digest, new_digest, steps, approval) VALUES (?, ?, ?, ?, ?)"),
            (ack, old.facts_digest.as_str(), facts_digest.as_str(), steps.to_string(), ack),
        )
        .await
        .map_err(|_| failure("외부 정책 journal 기록 실패"))?;
        tx.commit().await.map_err(|_| err("COMMIT_UNSETTLED", "외부 정책 배포 커밋 결과 불명; 같은 plan으로 재조회"))?;
        return Ok(json!({"ok": true, "schema": meta_schema, "digest": ack, "factsDigest": facts_digest, "businessDDL": [], "policyUpdated": true}));
    }

    begin_meta_bootstrap(conn, meta_schema).await.map_err(from_catalog)?;
    for statement in &statements {
        conn.query_drop(statement.as_str())
            .await
            .map_err(|_| failure("테이블/컬럼 매핑 view 생성 실패(원본 SELECT 권한과 CREATE VIEW 권한 필요)"))?;
    }
    // The structure is read again after the DDL above: it is what the runtime fence compares every request against.
    let structure = catalog_fingerprint_on_conn(conn, &mapping.schema, driver.is_mariadb()).await.map_err(from_catalog)?.digest;
    if structure != plan.structure {
        return Err(err("PLAN_CHANGED", "채택 중 원본 schema 구조가 바뀜"));
    }
    let mut tx = conn.start_transaction(TxOpts::default()).await.map_err(|_| failure("채택 확정 transaction 실패"))?;
    tx.exec_drop(
        format!(
            "UPDATE `{meta_schema}`.`{META_TABLE}` SET facts = ?, facts_digest = ?, schema_sha256 = ?, ddl_digest = ?, runtime_wire = ?, generator_version = 1, status = 'READY' \
             WHERE singleton = 1"
        ),
        (facts_text.as_str(), facts_digest.as_str(), structure.as_str(), ddl_digest.as_str(), wire),
    )
    .await
    .map_err(|_| failure("배포 marker 확정 실패"))?;
    tx.exec_drop(
        format!("INSERT INTO `{meta_schema}`.`{JOURNAL_TABLE}` (plan_digest, new_digest, steps, approval) VALUES (?, ?, ?, ?)"),
        (ack, facts_digest.as_str(), steps.to_string(), ack),
    )
    .await
    .map_err(|_| failure("채택 journal 기록 실패"))?;
    tx.commit().await.map_err(|_| err("COMMIT_UNSETTLED", "외부 채택 커밋 결과 불명; marker 조회 필요"))?;
    Ok(json!({"ok": true, "schema": meta_schema, "sourceSchema": mapping.schema, "digest": ack, "factsDigest": facts_digest, "businessDDL": []}))
}

/// What `serve` needs from a completed adoption.
pub struct Deployment {
    pub facts: Value,
    pub facts_digest: String,
    pub structure_digest: String,
    pub source_tables: Vec<String>,
}

/// Load the saved deployment. It never re-baselines drift: the structure digest comes from the marker, and the live
/// catalog must still match it before the server starts.
pub async fn deployment(url: &str, meta_schema: &str, facts: &Value, mapping: &Mapping, wire: &str) -> Result<Deployment, Value> {
    configure_schema(meta_schema)?;
    wire_label(wire)?;
    let driver = MysqlDriver::from_url(url).map_err(from_catalog)?;
    let mut conn = connect(&driver).await?;
    // facts_digest, schema_sha256, status, facts, runtime_wire
    type MarkerRow = (String, String, String, Option<String>, Option<String>);
    let row: Option<MarkerRow> = conn
        .query_first(format!(
            "SELECT facts_digest, schema_sha256, status, CAST(facts AS CHAR), runtime_wire FROM `{meta_schema}`.`{META_TABLE}` WHERE singleton = 1"
        ))
        .await
        .map_err(|_| failure("외부 채택 marker가 없음"))?;
    let (facts_digest, structure_digest, status, stored, stored_wire) = row.ok_or_else(|| failure("외부 채택 marker가 없음"))?;
    if status != "READY" {
        return Err(err("WIRE.SCHEMA_MISMATCH", "채택이 끝나지 않음(marker가 READY가 아님); 같은 계획으로 다시 적용해야 함"));
    }
    let deployed: Value = serde_json::from_str(&stored.ok_or_else(|| err("WIRE.SCHEMA_MISMATCH", "marker facts 없음"))?)
        .map_err(|_| err("WIRE.SCHEMA_MISMATCH", "marker facts를 읽을 수 없음"))?;
    let mut logical = deployed.clone();
    logical.as_object_mut().ok_or_else(|| err("WIRE.SCHEMA_MISMATCH", "marker facts 형식 오류"))?.remove("externalStorage");
    if logical != *facts || deployed["externalStorage"]["mapping"] != json!(mapping) {
        return Err(err("WIRE.SCHEMA_MISMATCH", "채택된 정의·매핑과 다름"));
    }
    if digest(&deployed) != facts_digest || stored_wire.as_deref() != Some(wire) {
        return Err(err("WIRE.SCHEMA_MISMATCH", "marker의 facts digest 또는 wire가 일치하지 않음"));
    }
    let live = catalog_fingerprint_on_conn(&mut conn, &mapping.schema, driver.is_mariadb()).await.map_err(from_catalog)?;
    if live.digest != structure_digest {
        return Err(err("WIRE.DEPLOYMENT_CHANGED", "채택 뒤 원본 schema 구조가 바뀜"));
    }
    Ok(Deployment { facts: deployed, facts_digest, structure_digest, source_tables: live.catalog.keys().cloned().collect() })
}
