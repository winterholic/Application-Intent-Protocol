//! Explicit adoption of a PostgreSQL schema that AIP does not own.
//! Physical names are translated through security-invoker, automatically updatable views.
use super::*;
use spike_v2_read::external::{Mapping, Table, quote, snapshot};

fn failure(msg: &str) -> Value {
    err("EXTERNAL_SCHEMA", msg)
}
#[derive(Clone)]
struct Column {
    name: String,
    ty: String,
    nullable: bool,
    default: Option<String>,
    generated: bool,
}
async fn columns(db: &impl GenericClient, schema: &str, table: &str) -> Result<Vec<Column>, Value> {
    let rows=db.query("SELECT a.attname,pg_catalog.format_type(a.atttypid,a.atttypmod),NOT a.attnotnull,pg_catalog.pg_get_expr(d.adbin,d.adrelid),(a.attgenerated<>'' OR a.attidentity='a') FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid=a.attrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=c.oid AND d.adnum=a.attnum WHERE n.nspname=$1 AND c.relname=$2 AND c.relkind IN ('r','p') AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum",&[&schema,&table]).await.map_err(|_|failure("컬럼 catalog 조회 실패"))?;
    Ok(rows.into_iter().map(|r| Column { name: r.get(0), ty: r.get(1), nullable: r.get(2), default: r.get(3), generated: r.get(4) }).collect())
}
async fn keys(db: &impl GenericClient, schema: &str, table: &str) -> Result<Value, Value> {
    let rows=db.query("SELECT c.contype::text,ARRAY(SELECT a.attname::text FROM unnest(c.conkey) WITH ORDINALITY k(attnum,ord) JOIN pg_catalog.pg_attribute a ON a.attrelid=c.conrelid AND a.attnum=k.attnum ORDER BY k.ord),tn.nspname,t.relname,ARRAY(SELECT a.attname::text FROM unnest(c.confkey) WITH ORDINALITY k(attnum,ord) JOIN pg_catalog.pg_attribute a ON a.attrelid=c.confrelid AND a.attnum=k.attnum ORDER BY k.ord) FROM pg_catalog.pg_constraint c JOIN pg_catalog.pg_class r ON r.oid=c.conrelid JOIN pg_catalog.pg_namespace n ON n.oid=r.relnamespace LEFT JOIN pg_catalog.pg_class t ON t.oid=c.confrelid LEFT JOIN pg_catalog.pg_namespace tn ON tn.oid=t.relnamespace WHERE n.nspname=$1 AND r.relname=$2 AND c.contype IN ('p','f','u') ORDER BY c.conname",&[&schema,&table]).await.map_err(|_|failure("키 catalog 조회 실패"))?;
    Ok(json!(rows.into_iter().map(|r|json!({"kind":r.get::<_,String>(0),"columns":r.get::<_,Vec<String>>(1),"schema":r.get::<_,Option<String>>(2),"table":r.get::<_,Option<String>>(3),"target":r.get::<_,Vec<String>>(4)})).collect::<Vec<_>>()))
}
fn logical(s: &str) -> String {
    s.split('_')
        .filter(|s| !s.is_empty())
        .map(|part| {
            let mut c = part.chars();
            c.next().map(|x| x.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
        })
        .collect()
}
fn scalar(ty: &str) -> Option<&'static str> {
    match ty {
        "text" => Some("Text"),
        "boolean" => Some("Bool"),
        "bigint" | "integer" | "smallint" => Some("Int"),
        "timestamp with time zone" => Some("Time"),
        "date" => Some("Date"),
        _ if ty.starts_with("character varying") || ty.starts_with("character(") => Some("Text"),
        _ => None,
    }
}
/// Read-only discovery. Unsupported keys/types fail with an actionable report, never disappear from the draft.
pub async fn introspect(url: &str, source: &str) -> Result<Value, Value> {
    if !spike_v2_read::external::valid_name(source) {
        return Err(failure("원본 schema 식별자 오류"));
    }
    if let Some((scheme, _)) = url.split_once("://") {
        let s = scheme.trim().to_ascii_lowercase();
        if s == "mysql" || s == "mariadb" {
            #[cfg(not(feature = "mysql"))]
            {
                return Err(err("DB_ADAPTER_FEATURE_DISABLED", "MySQL/MariaDB introspection을 사용하려면 mysql feature로 빌드해야 함"));
            }
            #[cfg(feature = "mysql")]
            {
                let cfg = crate::mysql_catalog::MysqlDriverConfig::parse_url(url)
                    .map_err(|e| err("DB_ADAPTER_CONFIG", &format!("MySQL/MariaDB URL 파싱 실패: {e}")))?;
                return Err(err(
                    "DB_ADAPTER_UNSUPPORTED",
                    &format!("MySQL/MariaDB({}) live driver introspection 실행기가 준비되지 않음", cfg.display_sanitized()),
                ));
            }
        }
    }
    let mut db = connect(url).await?;
    let tx = db.transaction().await.map_err(|_| failure("introspection transaction 실패"))?;
    tx.batch_execute(
        "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY; SET LOCAL statement_timeout='10s'; SET LOCAL search_path=pg_catalog",
    )
    .await
    .map_err(|_| failure("introspection snapshot 실패"))?;
    let tables=tx.query("SELECT c.relname FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relkind IN ('r','p') ORDER BY c.relname",&[&source]).await.map_err(|_|failure("테이블 조회 실패"))?;
    if tables.is_empty() {
        return Err(failure("원본 schema에 채택할 테이블이 없음"));
    }
    let mut mapping = Mapping { schema: source.into(), tables: BTreeMap::new() };
    for r in &tables {
        let table: String = r.get(0);
        let name = logical(&table);
        if !valid_ident(&name) || name.starts_with("Aip") || mapping.tables.contains_key(&name) {
            return Err(failure("resource 이름을 안전하게 만들 수 없음; 수동 매핑 필요"));
        }
        mapping.tables.insert(name, Table { table, columns: BTreeMap::new() });
    }
    let mut definition = String::from("// Introspection draft: review authorization before publishing read/write.\nactor principal\n");
    for (name, t) in &mut mapping.tables {
        let cols = columns(&tx, source, &t.table).await?;
        let ks = keys(&tx, source, &t.table).await?;
        let pk =
            ks.as_array().unwrap().iter().find(|k| k["kind"] == "p").ok_or_else(|| failure("단일 Id 기본키가 없는 테이블은 자동 채택할 수 없음"))?;
        let pks = pk["columns"].as_array().unwrap();
        if pks.len() != 1 {
            return Err(failure("복합 기본키는 아직 지원하지 않음"));
        }
        let pk = pks[0].as_str().unwrap();
        definition.push_str(&format!("resource {name} {{\n  fields {{\n"));
        for c in &cols {
            let fk = ks.as_array().unwrap().iter().find(|k| k["kind"] == "f" && k["columns"].as_array().unwrap().iter().any(|v| v == &c.name));
            let field = if c.name == pk {
                "id".into()
            } else if fk.is_some() {
                c.name.strip_suffix("_id").unwrap_or(&c.name).to_owned()
            } else {
                c.name.clone()
            };
            if !valid_ident(&field) || t.columns.contains_key(&field) || field == "xmin" {
                return Err(failure("필드 이름 충돌; 수동 매핑 필요"));
            }
            let ty = if c.name == pk {
                match c.ty.as_str() {
                    "uuid" => "Uuid".to_owned(),
                    "bigint" | "integer" | "smallint" => "Id".to_owned(),
                    _ => return Err(failure("기본키는 숫자 또는 UUID여야 함(자연키는 후속 확장)")),
                }
            } else if let Some(fk) = fk {
                if fk["columns"].as_array().unwrap().len() != 1 || fk["target"].as_array().unwrap().len() != 1 || fk["schema"] != source {
                    return Err(failure("복합·다른 schema FK는 수동 정의/매핑이 필요함"));
                }
                logical(fk["table"].as_str().unwrap())
            } else {
                scalar(&c.ty).ok_or_else(|| failure(&format!("지원하지 않는 컬럼 타입: {}.{} ({})", t.table, c.name, c.ty)))?.into()
            };
            definition.push_str(&format!(
                "    {field}: {ty}{}{};\n",
                if c.nullable { "?" } else { "" },
                if c.default.is_some() && c.name != pk { " default database" } else { "" }
            ));
            t.columns.insert(field, c.name.clone());
        }
        definition.push_str("  }\n  rows read when false\n}\n");
    }
    spike_v1_fixture::load_str(&definition, spike_v1_fixture::Form::A)
        .map_err(|_| failure("생성 정의가 유효하지 않음; 수동 resource/필드 이름 매핑 필요"))?;
    let cat = snapshot(&tx, source).await.map_err(|_| failure("구조 snapshot 실패"))?;
    tx.commit().await.map_err(|_| failure("introspection 종료 실패"))?;
    Ok(json!({"ok":true,"definition":definition,"mapping":mapping,"structureDigest":digest(&cat),"catalog":cat}))
}

async fn validate(db: &impl GenericClient, target: &str, facts: &Value, mapping: &Mapping) -> Result<(), Value> {
    if !spike_v2_read::external::valid_name(&mapping.schema)
        || mapping.schema == target
        || mapping.tables.len() != facts["resources"].as_object().unwrap().len()
    {
        return Err(failure("원본/인프라 schema는 서로 달라야 하며 모든 resource 매핑이 필요함"));
    }
    let mut used = BTreeSet::new();
    for (res, rf) in facts["resources"].as_object().unwrap() {
        let t = mapping.tables.get(res).ok_or_else(|| failure("resource 테이블 매핑 누락"))?;
        if !spike_v2_read::external::valid_name(&t.table) || !used.insert(&t.table) || sqlgen::snake(res).starts_with("aip_") {
            return Err(failure("테이블 매핑 충돌·식별자 오류"));
        }
        let cols = columns(db, &mapping.schema, &t.table).await?;
        if cols.is_empty() || t.columns.len() != rf["fields"].as_object().unwrap().len() {
            return Err(failure("테이블/필드 매핑 누락"));
        }
        let ks = keys(db, &mapping.schema, &t.table).await?;
        let pk = ks.as_array().unwrap().iter().find(|k| k["kind"] == "p").ok_or_else(|| failure("기본키 누락"))?;
        if pk["columns"].as_array().unwrap().len() != 1 || pk["columns"][0] != t.columns.get("id").map(|s| json!(s)).unwrap_or(Value::Null) {
            return Err(failure("매핑 id가 단일 기본키와 다름"));
        }
        let mut mapped = BTreeSet::new();
        for (field, fd) in rf["fields"].as_object().unwrap() {
            let physical = t.columns.get(field).ok_or_else(|| failure("컬럼 매핑 누락"))?;
            if !spike_v2_read::external::valid_name(physical) || !mapped.insert(physical) {
                return Err(failure("컬럼 매핑 중복·식별자 오류"));
            }
            let col = cols.iter().find(|c| &c.name == physical).ok_or_else(|| failure("매핑 컬럼이 없음"))?;
            let ty = fd["ty"].as_str().unwrap();
            let base = ty.trim_end_matches('?');
            let expected = if field == "id" || sqlgen::ref_target(base).is_some() { spike_v2_read::id_wire::id_cast(facts, base) } else { base };
            let ok = match expected {
                "uuid" => col.ty == "uuid",
                "bigint" => matches!(col.ty.as_str(), "bigint" | "integer" | "smallint"),
                _ => scalar(&col.ty) == Some(expected),
            };
            if fd["databaseDefault"] == true && col.default.is_none() {
                return Err(failure("default database 선언에 실제 DB default가 없음"));
            }
            if !ok || col.nullable != ty.ends_with('?') {
                return Err(failure(&format!("타입·nullable 불일치: {res}.{field}")));
            }
            let writes = rf["exposeCreate"]["fields"].as_array().is_some_and(|a| a.iter().any(|x| x == field))
                || rf["transitions"].as_object().is_some_and(|ts| {
                    ts.values().any(|tr| tr["to"].get(field).is_some() || tr["patch"].as_array().is_some_and(|a| a.iter().any(|x| x == field)))
                });
            if col.generated && writes {
                return Err(failure("generated 컬럼에는 쓰기를 공개할 수 없음"));
            }
            if let Some(target_res) = base.strip_prefix("Ref<").and_then(|x| x.strip_suffix('>')) {
                let other = mapping.tables.get(target_res).ok_or_else(|| failure("참조 resource 매핑 누락"))?;
                let correct = ks.as_array().unwrap().iter().any(|k| {
                    k["kind"] == "f"
                        && k["columns"] == json!([physical])
                        && k["schema"] == mapping.schema
                        && k["table"] == other.table
                        && k["target"] == json!([other.columns.get("id").unwrap()])
                });
                if !correct {
                    return Err(failure("Ref 매핑과 기존 FK가 다름"));
                }
            }
        }
        for unique in rf["unique"].as_array().into_iter().flatten() {
            let mut want: Vec<_> = unique.as_array().unwrap().iter().map(|f| t.columns[f.as_str().unwrap()].clone()).collect();
            want.sort();
            let existing = ks.as_array().unwrap().iter().any(|k| {
                if k["kind"] != "u" && k["kind"] != "p" {
                    return false;
                }
                let mut actual: Vec<_> = k["columns"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
                actual.sort();
                actual == want
            });
            if !existing {
                return Err(failure("AIP unique 선언에 대응하는 기존 PK/UNIQUE 제약이 없음; 원본 DBA가 관리해야 함"));
            }
        }
        if rf["invariants"].as_object().is_some_and(|xs| xs.values().any(|x| x["enforcement"]["kind"] == "partialUniqueIndex")) {
            return Err(failure("외부 schema의 AIP 부분 unique index 매핑은 아직 지원하지 않음"));
        }
        if !rf["exposeCreate"].is_null() {
            for c in &cols {
                if !mapped.contains(&c.name) && !c.nullable && c.default.is_none() && !c.generated {
                    return Err(failure("생성에 필요한 기존 NOT NULL 컬럼이 매핑되지 않음"));
                }
            }
        }
    }
    Ok(())
}

pub async fn plan_external(url: &str, schema: &str, facts: &Value, mapping: &Mapping, wire: &str) -> Result<Value, Value> {
    configure_schema(schema)?;
    wire_label(wire)?;
    validate_facts(facts)?;
    let mut db = connect(url).await?;
    let tx = db.transaction().await.map_err(|_| failure("계획 transaction 실패"))?;
    tx.batch_execute(
        "SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY; SET LOCAL statement_timeout='10s'; SET LOCAL search_path=pg_catalog",
    )
    .await
    .map_err(|_| failure("계획 snapshot 실패"))?;
    let p = derive_external(&tx, schema, facts, mapping, wire).await?;
    tx.commit().await.map_err(|_| failure("계획 종료 실패"))?;
    Ok(p)
}
async fn derive_external(db: &impl GenericClient, schema: &str, facts: &Value, mapping: &Mapping, wire: &str) -> Result<Value, Value> {
    validate(db, schema, facts, mapping).await?;
    let structure = digest(&snapshot(db, &mapping.schema).await.map_err(|_| failure("원본 구조 조회 실패"))?);
    let storage = json!({"mapping":mapping,"structureDigest":structure,"mode":"external-v1"});
    let mut deployed = facts.clone();
    deployed["externalStorage"] = storage;
    let exists: bool = db
        .query_one("SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_namespace WHERE nspname=$1)", &[&schema])
        .await
        .map_err(|_| failure("AIP schema 확인 실패"))?
        .get(0);
    let old = if exists { Some(verified_marker(db, schema).await?) } else { None };
    if let Some(old) = &old {
        if old.facts["externalStorage"]["mode"] != "external-v1"
            || old.facts["externalStorage"]["mapping"] != json!(mapping)
            || old.facts["actor"] != facts["actor"]
            || old.facts["actorMode"] != facts["actorMode"]
        {
            return Err(failure("owned schema 또는 다른 매핑·actor를 외부 채택으로 덮을 수 없음"));
        }
        let shapes = |v: &Value| {
            v["resources"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(name, r)| (name.clone(), json!({"fields":r["fields"],"idKind":r["idKind"]})))
                .collect::<BTreeMap<_, _>>()
        };
        if shapes(&old.facts) != shapes(facts) || old.facts["enums"] != facts["enums"] {
            return Err(failure("외부 resource/컬럼 타입·Id 종류 변경은 새 AIP schema의 매핑 채택이 필요함"));
        }
        let infrastructure = |v: &Value| {
            v["operations"].as_object().map(|ops| {
                ops.iter()
                    .map(|(name, o)| (name.clone(), json!({"kind":o["kind"],"input":o["input"],"output":o["output"],"access":o["access"]})))
                    .collect::<BTreeMap<_, _>>()
            })
        };
        if infrastructure(&old.facts) != infrastructure(facts) {
            return Err(failure("외부 배포의 operation 구조 변경은 새 AIP schema 채택이 필요함"));
        }
        let bound: Option<String> = db
            .query_one(&format!("SELECT runtime_wire FROM {schema}.{META} WHERE singleton"), &[])
            .await
            .map_err(|_| failure("wire 조회 실패"))?
            .get(0);
        if bound.as_deref() != Some(wire) {
            return Err(err("WIRE_MODE_MISMATCH", "외부 배포 wire 변경 거부"));
        }
    }
    let old_digest = old.as_ref().map(|m| m.facts_digest.clone());
    let digest = digest(&json!({"schema":schema,"facts":deployed,"wire":wire,"mode":"external-v1","oldFactsDigest":old_digest}));
    Ok(
        json!({"ok":true,"mode":"external","schema":schema,"sourceSchema":mapping.schema,"facts":deployed,"wire":wire,"digest":digest,"businessDDL":[],"structureDigest":structure,"oldFactsDigest":old_digest,"factsDigest":super::digest(&deployed)}),
    )
}
/// Applies to a fresh AIP schema or a metadata-only policy update; no external table ALTER/CREATE/DROP exists here.
pub async fn adopt_external(url: &str, schema: &str, facts: &Value, mapping: &Mapping, wire: &str, ack: &str) -> Result<Value, Value> {
    configure_schema(schema)?;
    wire_label(wire)?;
    validate_facts(facts)?;
    let mut db = connect(url).await?;
    let tx = db.transaction().await.map_err(|_| failure("채택 transaction 실패"))?;
    tx.batch_execute("SET LOCAL statement_timeout='15s'; SET LOCAL lock_timeout='2s'; SET LOCAL search_path=pg_catalog")
        .await
        .map_err(|_| failure("채택 기한 설정 실패"))?;
    tx.execute("SELECT pg_advisory_xact_lock(1095323725,hashtext($1))", &[&schema]).await.map_err(|_| failure("채택 잠금 실패"))?;
    // ACCESS SHARE keeps source DDL from changing between validation and the view snapshot.
    for t in mapping.tables.values() {
        if !spike_v2_read::external::valid_name(&t.table) || !spike_v2_read::external::valid_name(&mapping.schema) {
            return Err(failure("매핑 식별자 오류"));
        }
        tx.batch_execute(&format!("LOCK TABLE {}.{} IN ACCESS SHARE MODE", quote(&mapping.schema), quote(&t.table)))
            .await
            .map_err(|_| failure("기존 테이블 snapshot 잠금 실패"))?;
    }
    let p = derive_external(&tx, schema, facts, mapping, wire).await?;
    let exists: bool = tx
        .query_one("SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_namespace WHERE nspname=$1)", &[&schema])
        .await
        .map_err(|_| failure("인프라 schema 확인 실패"))?
        .get(0);
    let deployed = &p["facts"];
    let (ddl, ddl_digest) = creation(schema, deployed)?;
    if exists {
        let old = verified_marker(&tx, schema).await?;
        let prior: String = tx
            .query_one(&format!("SELECT plan_digest FROM {schema}.{JOURNAL} ORDER BY id DESC LIMIT 1"), &[])
            .await
            .map_err(|_| failure("채택 journal 조회 실패"))?
            .get(0);
        if prior == ack && old.facts == *deployed {
            tx.commit().await.map_err(|_| failure("채택 재조회 종료 실패"))?;
            return Ok(json!({"ok":true,"schema":schema,"alreadyApplied":true,"digest":ack}));
        }
        if p["digest"] != ack {
            return Err(err("PLAN_CHANGED", "이전 배포·원본 구조·정의·매핑·wire가 계획 뒤 바뀜"));
        }
        let facts_digest = digest(deployed);
        tx.execute(
            &format!("UPDATE {schema}.{META} SET facts=$1,facts_digest=$2,ddl_digest=$3 WHERE singleton"),
            &[deployed, &facts_digest, &ddl_digest],
        )
        .await
        .map_err(|_| failure("외부 정책 metadata 갱신 실패"))?;
        tx.execute(
            &format!("INSERT INTO {schema}.{JOURNAL}(plan_digest,old_digest,new_digest,steps,approval) VALUES($1,$2,$3,$4,$1)"),
            &[&ack, &old.facts_digest, &facts_digest, &json!({"mode":"external-policy","businessDDL":[]})],
        )
        .await
        .map_err(|_| failure("외부 정책 journal 기록 실패"))?;
        tx.commit().await.map_err(|_| err("COMMIT_UNSETTLED", "외부 정책 배포 커밋 결과 불명; 같은 plan으로 재조회"))?;
        return Ok(json!({"ok":true,"schema":schema,"digest":ack,"factsDigest":facts_digest,"businessDDL":[],"policyUpdated":true}));
    }
    if p["digest"] != ack {
        return Err(err("PLAN_CHANGED", "원본 구조·정의·매핑·wire가 계획 뒤 바뀜"));
    }
    for statement in ddl.iter().filter(|s| s.starts_with("CREATE SCHEMA ") || s.starts_with(&format!("CREATE TABLE {schema}.aip_"))) {
        tx.batch_execute(statement).await.map_err(|_| failure("AIP 인프라 생성 실패"))?;
    }
    for (res, t) in &mapping.tables {
        let fields = deployed["resources"][res]["fields"].as_object().unwrap();
        let mut pairs = Vec::new();
        for field in fields.keys() {
            pairs.push(format!("{} AS {}", quote(&t.columns[field]), quote(&sqlgen::column(deployed, res, field).unwrap())));
        }
        pairs.push("xmin".into());
        tx.batch_execute(&format!(
            "CREATE VIEW {schema}.{} WITH (security_invoker=true) AS SELECT {} FROM {}.{}",
            sqlgen::snake(res),
            pairs.join(","),
            quote(&mapping.schema),
            quote(&t.table)
        ))
        .await
        .map_err(|_| failure("테이블/컬럼 매핑 view 생성 실패(PG15 이상·원본 SELECT 권한 필요)"))?;
    }
    tx.batch_execute(&metadata_sql(schema, deployed)?).await.map_err(|_| failure("AIP 신원 metadata 생성 실패"))?;
    let structure = structure(&tx, schema).await?;
    let facts_digest = digest(deployed);
    tx.execute(&format!("INSERT INTO {schema}.{META}(singleton,facts,facts_digest,ddl_digest,structure_digest,generator_version,runtime_wire) VALUES(true,$1,$2,$3,$4,$5,$6)"),&[deployed,&facts_digest,&ddl_digest,&structure,&VERSION,&wire]).await.map_err(|_|failure("채택 marker 기록 실패"))?;
    tx.execute(
        &format!("INSERT INTO {schema}.{JOURNAL}(plan_digest,new_digest,steps,approval) VALUES($1,$2,$3,$1)"),
        &[&ack, &facts_digest, &json!({"mode":"external","businessDDL":[]})],
    )
    .await
    .map_err(|_| failure("채택 journal 기록 실패"))?;
    tx.commit().await.map_err(|_| err("COMMIT_UNSETTLED", "외부 채택 커밋 결과 불명; marker 조회 필요"))?;
    Ok(json!({"ok":true,"schema":schema,"sourceSchema":mapping.schema,"digest":ack,"factsDigest":facts_digest,"businessDDL":[]}))
}
/// Load the saved baseline, never re-baseline drift at serve time.
pub async fn deployment_facts(url: &str, schema: &str, facts: &Value, mapping: &Mapping) -> Result<Value, Value> {
    let db = connect(url).await?;
    let row =
        db.query_one(&format!("SELECT facts FROM {schema}.{META} WHERE singleton"), &[]).await.map_err(|_| failure("외부 채택 marker가 없음"))?;
    let deployed: Value = row.get(0);
    let mut logical = deployed.clone();
    logical.as_object_mut().unwrap().remove("externalStorage");
    if logical != *facts || deployed["externalStorage"]["mapping"] != json!(mapping) {
        return Err(err("WIRE.SCHEMA_MISMATCH", "채택된 정의·매핑과 다름"));
    }
    Ok(deployed)
}
