//! Explicit, reviewed transition from a verified prototype schema.
use super::*;

const LEGACY_CATALOG: &str = include_str!("../../../prototype/src/schema_catalog.sql");

fn legacy_idempotency(schema: &str) -> String {
    format!(
        "CREATE TABLE {schema}.aip_idem (principal text NOT NULL, key text, request text NOT NULL, result jsonb NOT NULL, PRIMARY KEY (principal, key))"
    )
}

async fn legacy_plan(db: &impl GenericClient, schema: &str, facts: &Value, wire: Option<&str>) -> Result<Value, Value> {
    configure_schema(schema)?;
    let product: String = format!("{schema}.{META}");
    if db.query_one("SELECT to_regclass($1) IS NOT NULL", &[&product]).await.map_err(|_| err("DB_CATALOG", "기존 배포 조회 실패"))?.get::<_, bool>(0)
    {
        return Err(err("ALREADY_DEPLOYED", "제품 배포 이력은 prototype 이관으로 덮어쓰지 않음"));
    }
    let marker = db
        .query_opt(&format!("SELECT ddl_digest,structure_digest FROM {schema}.aip_proto_meta WHERE singleton=true"), &[])
        .await
        .map_err(|_| err("SCHEMA_NOT_READY", "기존 prototype의 구조 marker가 필요함"))?
        .ok_or_else(|| err("SCHEMA_NOT_READY", "prototype 구조 marker가 없음"))?;
    let saved_ddl: String = marker.try_get(0).map_err(|_| err("SCHEMA_NOT_READY", "prototype marker 형식 오류"))?;
    let saved_structure: String = marker.try_get(1).map_err(|_| err("SCHEMA_NOT_READY", "prototype marker 형식 오류"))?;
    let (mut statements, _) = creation(schema, facts)?;
    statements.push(legacy_idempotency(schema));
    if digest(&json!(statements)) != saved_ddl {
        return Err(err("SCHEMA_MISMATCH", "제출한 정의와 prototype 초기화 구조가 다름"));
    }
    db.batch_execute("SET LOCAL search_path TO pg_catalog").await.map_err(|_| err("DB_CATALOG", "catalog 설정 실패"))?;
    let text: String = db.query_one(LEGACY_CATALOG, &[&schema]).await.map_err(|_| err("DB_CATALOG", "prototype 구조 조회 실패"))?.get(0);
    let legacy: Value = serde_json::from_str(&text).map_err(|_| err("DB_CATALOG", "prototype 구조 형식 오류"))?;
    let actual = digest(&json!({"format":"prototype-catalog-v1","catalog":legacy}));
    if actual != saved_structure {
        return Err(err("SCHEMA_MISMATCH", "prototype 초기화 이후 실제 구조가 달라짐"));
    }
    if let Some(wire) = wire {
        validate_existing_wire(db, schema, wire).await?;
    }
    let payload = json!({"operation":"adopt-prototype","version":VERSION,"schema":schema,"wire":wire,"executionDigest":digest(facts),"oldDdlDigest":saved_ddl,"oldStructureDigest":saved_structure,"currentStructureDigest":structure(db,schema).await?,"steps":["create product journal and immutable principal mappings","preserve application rows and committed idempotency","remove prototype marker","verify target schema"]});
    Ok(json!({"ok":true,"digest":digest(&payload),"requiresReview":true,"blocked":false,"plan":payload}))
}

pub async fn plan(db_url: &str, schema: &str, facts: &Value) -> Result<Value, Value> {
    plan_selected(db_url, schema, facts, None).await
}
pub async fn plan_with_wire(db_url: &str, schema: &str, facts: &Value, wire: &str) -> Result<Value, Value> {
    plan_selected(db_url, schema, facts, Some(wire)).await
}
async fn plan_selected(db_url: &str, schema: &str, facts: &Value, wire: Option<&str>) -> Result<Value, Value> {
    tokio::time::timeout(std::time::Duration::from_secs(12), async {
        let mut db = connect(db_url).await?;
        let tx = db.transaction().await.map_err(|_| err("DB_PLAN", "이관 계획 시작 실패"))?;
        tx.batch_execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY; SET LOCAL statement_timeout='10s'")
            .await
            .map_err(|_| err("DB_PLAN", "이관 계획 설정 실패"))?;
        let report = legacy_plan(&tx, schema, facts, wire).await?;
        tx.commit().await.map_err(|_| err("DB_PLAN", "이관 계획 종료 실패"))?;
        Ok(report)
    })
    .await
    .map_err(|_| err("DB_PLAN", "이관 계획 기한 초과"))?
}

pub async fn apply(db_url: &str, schema: &str, facts: &Value, acknowledgement: &str) -> Result<Value, Value> {
    apply_selected(db_url, schema, facts, acknowledgement, None).await
}
pub async fn apply_with_wire(db_url: &str, schema: &str, facts: &Value, acknowledgement: &str, wire: &str) -> Result<Value, Value> {
    apply_selected(db_url, schema, facts, acknowledgement, Some(wire)).await
}
async fn apply_selected(db_url: &str, schema: &str, facts: &Value, acknowledgement: &str, wire: Option<&str>) -> Result<Value, Value> {
    tokio::time::timeout(std::time::Duration::from_secs(25),async{
        configure_schema(schema)?;
        let mut db=connect(db_url).await?;
        let tx=db.transaction().await.map_err(|_|err("DB_APPLY","이관 트랜잭션 시작 실패"))?;
        tx.batch_execute("SET LOCAL lock_timeout='2s'; SET LOCAL statement_timeout='20s'").await.map_err(|_|err("DB_APPLY","이관 기한 설정 실패"))?;
        tx.execute("SELECT pg_advisory_xact_lock(1095323725,hashtext($1))",&[&schema]).await.map_err(|_|err("DB_LOCK","이관 잠금 실패"))?;
        lock_managed_tables(&tx,schema).await?;
        let product = format!("{schema}.{META}");
        let deployed: bool = tx.query_one("SELECT to_regclass($1) IS NOT NULL", &[&product]).await
            .map_err(|_| err("DB_CATALOG", "제품 배포 조회 실패"))?.get(0);
        if deployed {
            let latest = tx.query_opt(&format!("SELECT plan_digest,new_digest,steps FROM {schema}.{JOURNAL} ORDER BY id DESC LIMIT 1"), &[]).await
                .map_err(|_| err("SCHEMA_NOT_READY", "이관 journal 조회 실패"))?;
            if let Some(row) = latest {
                let prior_plan: String = row.get(0);
                let prior_facts: String = row.get(1);
                let steps: Value = row.get(2);
                if prior_plan == acknowledgement && steps.get(2).and_then(Value::as_str) == Some("remove prototype marker") {
                    let marker = verified_marker(&tx, schema).await?;
                    if let Some(requested)=wire {
                        let bound:Option<String>=tx.query_one(&format!("SELECT runtime_wire FROM {schema}.{META} WHERE singleton=true"),&[]).await
                            .map_err(|_|err("SCHEMA_NOT_READY","이관 wire 조회 실패"))?.get(0);
                        if bound.as_deref()!=Some(requested) { return Err(err("WIRE_MODE_MISMATCH","이관된 wire와 요청 wire가 다름")); }
                    }
                    if prior_facts != digest(facts) || marker.facts_digest != prior_facts {
                        return Err(err("PLAN_CHANGED", "같은 이관 계획이 다른 정의를 가리킴"));
                    }
                    tx.commit().await.map_err(|_| err("COMMIT_UNSETTLED", "이관 재조회 종료 결과 불명"))?;
                    return Ok(json!({"ok":true,"schema":schema,"alreadyApplied":true,"adopted":true,"digest":acknowledgement,"factsDigest":prior_facts}));
                }
            }
            return Err(err("ALREADY_DEPLOYED", "다른 제품 배포 이력을 prototype 이관으로 덮어쓰지 않음"));
        }
        let report=legacy_plan(&tx,schema,facts,wire).await?;
        if report["digest"].as_str()!=Some(acknowledgement){return Err(err("PLAN_CHANGED","검토한 이관 계획과 현재 정의·구조가 다름"));}
        let all=metadata_sql(schema,facts)?;
        // The existing idempotency rows are the recovery record, so never recreate their table.
        let suffix=format!("; CREATE TABLE {schema}.aip_idem (principal text NOT NULL, key text NOT NULL, request text NOT NULL, result jsonb NOT NULL, PRIMARY KEY (principal,key))");
        let metadata=all.strip_suffix(&suffix).ok_or_else(||err("ADOPTION_UNSUPPORTED","생성기 metadata 형태가 바뀜; 이관 adapter 갱신 필요"))?;
        tx.batch_execute(metadata).await.map_err(|_|err("MIGRATION_FAILED","제품 metadata 생성 실패; 이관 트랜잭션 취소"))?;
        tx.batch_execute(&format!("DROP TABLE {schema}.aip_proto_meta")).await.map_err(|_|err("MIGRATION_FAILED","prototype marker 이관 실패"))?;
        verify_target(&tx,schema,facts).await?;
        let structure_digest=structure(&tx,schema).await?;let facts_digest=digest(facts);let (_,ddl_digest)=creation(schema,facts)?;
        tx.execute(&format!("INSERT INTO {schema}.{META} VALUES(true,$1,$2,$3,$4,$5,$6)"),&[facts,&facts_digest,&ddl_digest,&structure_digest,&VERSION,&wire]).await.map_err(|_|err("MIGRATION_FAILED","제품 marker 저장 실패"))?;
        let old=report["plan"]["oldDdlDigest"].as_str().ok_or_else(||err("MIGRATION_FAILED","이전 marker 지문 없음"))?;let steps=report["plan"]["steps"].clone();
        tx.execute(&format!("INSERT INTO {schema}.{JOURNAL}(plan_digest,old_digest,new_digest,steps,approval) VALUES($1,$2,$3,$4,$5)"),&[&acknowledgement,&old,&facts_digest,&steps,&acknowledgement]).await.map_err(|_|err("MIGRATION_FAILED","이관 journal 저장 실패"))?;
        tx.commit().await.map_err(|_|err("COMMIT_UNSETTLED","이관 커밋 결과 불명; 제품 journal 확인 필요"))?;
        Ok(json!({"ok":true,"schema":schema,"digest":acknowledgement,"factsDigest":facts_digest,"structureDigest":structure_digest,"adopted":true}))
    }).await.map_err(|_|err("MIGRATION_UNSETTLED","이관 기한 초과; 제품 journal로 커밋 여부 확인 필요"))?
}
