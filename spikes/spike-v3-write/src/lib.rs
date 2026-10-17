//! V3-1 단일 표준 전이 쓰기. 대상 판정과 변경을 한 트랜잭션에서 하고, 일부만 바꾼 성공 응답을 만들지 않는다.
use serde_json::{json, Map, Value};
use spike_v2_read::id_wire::{id_array_cast, id_kind, parse_entity_id, parse_typed_id, EntityId, IdWire};
use spike_v2_read::plan::{Caller, Reject};
use spike_v2_read::sqlgen::{self, column, table, Ctx, Env};
use std::time::Duration;
use tokio_postgres::{types::ToSql, Client, Transaction};

#[derive(Debug, PartialEq)]
pub struct Applied {
    pub changed: Vec<EntityId>,
    pub unchanged: Vec<EntityId>,
}

/// 테스트용 조절 손잡이. 잠금 뒤 멈춰서 동시 실행 순서를 재현한다.
#[derive(Default, Clone)]
pub struct Knobs {
    pub pause_after_lock_ms: u64,
    pub skip_row_lock: bool,
    pub skip_policy_lock: bool,
    pub pause_before_commit_ms: u64,
    /// After the row locks are held, read the database clock again; when more than 250ms passed since `caller.now` the
    /// decision is made again with the fresh time. Servers set it; tests with a fixed clock leave it off.
    pub fresh_now_after_lock: bool,
    /// 감사 행에 남길 쓰기 경로(`apply`, `extension`, `job`)와 요청 키. 감사 테이블이 설정된 때만 쓰인다.
    pub audit_via: Option<&'static str>,
    pub audit_key: Option<String>,
}

static AUDIT_TABLE: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

/// 업무 감사 테이블(`<schema>.aip_audit`)을 지정한다. 지정하면 모든 전이 변경이 같은 문장에서 감사 행을 남긴다.
/// 이름은 서버가 배포 schema에서 만든 식별자만 받는다.
pub fn set_audit_table(table: Option<String>) -> Result<(), &'static str> {
    if let Some(name) = &table {
        let valid = name.split('.').count() == 2
            && name.split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'));
        if !valid {
            return Err("invalid audit table");
        }
    }
    *AUDIT_TABLE.write().map_err(|_| "audit table lock poisoned")? = table;
    Ok(())
}

/// 감사 테이블 DDL. 전이가 쓰는 열과 같은 순서로 유지한다. 행은 추가만 하고 고치지 않는다.
/// 여러 instance가 동시에 시작해도 catalog 경합이 없도록 transaction advisory lock 뒤에서 만든다.
pub fn audit_ddl(schema: &str) -> String {
    format!(
        "BEGIN; SELECT pg_advisory_xact_lock(hashtext('{schema}.aip_audit')); \
         CREATE TABLE IF NOT EXISTS {schema}.aip_audit(id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, recorded_at timestamptz NOT NULL DEFAULT now(), \
         actor_id bigint, via text NOT NULL, request_key text, resource text NOT NULL, transition text NOT NULL, target_id bigint NOT NULL, \
         before jsonb NOT NULL, after jsonb NOT NULL); \
         CREATE INDEX IF NOT EXISTS aip_audit_target ON {schema}.aip_audit(resource, target_id, id); \
         CREATE OR REPLACE FUNCTION {schema}.aip_audit_append_only() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'aip_audit rows are append-only' USING ERRCODE='42501'; END $$; \
         CREATE OR REPLACE TRIGGER aip_audit_append_only BEFORE UPDATE ON {schema}.aip_audit FOR EACH ROW EXECUTE FUNCTION {schema}.aip_audit_append_only(); COMMIT"
    )
}

/// 커밋 직전 남은 기한이 이보다 적으면 커밋하지 않고 rollback한다. 커밋을 보낸 뒤의 기한 초과와 구분하기 위해서다(R3-05).
pub const COMMIT_MARGIN_MS: u64 = 300;

/// 요청 하나의 기한 상태.
#[derive(Clone)]
pub struct Ctl {
    started: std::time::Instant,
    commit_sent: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// 쓰기 요청 전체 기한. 계획·잠금 대기·변경·커밋을 모두 포함한다(R2B-07).
pub const WRITE_DEADLINE_MS: u64 = 2000;

/// 직렬화 실패(40001)·교착(40P01)으로 전체 rollback된 요청의 메시지 꼬리말. 서버가 같은 요청을 다시 실행해도 되는지의 표지다.
const RETRY_MARK: &str = "다시 시도 가능";

/// SERIALIZABLE 쓰기(O-26)를 서버가 내부에서 다시 실행하는 최대 횟수. 첫 실행 뒤 재시도 횟수이며 같은 쓰기 기한 안에서 돈다.
pub const SERIALIZATION_RETRIES: usize = 3;

/// 이 거절이 직렬화 실패·교착으로 전체 rollback된 것인지. 다른 `WIRE.CONFLICT`(판정과 실제 변경 수 불일치)는 해당하지 않는다.
pub fn is_retryable(reject: &Reject) -> bool {
    reject.code == "WIRE.CONFLICT" && reject.msg.ends_with(RETRY_MARK)
}

/// 호출자가 정한 쓰기 단계에서 직렬화 충돌과 같게 다룰 거절(예: SERIALIZABLE에서 멱등 키 행 삽입 충돌). 재시도 대상이다.
pub fn serialization_conflict(stage: &str) -> Reject {
    Reject { code: "WIRE.CONFLICT", msg: format!("{stage}: 동시 쓰기와 충돌. {RETRY_MARK}") }
}

/// 재시도 사이 대기. 같은 두 요청이 같은 박자로 다시 부딪치지 않게 시도마다 늘리고 흩뜨린다.
pub async fn retry_backoff(attempt: usize) {
    let jitter = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| u64::from(d.subsec_nanos())) % 20;
    // 20, 40, 80ms 안팎: 충돌 상대 트랜잭션이 끝날 시간을 주되 3회가 한 번에 소진되지 않게 한다.
    tokio::time::sleep(Duration::from_millis(10 * (1u64 << attempt.min(6)) + jitter)).await;
}

fn db_err(stage: &str, e: tokio_postgres::Error) -> Reject {
    match e.code().map(|c| c.code()) {
        Some("57014") => Reject { code: "WIRE.DEADLINE_EXCEEDED", msg: format!("{stage}: {WRITE_DEADLINE_MS}ms 안에 끝나지 않음") },
        // 교착·직렬화 실패는 전체 rollback된 상태라 다시 시도할 수 있다.
        Some("40P01") | Some("40001") => Reject { code: "WIRE.CONFLICT", msg: format!("{stage}: 동시 쓰기와 충돌. {RETRY_MARK}") },
        c => Reject { code: "WIRE.INTERNAL", msg: format!("{stage} 실패(sqlstate {})", c.unwrap_or("?")) },
    }
}

fn rej<T>(code: &'static str, msg: impl Into<String>) -> Result<T, Reject> {
    Err(Reject { code, msg: msg.into() })
}
fn internal(e: impl ToString) -> Reject {
    Reject { code: "WIRE.INTERNAL", msg: e.to_string() }
}

pub fn keys(o: &Map<String, Value>, allowed: &[&str], what: &str) -> Result<(), Reject> {
    match o.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => rej("WIRE.UNKNOWN_KEY", format!("{what}에 알 수 없는 키 `{k}`")),
        None => Ok(()),
    }
}

#[cfg(test)]
mod scalar_literal_tests {
    use super::{literal, IdWire, Src};
    use serde_json::json;

    #[test]
    fn email_and_date_literals_are_validated_and_use_typed_sql_casts() {
        let facts = json!({"resources":{"Item":{"fields":{"email":{"ty":"Email"},"date":{"ty":"Date"}}}}});
        assert!(
            matches!(literal(&facts, "Item", "email", &json!("person@example.test"), IdWire::Legacy), Ok(Src::Lit(s, "text")) if s == "person@example.test")
        );
        assert!(literal(&facts, "Item", "email", &json!("not-an-email"), IdWire::Legacy).is_err());
        assert!(matches!(literal(&facts, "Item", "date", &json!("2024-02-29"), IdWire::Legacy), Ok(Src::Lit(s, "date")) if s == "2024-02-29"));
        assert!(literal(&facts, "Item", "date", &json!("2026-02-29"), IdWire::Legacy).is_err());
    }

    #[test]
    fn decimal_create_values_are_validated_and_use_numeric_bindings() {
        let facts = json!({"resources":{"Item":{"fields":{"amount":{"ty":"Decimal<5,2>"}}}}});
        assert!(matches!(literal(&facts, "Item", "amount", &json!("12.30"), IdWire::Legacy), Ok(Src::Lit(s, "numeric")) if s == "12.30"));
        for invalid in ["1234.56", "1.234", "NaN", "1e3", "+1", "01.00"] {
            assert!(literal(&facts, "Item", "amount", &json!(invalid), IdWire::Legacy).is_err(), "{invalid}");
        }
        assert!(literal(&facts, "Item", "amount", &json!(12.3), IdWire::Legacy).is_err());
    }

    #[test]
    fn create_literals_enforce_text_code_points_integer_bounds_and_nullable_ranges() {
        let facts = json!({"resources":{"Comment":{"fields":{
            "content":{"ty":"Text","range":[1,500]},
            "score":{"ty":"Int","range":[0,100]},
            "memo":{"ty":"Text?","range":[1,5]}
        }}}});
        for value in [json!(""), json!("😀".repeat(501))] {
            assert_eq!(literal(&facts, "Comment", "content", &value, IdWire::Legacy).err().unwrap().code, "WIRE.BAD_VALUE");
        }
        for value in [json!("a"), json!("😀".repeat(500))] {
            assert!(literal(&facts, "Comment", "content", &value, IdWire::Legacy).is_ok());
        }
        for value in [json!(-1), json!(101), json!(1.5)] {
            assert_eq!(literal(&facts, "Comment", "score", &value, IdWire::Legacy).err().unwrap().code, "WIRE.BAD_VALUE");
        }
        for value in [json!(0), json!(100)] {
            assert!(literal(&facts, "Comment", "score", &value, IdWire::Legacy).is_ok());
        }
        assert!(literal(&facts, "Comment", "memo", &json!(null), IdWire::Legacy).is_ok());
        assert_eq!(literal(&facts, "Comment", "memo", &json!(""), IdWire::Legacy).err().unwrap().code, "WIRE.BAD_VALUE");
    }

    #[test]
    fn create_time_url_and_text_use_the_common_scalar_parser() {
        let facts = json!({"resources":{"Comment":{"fields":{
            "at":{"ty":"Time"},"url":{"ty":"Url"},"content":{"ty":"Text"}
        }}}});
        assert!(literal(&facts, "Comment", "at", &json!("2026-10-15T00:00:00Z"), IdWire::Legacy).is_ok());
        assert!(literal(&facts, "Comment", "url", &json!("https://example.test/video"), IdWire::Legacy).is_ok());
        for (field, value) in [("at", json!("invalid")), ("content", json!("text\u{0}tail")), ("url", json!("https://example.test/\u{0}"))] {
            assert_eq!(literal(&facts, "Comment", field, &value, IdWire::Legacy).err().unwrap().code, "WIRE.BAD_VALUE");
        }
    }
}

pub async fn apply(db: &mut Client, facts: &Value, req: &Value, caller: &Caller, knobs: &Knobs) -> Result<Applied, Reject> {
    let serializable = apply_needs_serializable(facts, req);
    with_deadline(|ctl| async move {
        let mut attempt = 0;
        let retry_knobs = Knobs { pause_after_lock_ms: 0, pause_before_commit_ms: 0, ..knobs.clone() };
        loop {
            match apply_inner(&mut *db, facts, req, caller, if attempt == 0 { knobs } else { &retry_knobs }, ctl.clone(), serializable).await {
                Err(e) if serializable && is_retryable(&e) && attempt < SERIALIZATION_RETRIES => {
                    attempt += 1;
                    retry_backoff(attempt).await;
                }
                other => return other,
            }
        }
    })
    .await
}

async fn with_deadline<T, F, Fut>(f: F) -> Result<T, Reject>
where
    F: FnOnce(Ctl) -> Fut,
    Fut: std::future::Future<Output = Result<T, Reject>>,
{
    let ctl = Ctl { started: std::time::Instant::now(), commit_sent: Default::default() };
    let sent = ctl.commit_sent.clone();
    match tokio::time::timeout(Duration::from_millis(WRITE_DEADLINE_MS), f(ctl)).await {
        Ok(r) => r,
        // 커밋을 보낸 뒤라면 커밋됐을 수도 있다. 실패로 단정하지 않고 결과 미확정으로 알린다(R3-05).
        Err(_) if sent.load(std::sync::atomic::Ordering::SeqCst) => rej("WIRE.COMMIT_UNKNOWN", "커밋 결과를 기한 안에 확인하지 못함. 상태 조회 필요"),
        // 커밋을 보내기 전이면 트랜잭션은 drop되며 rollback된다.
        Err(_) => rej("WIRE.DEADLINE_EXCEEDED", format!("쓰기 요청이 {WRITE_DEADLINE_MS}ms 안에 끝나지 않음")),
    }
}

async fn commit(tx: Transaction<'_>, ctl: &Ctl, knobs: &Knobs) -> Result<(), Reject> {
    if knobs.pause_before_commit_ms > 0 {
        tokio::time::sleep(Duration::from_millis(knobs.pause_before_commit_ms)).await;
    }
    let left = WRITE_DEADLINE_MS.saturating_sub(ctl.started.elapsed().as_millis() as u64);
    if left < COMMIT_MARGIN_MS {
        tx.rollback().await.ok();
        return rej("WIRE.DEADLINE_EXCEEDED", format!("커밋 전 남은 기한 {left}ms. rollback함"));
    }
    // 지연 제약(커밋 시점 불변식) 위반은 커밋에서 드러난다.
    send_commit(tx, || ctl.commit_sent.store(true, std::sync::atomic::Ordering::SeqCst)).await.map_err(CommitFailure::into_reject)
}

fn bad(msg: impl Into<String>) -> Reject {
    Reject { code: "WIRE.BAD_REQUEST", msg: msg.into() }
}

fn id_array(ids: &[EntityId]) -> String {
    format!("{{{}}}", ids.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","))
}

fn bind_all(params: &[Option<String>]) -> Vec<&(dyn ToSql + Sync)> {
    params.iter().map(|p| p as &(dyn ToSql + Sync)).collect()
}

// 매개변수는 모두 text로 보내고 SQL 안에서 캐스트한다. 타입을 명시해야 식에서 쓰이지 않은
// 매개변수(인자를 쓰지 않는 predicate 등)도 42P18 없이 실행된다.
async fn typed_query(tx: &Transaction<'_>, sql: &str, params: &[Option<String>]) -> Result<Vec<tokio_postgres::Row>, tokio_postgres::Error> {
    let statement = tx.prepare_typed(sql, &vec![tokio_postgres::types::Type::TEXT; params.len()]).await?;
    tx.query(&statement, &bind_all(params)).await
}

pub enum TargetSpec {
    Ids(Vec<EntityId>),
    Where(Vec<(String, String, &'static str)>),
}

fn parse_ids(facts: &Value, res: &str, raw: &Value, bulk: i64, wire: IdWire) -> Result<Vec<EntityId>, Reject> {
    let raw = raw.as_array().ok_or(bad("ids는 배열"))?;
    if raw.is_empty() {
        return Err(bad("ids가 비었음"));
    }
    // 상한을 먼저 본다. 큰 배열의 중복 검사가 기한 밖 CPU 시간을 쓰지 않게 한다.
    if raw.len() as i64 > bulk {
        return rej("WIRE.BULK_LIMIT", format!("대상 {}개 > 상한 {bulk}", raw.len()));
    }
    let mut v = vec![];
    for x in raw {
        let n = parse_entity_id(facts, res, x, wire)?;
        if v.contains(&n) {
            return rej("WIRE.DUPLICATE_TARGET", format!("id {n} 중복"));
        }
        v.push(n);
    }
    Ok(v)
}

pub fn parse_target(facts: &Value, res: &str, ex: &Value, tg: &Value, wire: IdWire) -> Result<TargetSpec, Reject> {
    let rf = &facts["resources"][res];
    let bulk = ex["bulkMaxRows"].as_i64().unwrap_or(1);
    let forms: Vec<&str> = ex["target"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    let tg = tg.as_object().ok_or(bad("target 객체 필요"))?;
    keys(tg, &["ids", "where"], "target")?;
    match (tg.get("ids"), tg.get("where")) {
        (Some(ids), None) => {
            if !forms.contains(&"id") {
                return rej("WIRE.TARGET_NOT_ALLOWED", "id 대상은 계약에 없음");
            }
            Ok(TargetSpec::Ids(parse_ids(facts, res, ids, bulk, wire)?))
        }
        (None, Some(w)) => {
            if !forms.contains(&"where") {
                return rej("WIRE.TARGET_NOT_ALLOWED", "where 대상은 계약에 없음");
            }
            let allow = rf["exposeRead"]["filter"].as_array().cloned().unwrap_or_default();
            let mut v = vec![];
            for f in w.as_array().ok_or(bad("where는 배열"))? {
                let fo = f.as_object().ok_or(bad("where 항목은 객체"))?;
                keys(fo, &["field", "op", "value"], "where")?;
                let (field, op) = (fo.get("field").and_then(Value::as_str).unwrap_or(""), fo.get("op").and_then(Value::as_str).unwrap_or(""));
                // 쓰기 대상 조건도 읽기 filter 허용 목록을 따른다. 아니면 변경 개수로 숨은 값을 추론할 수 있다.
                if !allow.iter().any(|x| x == &json!(format!("{field}.{op}"))) || op != "eq" {
                    return rej("WIRE.FILTER_NOT_ALLOWED", format!("`{field}.{op}` 조건은 계약에 없음"));
                }
                // 같은 필드 eq를 반복하면 의미 없이 SQL만 커진다. 조건 수는 허용 목록 크기로 제한된다.
                if v.iter().any(|(c, _, _): &(String, String, &str)| Some(c) == column(facts, res, field).as_ref()) {
                    return rej("WIRE.DUPLICATE_FILTER", format!("`{field}.{op}` 조건 중복"));
                }
                let ty = rf["fields"][field]["ty"].as_str().unwrap_or("").trim_end_matches('?');
                // 읽기는 Ref 필터에 대상 행 정책을 함께 건다. 쓰기 대상 조건은 그 검사가 없어
                // 변경 결과로 안 보이는 대상 id를 확인할 수 있으므로 Ref 조건을 받지 않는다.
                if ty.starts_with("Ref<") {
                    return rej("WIRE.FILTER_NOT_ALLOWED", format!("`{field}.{op}`: 쓰기 대상 조건에는 참조 필드를 쓸 수 없음. ids 대상을 사용"));
                }
                let value = fo.get("value").unwrap_or(&Value::Null);
                let (val, cast) = if ty.starts_with("Id<") || ty.starts_with("Ref<") {
                    parse_typed_id(facts, ty, value, wire)?
                } else {
                    spike_v2_read::scalar::parse(facts, ty, value)?
                };
                v.push((column(facts, res, field).unwrap(), val, cast));
            }
            Ok(TargetSpec::Where(v))
        }
        _ => Err(bad("target은 ids 또는 where 중 하나")),
    }
}

struct Judged {
    /// The time the decision was made with; later statements of the same write use it too.
    now: String,
    change: Vec<EntityId>,
    unchanged: Vec<EntityId>,
    scopes: Vec<Option<String>>,
}

/// 판정 SQL 하나와 그 바인드 값. `now`는 이 문장이 보는 시각이고 `locked`가 참이면 대상 행을 잠근다.
#[allow(clippy::too_many_arguments)]
fn judge_query(
    facts: &Value,
    res: &str,
    tr: &str,
    target: &TargetSpec,
    bulk: i64,
    scope: &Value,
    actor_id: Option<i64>,
    now: &str,
    locked: bool,
    knobs: &Knobs,
) -> Result<(String, Vec<Option<String>>), Reject> {
    let rf = &facts["resources"][res];
    let t = &rf["transitions"][tr];
    let mut cx = Ctx::new(facts, actor_id, now);
    cx.lock_reads = !knobs.skip_policy_lock;
    let env = Env { this: Some(("t".into(), res.to_string())), ..Default::default() };
    let vis = cx.cond(&rf["rowRead"], &env).map_err(internal)?;
    let allow = cx.cond(&t["allow"], &env).map_err(internal)?;
    let from = cx.cond(&t["from"], &env).map_err(internal)?;
    let mut done = vec![];
    for (f, v) in t["to"].as_object().unwrap() {
        let col = column(facts, res, f).unwrap();
        if v.get("increment").is_some() {
            // 증감은 "이미 목표 상태"가 없다. sema가 repeat unchanged와 함께 쓰지 못하게 막는다.
            done.push("FALSE".into());
            continue;
        }
        let val = cx.value(v, &env).map_err(internal)?;
        done.push(format!("(t.{col} IS NOT DISTINCT FROM {val})"));
    }
    // 호출자 patch는 매번 실제 쓰기다. 멱등 재시도는 같은 key의 저장 결과를 재생한다.
    let done = if t.get("patch").is_some() { "FALSE".into() } else { format!("({})", done.join(" AND ")) };
    let scope_sql = if scope.is_null() { "NULL::text".to_string() } else { format!("({})::text", cx.value(scope, &env).map_err(internal)?) };
    let lock = if !locked || knobs.skip_row_lock { "" } else { " FOR UPDATE OF t" };
    let sel = match target {
        TargetSpec::Ids(ids) => {
            let p = cx.params.bind(Some(id_array(ids)), id_array_cast(facts, res));
            // 행 정책을 WHERE에 둬서 안 보이는 행은 잠그지도 기다리지도 않는다(R2B-11).
            format!("SELECT t.id, {allow}, {from}, {done}, {scope_sql} FROM {} t WHERE t.id = ANY({p}) AND {vis} ORDER BY t.id{lock}", table(res))
        }
        TargetSpec::Where(conds) => {
            let lim = cx.params.bind(Some((bulk + 1).to_string()), "bigint");
            // where 대상은 지금 바꿀 행(from 충족)만이다(R2B-04). 상한+1개까지만 잠근다.
            let mut w = vec![vis.clone(), allow.clone(), from.clone()];
            for (col, val, cast) in conds {
                let p = cx.params.bind(Some(val.clone()), cast);
                w.push(format!("(t.{col} = {p})"));
            }
            format!("SELECT t.id, TRUE, {from}, {done}, {scope_sql} FROM {} t WHERE {} ORDER BY t.id LIMIT {lim}{lock}", table(res), w.join(" AND "))
        }
    };
    let params = cx.params.values.clone();
    Ok((sel, params))
}

/// 대상 행을 잠그고 행 정책·allow·from·이미 목표 상태 여부, sameScope 값을 판정한다.
#[allow(clippy::too_many_arguments)]
async fn judge(
    tx: &Transaction<'_>,
    facts: &Value,
    res: &str,
    tr: &str,
    target: &TargetSpec,
    bulk: i64,
    scope: &Value,
    caller: &Caller,
    knobs: &Knobs,
) -> Result<Judged, Reject> {
    let t = &facts["resources"][res]["transitions"][tr];
    let (sel, params) = judge_query(facts, res, tr, target, bulk, scope, caller.actor_id, &caller.now, true, knobs)?;
    let mut rows = typed_query(tx, sel.as_str(), &params).await.map_err(|e| db_err("대상 조회", e))?;
    if knobs.pause_after_lock_ms > 0 {
        tokio::time::sleep(Duration::from_millis(knobs.pause_after_lock_ms)).await;
    }
    // The clock was read before the idempotency and row lock waits (up to 2s). The rows are locked now, so decide again
    // with the current time; a deadline that passed during the wait must not be honoured by a stale `now`.
    let mut now_used = caller.now.clone();
    if knobs.fresh_now_after_lock {
        let clock = tx
            .query_one("SELECT clock_timestamp()::text, (clock_timestamp() - $1::text::timestamptz) > interval '250 milliseconds'", &[&caller.now])
            .await
            .map_err(|e| db_err("판정 시각", e))?;
        if clock.get::<_, bool>(1) {
            now_used = clock.get::<_, String>(0);
            let (again, again_params) = judge_query(facts, res, tr, target, bulk, scope, caller.actor_id, &now_used, false, knobs)?;
            rows = typed_query(tx, again.as_str(), &again_params).await.map_err(|e| db_err("대상 재판정", e))?;
        }
    }
    let mut j = Judged { now: now_used, change: vec![], unchanged: vec![], scopes: vec![] };
    let mut seen = vec![];
    for r in &rows {
        let (id, a, f, d, sc): (EntityId, Option<bool>, Option<bool>, Option<bool>, Option<String>) =
            (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4));
        seen.push(id);
        if a != Some(true) {
            return rej("WIRE.FORBIDDEN", format!("{id}에 `{tr}` 권한 없음"));
        }
        // 이미 목표 상태면 from보다 먼저 본다(R2B-10).
        if d == Some(true) && t["repeat"] == "unchanged" {
            j.unchanged.push(id);
        } else if f == Some(true) {
            j.change.push(id);
        } else {
            return rej("WIRE.INVALID_STATE", format!("{id}는 `{tr}` 이전 상태가 아님"));
        }
        j.scopes.push(sc);
    }
    match target {
        TargetSpec::Ids(ids) => {
            let missing = ids.iter().filter(|x| !seen.contains(x)).count();
            if missing > 0 {
                // 없는 행과 안 보이는 행을 구분하지 않는다.
                return rej("WIRE.MISSING_TARGET", format!("대상 {missing}개를 찾을 수 없음"));
            }
        }
        TargetSpec::Where(_) if seen.len() as i64 > bulk => return rej("WIRE.BULK_LIMIT", format!("대상이 상한 {bulk}을 넘음")),
        _ => {}
    }
    // 같은 범위(예: 같은 동아리) 검사는 가시성·권한 판정 뒤에 한다. 권한 없는 호출자는 범위 일치 여부를 알 수 없다.
    if !scope.is_null() {
        let first = j.scopes.first().cloned().flatten();
        if first.is_none() || j.scopes.iter().any(|x| *x != first) {
            return rej("WIRE.NOT_SAME_SCOPE", "대상들이 같은 범위가 아님");
        }
    }
    j.change.sort();
    j.unchanged.sort();
    Ok(j)
}

#[allow(clippy::too_many_arguments)]
async fn update(
    tx: &Transaction<'_>,
    facts: &Value,
    res: &str,
    tr: &str,
    change: &[EntityId],
    caller: &Caller,
    knobs: &Knobs,
    input: Option<&ApplyInput>,
) -> Result<(), Reject> {
    if change.is_empty() {
        return Ok(());
    }
    let t = &facts["resources"][res]["transitions"][tr];
    let mut cx = Ctx::new(facts, caller.actor_id, &caller.now);
    let env = Env { this: Some(("t".into(), res.to_string())), ..Default::default() };
    let mut from = cx.cond(&t["from"], &env).map_err(internal)?;
    if let Some(input) = input {
        for (field, src) in &input.expected {
            let col = column(facts, res, field).unwrap();
            let value = input_sql(&mut cx, res, field, src)?;
            from = format!("({from}) AND (t.{col} IS NOT DISTINCT FROM {value})");
        }
    }
    let mut sets = vec![];
    for (f, v) in t["to"].as_object().unwrap() {
        let col = column(facts, res, f).unwrap();
        if let Some(delta) = v.get("increment") {
            let delta = delta.as_i64().ok_or_else(|| internal("증감 값".to_string()))?;
            let p = cx.params.bind(Some(delta.to_string()), "bigint");
            // 잠근 행의 현재 값에서 계산하므로 동시 증감이 서로를 덮어쓰지 않는다.
            sets.push(format!("{col} = t.{col} + {p}"));
            continue;
        }
        sets.push(format!("{col} = {}", cx.value(v, &env).map_err(internal)?));
    }
    if let Some(input) = input {
        for (field, src) in &input.values {
            let col = column(facts, res, field).unwrap();
            sets.push(format!("{col} = {}", input_sql(&mut cx, res, field, src)?));
        }
    }
    // 증감한 Int 필드의 선언 범위. DDL CHECK가 없으므로 같은 트랜잭션에서 UPDATE 결과로 확인한다.
    let mut ranged = vec![];
    for (f, v) in t["to"].as_object().unwrap() {
        let range = &facts["resources"][res]["fields"][f]["range"];
        if let (Some(_), Some(lo), Some(hi)) = (v.get("increment"), range[0].as_i64(), range[1].as_i64()) {
            ranged.push(format!("t.{} NOT BETWEEN {lo} AND {hi}", column(facts, res, f).unwrap()));
        }
    }
    let ip = cx.params.bind(Some(id_array(change)), id_array_cast(facts, res));
    // UPDATE에도 from 조건을 다시 건다. 잠금이 없거나 우회돼도 이미 바뀐 행을 두 번 바꾸지 않는다.
    let out_of_range = if ranged.is_empty() { "FALSE".to_string() } else { format!("({})", ranged.join(" OR ")) };
    let audit_table = AUDIT_TABLE.read().map_err(|_| internal("감사 테이블 잠금"))?.clone();
    let up = match audit_table {
        None => format!("UPDATE {} t SET {} WHERE t.id = ANY({ip}) AND {from} RETURNING t.id, {out_of_range}", table(res), sets.join(", ")),
        Some(audit) => {
            // 바뀌는 필드의 이전 값은 같은 문장의 스냅숏에서, 이후 값은 RETURNING에서 읽는다. 감사 행은 전이와 함께만 commit된다.
            let snapshot = |alias: &str| {
                let pairs: Vec<String> = t["to"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .chain(input.into_iter().flat_map(|i| i.values.iter().map(|(f, _)| f)))
                    .map(|f| format!("'{f}', to_jsonb({alias}.{})", column(facts, res, f).unwrap()))
                    .collect();
                // jsonb_build_object takes at most 100 arguments, so wide transitions are built in parts.
                let parts: Vec<String> = pairs.chunks(50).map(|chunk| format!("jsonb_build_object({})", chunk.join(", "))).collect();
                if parts.is_empty() {
                    "'{}'::jsonb".to_string()
                } else {
                    parts.join(" || ")
                }
            };
            let actor = cx.params.bind(caller.actor_id.map(|id| id.to_string()), "bigint");
            let via = cx.params.bind(Some(knobs.audit_via.unwrap_or("apply").to_string()), "text");
            let key = cx.params.bind(knobs.audit_key.clone(), "text");
            let resource = cx.params.bind(Some(res.to_string()), "text");
            let transition = cx.params.bind(Some(tr.to_string()), "text");
            format!(
                "WITH changed AS (UPDATE {table} t SET {sets} FROM (SELECT aip_p.id, {before} AS before FROM {table} aip_p WHERE aip_p.id = ANY({ip})) aip_prior \
                 WHERE t.id = aip_prior.id AND t.id = ANY({ip}) AND {from} RETURNING t.id, {out_of_range} AS out_of_range, aip_prior.before, {after} AS after), \
                 recorded AS (INSERT INTO {audit}(actor_id, via, request_key, resource, transition, target_id, before, after) \
                 SELECT {actor}, {via}, {key}, {resource}, {transition}, id, before, after FROM changed) \
                 SELECT id, out_of_range FROM changed",
                table = table(res),
                sets = sets.join(", "),
                before = snapshot("aip_p"),
                after = snapshot("t"),
            )
        }
    };
    let params = cx.params.values.clone();
    let rows = typed_query(tx, up.as_str(), &params).await.map_err(|e| write_err("변경", e))?;
    // RETURNING은 갱신 뒤 값을 본다. 하나라도 범위를 벗어나면 전체 트랜잭션이 롤백된다.
    if rows.iter().any(|r| r.get::<_, bool>(1)) {
        return rej("WIRE.BAD_VALUE", "증감 결과가 필드 범위를 벗어남");
    }
    let n = rows.len();
    if n != change.len() {
        return rej("WIRE.CONFLICT", format!("판정 {}개와 실제 변경 {n}개가 다름. 전체 취소", change.len()));
    }
    Ok(())
}

/// 연결이 끊기거나 응답을 읽을 수 없어 서버가 문장을 끝냈는지 알 수 없는 오류인지 가른다.
/// SQLSTATE가 있는 서버 오류는 문장 결과가 확정이라(제약 위반·직렬화 실패 등) 여기에 넣지 않는다.
/// tokio-postgres는 오류 종류를 공개하지 않아 `is_closed`와 표시 문구(Io·UnexpectedMessage·Parse)로 가른다.
/// 문구가 바뀌면 `unexpected_message_*`·`unparseable_*` 테스트가 먼저 깨진다.
/// Io는 연결 task에서만 나서 호출자의 COMMIT 호출에는 Closed로 보인다(RST 테스트로 확인). Io 문구 판정은 접속 단계의 실제 Io 오류로
/// 고정한다(`io_error_during_startup_is_a_connection_failure`).
pub fn connection_failed(e: &tokio_postgres::Error) -> bool {
    e.code().is_none()
        && (e.is_closed()
            || ["error communicating with the server", "unexpected message from server", "error parsing response from server"]
                .iter()
                .any(|prefix| e.to_string().starts_with(prefix)))
}

/// COMMIT을 보내기 전에 연결을 잃은 실패. 서버는 끊긴 연결의 열린 트랜잭션을 rollback하므로 아무것도 커밋되지 않았다.
/// 같은 키로 다시 보내도 안전하다.
pub fn lost_before_commit() -> Reject {
    Reject { code: "WIRE.DB_UNAVAILABLE", msg: "DB 연결을 잃음. 커밋되지 않았음".into() }
}

/// COMMIT 전 단계에서 쓰기 엔진이 오류 원본을 지우고 돌려준 거절이, 연결이 이미 닫힌 뒤의 것이라면 내부 오류가 아니라 연결 끊김이다.
pub fn before_commit_rejection(tx: &Transaction<'_>, reject: Reject) -> Reject {
    if reject.code == "WIRE.INTERNAL" && tx.client().is_closed() {
        lost_before_commit()
    } else {
        reject
    }
}

/// COMMIT 한 번의 실패를 보낸 시점 기준으로 나눈 것. 모든 COMMIT 경로가 이 분류를 같이 쓴다.
#[derive(Debug)]
pub enum CommitFailure {
    /// COMMIT이 나가지 못했다(이미 닫힌 연결). 서버는 열린 트랜잭션을 rollback하므로 커밋되지 않았고 다시 보내도 안전하다.
    NotSent,
    /// COMMIT을 보낸 뒤 연결이 끊기거나 응답을 해석할 수 없다. 서버가 커밋했는지 이 연결로는 알 수 없다.
    Unknown,
    /// 서버가 SQLSTATE와 함께 거절했다(제약 위반·직렬화 실패·교착). rollback이 확정이라 호출 경로의 기존 분류를 따른다.
    Rejected(tokio_postgres::Error),
}

impl CommitFailure {
    pub fn into_reject(self) -> Reject {
        match self {
            CommitFailure::NotSent => lost_before_commit(),
            CommitFailure::Unknown => Reject { code: "WIRE.COMMIT_UNKNOWN", msg: "커밋 응답을 받지 못함. 상태 조회 필요".into() },
            CommitFailure::Rejected(e) => write_err("커밋", e),
        }
    }
}

/// COMMIT을 한 번 보내고 결과를 분류한다. `on_send`는 COMMIT을 실제로 보내기 직전에 한 번 불린다(결과 불명 표지를 올리는 자리).
/// 연결이 이미 닫혔으면 `on_send`를 부르지 않는다. 보냈다고 표시하면 커밋되지 않은 쓰기가 결과 불명으로 보고된다.
pub async fn send_commit(tx: Transaction<'_>, on_send: impl FnOnce()) -> Result<(), CommitFailure> {
    if tx.client().is_closed() {
        return Err(CommitFailure::NotSent);
    }
    on_send();
    match tx.commit().await {
        Ok(()) => Ok(()),
        Err(e) if connection_failed(&e) => Err(CommitFailure::Unknown),
        Err(e) => Err(CommitFailure::Rejected(e)),
    }
}

fn write_err(stage: &str, e: tokio_postgres::Error) -> Reject {
    match e.code().map(|c| c.code()) {
        Some("23505") => Reject { code: "WIRE.ALREADY_EXISTS", msg: format!("{stage}: 유일 제약 위반") },
        Some("23P01") => Reject { code: "WIRE.INVARIANT_VIOLATED", msg: format!("{stage}: 불변식 위반") },
        // 정의의 값 제약(길이·범위·필수)은 호출자 값 오류다. 내부 오류로 보고하지 않는다.
        Some("23514") => Reject { code: "WIRE.BAD_VALUE", msg: format!("{stage}: 값 제약 위반") },
        Some("23502") => Reject { code: "WIRE.BAD_VALUE", msg: format!("{stage}: 필수 값 누락") },
        Some("22003") => Reject { code: "WIRE.BAD_VALUE", msg: format!("{stage}: 숫자 범위 초과") },
        _ => db_err(stage, e),
    }
}

/// 서버가 정의한 전이 효과. 값은 바뀐 대상 행에서 계산한다(W2).
async fn effects(tx: &Transaction<'_>, facts: &Value, res: &str, tr: &str, change: &[EntityId], caller: &Caller) -> Result<(), Reject> {
    if change.is_empty() {
        return Ok(());
    }
    for e in facts["resources"][res]["transitions"][tr]["effects"].as_array().into_iter().flatten() {
        let mut cx = Ctx::new(facts, caller.actor_id, &caller.now);
        let env = Env { this: Some(("t".into(), res.to_string())), ..Default::default() };
        if let Some(target) = e["update"].as_str() {
            // 대상 행마다 조건에 맞는 다른 행을 정확히 하나 바꾼다. 못 찾거나 여럿이면 전체 취소.
            let mut conds = vec![];
            for (f, v) in e["match"].as_object().unwrap() {
                conds.push(format!("u.{} = {}", column(facts, target, f).unwrap(), cx.value(v, &env).map_err(internal)?));
            }
            let mut sets = vec![];
            for (f, v) in e["values"].as_object().unwrap() {
                sets.push(format!("{} = {}", column(facts, target, f).unwrap(), cx.value(v, &env).map_err(internal)?));
            }
            let ip = cx.params.bind(Some(id_array(change)), id_array_cast(facts, res));
            let children = if let Some(max) = e.get("maxRows") {
                let max = max.as_i64().filter(|n| (1..=1000).contains(n)).ok_or_else(|| internal("many update 상한 오류"))?;
                // 부모 잠금 뒤 새 RC 스냅샷으로 FK 삽입을 관측하고 자식을 ID 순으로 잠근다.
                let select = format!(
                    "SELECT u.id FROM {} u JOIN {} t ON {} WHERE t.id = ANY({ip}) ORDER BY u.id LIMIT {} FOR UPDATE OF u",
                    table(target),
                    table(res),
                    conds.join(" AND "),
                    max + 1
                );
                let rows = typed_query(tx, &select, &cx.params.values).await.map_err(|err| write_err("전이 효과", err))?;
                if rows.len() as i64 > max {
                    return rej("WIRE.EFFECT_TARGET_MISSING", "선언한 전이 효과 범위를 처리할 수 없음");
                }
                let ids: Vec<EntityId> = rows.iter().map(|row| row.get(0)).collect();
                if ids.is_empty() {
                    continue;
                }
                let bounded = cx.params.bind(Some(id_array(&ids)), id_array_cast(facts, res));
                conds.push(format!("u.id = ANY({bounded})"));
                Some(ids.len())
            } else {
                None
            };
            let sql = format!(
                "UPDATE {} u SET {} FROM {} t WHERE t.id = ANY({ip}) AND {} RETURNING t.id",
                table(target),
                sets.join(", "),
                table(res),
                conds.join(" AND ")
            );
            let params = cx.params.values.clone();
            let mut got: Vec<EntityId> =
                typed_query(tx, sql.as_str(), &params).await.map_err(|e| write_err("전이 효과", e))?.iter().map(|r| r.get(0)).collect();
            got.sort();
            if children.is_some_and(|count| count != got.len()) || (children.is_none() && got != change) {
                return rej("WIRE.EFFECT_TARGET_MISSING", format!("`{target}` 갱신 대상이 대상 행마다 하나가 아님"));
            }
            continue;
        }
        let sql = if let Some(target) = e["create"].as_str() {
            let (mut cols, mut vals) = (vec![], vec![]);
            for (f, v) in e["values"].as_object().unwrap() {
                cols.push(column(facts, target, f).unwrap());
                vals.push(cx.value(v, &env).map_err(internal)?);
            }
            let ip = cx.params.bind(Some(id_array(change)), id_array_cast(facts, res));
            format!("INSERT INTO {} ({}) SELECT {} FROM {} t WHERE t.id = ANY({ip})", table(target), cols.join(", "), vals.join(", "), table(res))
        } else {
            let n = &e["notify"];
            let to = cx.value(&n["to"], &env).map_err(internal)?;
            let mixed = facts["resources"].as_object().unwrap().values().any(|r| r["idKind"] == "uuid");
            let to = if mixed { format!("({to})::text") } else { to };
            let source_id = if mixed { "t.id::text" } else { "t.id" };
            let topic = cx.params.bind(Some(n["topic"].as_str().unwrap().to_string()), "text");
            let src = cx.params.bind(Some(res.to_string()), "text");
            let ip = cx.params.bind(Some(id_array(change)), id_array_cast(facts, res));
            // 알림은 같은 트랜잭션의 outbox 행이다. 승인이 rollback되면 알림 기록도 없다.
            format!(
                "INSERT INTO {}.aip_outbox (topic, recipient_id, source, source_id) SELECT {topic}, {to}, {src}, {source_id} FROM {} t WHERE t.id = ANY({ip})",
                sqlgen::schema(),
                table(res)
            )
        };
        let params = cx.params.values.clone();
        typed_query(tx, sql.as_str(), &params).await.map_err(|e| write_err("전이 효과", e))?;
    }
    Ok(())
}

/// 생성할 행의 값 출처. 호출자 리터럴(W0) 또는 서버가 읽은 대상 행의 경로(W1).
pub enum Src {
    Lit(String, &'static str),
    Null,
    Item { res: String, id: EntityId, path: Value },
}

/// 호출자가 준 Ref 값은 호출자가 읽을 수 있는 행이어야 한다(창시자 결정 2026-10-13, 테넌트 격리 T2-A).
/// 그렇지 않으면 `allow`가 참조 행을 보지 않는 한 다른 테넌트의 행에 자기 행을 끼워 넣을 수 있다.
/// 예외는 actor 자기 행이다. actor resource에는 `rows read`가 없는 정의가 흔하고 `author = actor`가 정상 사용이다.
/// 안 보이는 행은 없는 행과 같은 MISSING_TARGET이라 존재 여부도 새지 않는다.
async fn ensure_refs_readable(
    tx: &Transaction<'_>,
    facts: &Value,
    target: &str,
    values: &[(String, Src)],
    caller: &Caller,
    knobs: &Knobs,
) -> Result<(), Reject> {
    for (field, src) in values {
        let Src::Lit(id, _) = src else { continue };
        let ty = facts["resources"][target]["fields"][field.as_str()]["ty"].as_str().unwrap_or("");
        let Some(referenced) = ty.trim_end_matches('?').strip_prefix("Ref<").and_then(|t| t.strip_suffix('>')) else { continue };
        let id = parse_entity_id(facts, referenced, &json!(id), IdWire::DecimalString)?;
        if facts["actor"].as_str() == Some(referenced) && caller.actor_id.map(EntityId::Number) == Some(id) {
            continue;
        }
        if facts["actor"].as_str() == Some(referenced) && id_kind(facts, referenced) == "uuid" {
            if let Some(handle) = caller.actor_id {
                let lock = if knobs.skip_policy_lock { "" } else { " FOR SHARE OF i,t" };
                let sql = format!(
                    "SELECT 1 FROM {}.aip_actor_identities i JOIN {} t ON t.id=i.resource_id WHERE i.id=$1 AND i.resource_id=$2::text::uuid{lock}",
                    sqlgen::schema(),
                    table(referenced)
                );
                if tx.query_opt(&sql, &[&handle, &id.to_string()]).await.map_err(|e| write_err("UUID actor 참조 확인", e))?.is_some() {
                    continue;
                }
            }
        }
        let mut cx = Ctx::new(facts, caller.actor_id, &caller.now);
        cx.lock_reads = !knobs.skip_policy_lock;
        let env = Env { this: Some(("t".into(), referenced.to_string())), ..Default::default() };
        let vis = cx.cond(&facts["resources"][referenced]["rowRead"], &env).map_err(internal)?;
        let p = cx.params.bind(Some(id.to_string()), id_kind(facts, referenced));
        let sql = format!("SELECT 1 FROM {} t WHERE t.id = {p} AND {vis}", table(referenced));
        let params = cx.params.values.clone();
        if typed_query(tx, sql.as_str(), &params).await.map_err(|e| write_err("참조 행 확인", e))?.is_empty() {
            return rej("WIRE.MISSING_TARGET", format!("`{target}.{field}`가 가리키는 `{referenced}` 행을 찾을 수 없음"));
        }
    }
    Ok(())
}

/// expose create의 allow를 새 행에 대해 평가하며 한 행씩 넣는다. allow 거짓이면 FORBIDDEN.
async fn create_row(
    tx: &Transaction<'_>,
    facts: &Value,
    target: &str,
    values: &[(String, Src)],
    caller: &Caller,
    knobs: &Knobs,
) -> Result<Option<EntityId>, Reject> {
    let ec = &facts["resources"][target]["exposeCreate"];
    let mut cx = Ctx::new(facts, caller.actor_id, &caller.now);
    cx.lock_reads = !knobs.skip_policy_lock;
    let (mut cols, mut exprs) = (vec![], vec![]);
    let mut from = String::new();
    for (f, src) in values {
        let col = column(facts, target, f).unwrap();
        let e = match src {
            Src::Lit(v, cast) => cx.params.bind(Some(v.clone()), cast),
            Src::Null => format!("(NULL::{}).{col}", table(target)),
            Src::Item { res, id, path } => {
                if from.is_empty() {
                    let p = cx.params.bind(Some(id.to_string()), id_kind(facts, res));
                    from = format!(" FROM {} s WHERE s.id = {p}", table(res));
                }
                let env = Env { this: Some(("s".into(), res.clone())), ..Default::default() };
                cx.value(path, &env).map_err(internal)?
            }
        };
        exprs.push(format!("{e} AS {col}"));
        cols.push(col);
    }
    // 생략 필드도 allow가 볼 후보 행에 있어야 한다. 테이블 행 타입이 각 NULL의 SQL 타입을 결정한다.
    for (field, _) in facts["resources"][target]["fields"].as_object().unwrap() {
        if field != "id" && !values.iter().any(|(provided, _)| provided == field) {
            let col = column(facts, target, field).unwrap();
            exprs.push(format!("(NULL::{}).{col} AS {col}", table(target)));
        }
    }
    if exprs.is_empty() {
        exprs.push("1".into());
    }
    let venv = Env { this: Some(("v".into(), target.to_string())), ..Default::default() };
    let external = !facts["externalStorage"].is_null();
    let allow = if external { "TRUE".into() } else { cx.cond(&ec["allow"], &venv).map_err(internal)? };
    let insert_cols = if cols.is_empty() { String::new() } else { format!(" ({})", cols.join(", ")) };
    // 비공개 관계 테이블은 `id` 열이 없을 수 있다. 그런 행은 id를 돌려줄 수 없다.
    let has_id = facts["resources"][target]["fields"].get("id").is_some();
    let returning = if has_id { "id" } else { "1" };
    let sql = format!(
        "INSERT INTO {}{insert_cols} SELECT {} FROM (SELECT {}{from}) v WHERE {allow} RETURNING {returning}",
        table(target),
        cols.iter().map(|c| format!("v.{c}")).collect::<Vec<_>>().join(", "),
        exprs.join(", ")
    );
    let params = cx.params.values.clone();
    let rows = typed_query(tx, sql.as_str(), &params).await.map_err(|e| write_err("생성", e))?;
    if rows.len() != 1 {
        return rej("WIRE.FORBIDDEN", format!("`{target}` 생성 권한 없음"));
    }
    let id = has_id.then(|| rows[0].get::<_, EntityId>(0));
    if external {
        let id = id.ok_or_else(|| bad("외부 create에는 단일 기본키 필요"))?;
        let mut policy = Ctx::new(facts, caller.actor_id, &caller.now);
        policy.lock_reads = !knobs.skip_policy_lock;
        let env = Env { this: Some(("t".into(), target.into())), ..Default::default() };
        let allow = policy.cond(&ec["allow"], &env).map_err(internal)?;
        let p = policy.params.bind(Some(id.to_string()), id_kind(facts, target));
        let sql = format!("SELECT 1 FROM {} t WHERE t.id={p} AND {allow}", table(target));
        if typed_query(tx, &sql, &policy.params.values).await.map_err(|e| write_err("생성 후 인가", e))?.is_empty() {
            return rej("WIRE.FORBIDDEN", "DB 기본값 적용 후 생성 권한 없음");
        }
    }
    Ok(id)
}

/// 이 트랜잭션이 `res` 테이블을 바꿨는지 판정한다. `xmin = pg_current_xact_id()`는 인덱스가 없어 전체 스캔이므로(O-25),
/// 먼저 트랜잭션 로컬 통계(`pg_stat_xact_all_tables`)로 변경이 전혀 없는 테이블을 스캔 없이 걸러낸다.
/// 통계는 savepoint에서 롤백된 쓰기도 세므로 "변경 없음"을 단정할 때만 믿고, 변경이 있어 보이면 기존 xmin 검사로 확정한다.
/// `track_counts`가 꺼져 있으면 통계가 쌓이지 않으므로 걸러내지 않는다(검사 누락은 fail-open).
async fn changed_in_transaction(tx: &Transaction<'_>, res: &str) -> Result<bool, tokio_postgres::Error> {
    let table = table(res);
    let touched: bool = tx
        .query_one(
            format!(
                "SELECT current_setting('track_counts') <> 'on' OR coalesce((SELECT n_tup_ins + n_tup_upd + n_tup_del > 0 FROM pg_stat_xact_all_tables WHERE relid = '{table}'::regclass), true)"
            )
            .as_str(),
            &[],
        )
        .await?
        .get(0);
    if !touched {
        return Ok(false);
    }
    Ok(tx.query_one(format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE xmin = pg_current_xact_id()::xid)").as_str(), &[]).await?.get(0))
}

/// `check`는 기존처럼 변경 행의 사후조건이고, locked count는 별도 잠금으로 직렬화한 그룹 제약이다.
/// 일반 `check`의 교차 트랜잭션 근거 행은 잠기지 않는다(R3-03). savepoint 안에서 쓴 행은 변경 검사에서 빠진다.
async fn run_checks(tx: &Transaction<'_>, facts: &Value, caller: &Caller, knobs: &Knobs) -> Result<(), Reject> {
    let limited: Vec<(&str, &Value)> = facts["resources"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, rf)| rf["invariants"].as_object().is_some_and(|invs| invs.values().any(|inv| inv["enforcement"]["kind"] == "lockedCountCheck")))
        .map(|(res, rf)| (res.as_str(), rf))
        .collect();

    if !limited.is_empty() {
        let isolation: String = tx.query_one("SELECT current_setting('transaction_isolation')", &[]).await.map_err(internal)?.get(0);
        // SERIALIZABLE도 직렬성을 보장하므로 허용한다(O-26). 충돌은 40001로 드러나고 서버가 재시도한다.
        if isolation != "read committed" && isolation != "serializable" {
            return rej("WIRE.UNSUPPORTED_ISOLATION", "정원 제약 쓰기는 READ COMMITTED 또는 SERIALIZABLE 트랜잭션이 필요함");
        }
    }

    // 모든 변경 resource를 먼저 잠가 bundle/extension 안에서도 잠금 순서를 고정한다.
    // 실제 정원 집계는 잠금 획득 뒤 별도 SQL에서 실행해 앞선 writer의 커밋을 관찰한다.
    let mut touched = Vec::new();
    for (res, _) in &limited {
        if changed_in_transaction(tx, res).await.map_err(|e| db_err("정원 변경 확인", e))? {
            let lock = format!("aip.capacity:{}:{res}", sqlgen::schema());
            tx.query_one("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))", &[&lock]).await.map_err(|e| db_err("정원 잠금", e))?;
            touched.push(*res);
        }
    }

    for res in touched {
        let rf = &facts["resources"][res];
        for (name, inv) in rf["invariants"].as_object().unwrap() {
            let enf = &inv["enforcement"];
            if enf["kind"] != "lockedCountCheck" {
                continue;
            }
            let per = inv["per"].as_str().ok_or_else(|| internal("정원 그룹 필드 누락"))?;
            let per_col = column(facts, res, per).ok_or_else(|| internal("정원 그룹 열 누락"))?;
            let max = enf["max"].as_i64().filter(|n| *n >= 1).ok_or_else(|| internal("정원 상한 누락"))?;
            let mut cx = Ctx::new(facts, None, "");
            let env = Env { this: Some(("t".into(), res.to_string())), ..Default::default() };
            let cond = cx.cond(&enf["where"], &env).map_err(internal)?;
            let predicate_params = cx.params.values.clone();
            let touched_groups = format!(
                "SELECT DISTINCT t.{per_col}::text FROM {} t WHERE t.xmin = pg_current_xact_id()::xid AND ({cond}) IS TRUE AND t.{per_col} IS NOT NULL",
                table(res)
            );
            let touched_params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = predicate_params.iter().map(|value| value as _).collect();
            let groups: Vec<String> = tx
                .query(touched_groups.as_str(), &touched_params)
                .await
                .map_err(|e| db_err("정원 그룹 조회", e))?
                .iter()
                .map(|row| row.get(0))
                .collect();
            if groups.is_empty() {
                continue;
            }
            let groups_idx = predicate_params.len() + 1;
            let max_idx = groups_idx + 1;
            let sql = format!(
                "SELECT t.{per_col} FROM {} t WHERE t.{per_col} = ANY(${groups_idx}::text::{group_cast}[]) AND ({cond}) IS TRUE GROUP BY t.{per_col} HAVING count(*) > ${max_idx} LIMIT 1",
                table(res), group_cast=spike_v2_read::id_wire::id_cast(facts,rf["fields"][per]["ty"].as_str().unwrap()),
            );
            let groups = format!("{{{}}}", groups.join(","));
            let mut query_params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = predicate_params.iter().map(|value| value as _).collect();
            query_params.push(&groups);
            query_params.push(&max);
            if !tx.query(&sql, &query_params).await.map_err(|e| db_err("정원 집계", e))?.is_empty() {
                return rej("WIRE.INVARIANT_VIOLATED", format!("`{res}.{name}` 정원 제약 위반"));
            }
        }
    }

    for (res, rf) in facts["resources"].as_object().unwrap() {
        if rf["checks"].as_object().is_none_or(|checks| checks.is_empty()) {
            continue;
        }
        if !changed_in_transaction(tx, res).await.map_err(|e| db_err("커밋 검사 대상 확인", e))? {
            continue;
        }
        for (name, cond) in rf["checks"].as_object().into_iter().flatten() {
            let mut cx = Ctx::new(facts, caller.actor_id, &caller.now);
            // 검사 근거가 된 다른 행(예: 승인된 Apply)을 커밋까지 공유 잠금한다. 검사 뒤 커밋 전 철회를 막는다(R3-03).
            cx.lock_reads = !knobs.skip_policy_lock;
            let env = Env { this: Some(("c".into(), res.clone())), ..Default::default() };
            let c = cx.cond(cond, &env).map_err(internal)?;
            let sql = format!("SELECT count(*) FROM {} c WHERE c.xmin = pg_current_xact_id()::xid AND ({c}) IS NOT TRUE", table(res));
            let params = cx.params.values.clone();
            let n: i64 = typed_query(tx, sql.as_str(), &params).await.map_err(|e| db_err("커밋 검사", e))?[0].get(0);
            if n > 0 {
                return rej("WIRE.CHECK_FAILED", format!("`{res}.{name}`를 만족하지 않는 행 {n}개"));
            }
        }
    }
    Ok(())
}

/// 다른 행을 읽는 정책에 의존하는 쓰기는 SERIALIZABLE로 실행한다(O-26). 컴파일 단계가 facts에 `isolation`을 표시한다.
pub fn isolation_sql(serializable: bool) -> &'static str {
    if serializable {
        "SET TRANSACTION ISOLATION LEVEL SERIALIZABLE"
    } else {
        "SET TRANSACTION ISOLATION LEVEL READ COMMITTED"
    }
}

fn is_serializable(marker: &Value) -> bool {
    marker == "serializable"
}

/// 전이 하나를 SERIALIZABLE로 실행해야 하는지. 요청 형식 오류는 여기서 판단하지 않고 false로 둔다(이후 검사가 거절한다).
pub fn apply_needs_serializable(facts: &Value, req: &Value) -> bool {
    let name = req["apply"].as_str().unwrap_or("");
    name.split_once('.').is_some_and(|(res, tr)| is_serializable(&facts["resources"][res]["transitions"][tr]["isolation"]))
}

/// 묶음의 연산 중 하나라도 SERIALIZABLE이 필요하면 묶음 전체가 그렇다(한 트랜잭션이다).
pub fn bundle_needs_serializable(facts: &Value, req: &Value) -> bool {
    req["atomic"].as_array().into_iter().flatten().any(|op| {
        if op.get("apply").is_some() {
            apply_needs_serializable(facts, op)
        } else {
            op["create"].as_str().is_some_and(|res| is_serializable(&facts["resources"][res]["exposeCreate"]["isolation"]))
        }
    })
}

/// `/apply` 요청 하나(전이, 묶음, 조합)가 SERIALIZABLE을 요구하는지.
pub fn write_needs_serializable(facts: &Value, req: &Value) -> bool {
    if req.get("atomic").is_some() {
        bundle_needs_serializable(facts, req)
    } else if req.get("compose").is_some() {
        compose_needs_serializable(facts, req)
    } else {
        apply_needs_serializable(facts, req)
    }
}

pub fn compose_needs_serializable(facts: &Value, req: &Value) -> bool {
    req["compose"].as_str().is_some_and(|res| is_serializable(&facts["resources"][res]["exposeCompose"]["isolation"]))
}

fn begin_failure(e: tokio_postgres::Error) -> Reject {
    if connection_failed(&e) {
        lost_before_commit()
    } else {
        internal(e)
    }
}

async fn begin<'a>(db: &'a mut Client, serializable: bool) -> Result<Transaction<'a>, Reject> {
    let tx = db.transaction().await.map_err(begin_failure)?;
    tx.batch_execute(&format!("{}; SET LOCAL statement_timeout = '{WRITE_DEADLINE_MS}ms'; SET LOCAL TimeZone = 'UTC'", isolation_sql(serializable)))
        .await
        .map_err(begin_failure)?;
    Ok(tx)
}

pub fn exposed<'a>(facts: &'a Value, name: &str) -> Result<(&'a str, &'a str, &'a Value), Reject> {
    let (res, tr) = name.split_once('.').ok_or(bad("apply는 `Resource.transition`"))?;
    let ex = &facts["resources"][res]["exposeApply"][tr];
    if ex.is_null() {
        return rej("WIRE.NOT_EXPOSED", format!("`{name}`는 공개된 쓰기 동작이 아님"));
    }
    let res = facts["resources"].as_object().unwrap().keys().find(|k| *k == res).unwrap();
    let tr = facts["resources"][res.as_str()]["transitions"].as_object().unwrap().keys().find(|k| *k == tr).unwrap();
    Ok((res, tr, ex))
}

#[derive(Default)]
pub struct ApplyInput {
    pub values: Vec<(String, Src)>,
    pub expected: Vec<(String, Src)>,
}

/// 입력 값은 SQL 식이 아니다. 공통 scalar/Id wire 검증을 거친 바인드 값만 받는다.
pub fn apply_input(facts: &Value, res: &str, tr: &str, request: &Map<String, Value>, wire: IdWire) -> Result<ApplyInput, Reject> {
    let declaration = &facts["resources"][res]["transitions"][tr];
    let mut input = ApplyInput::default();
    for (key, declared, dest) in [("values", "patch", &mut input.values), ("expect", "expect", &mut input.expected)] {
        let allowed: Vec<&str> = declaration[declared].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        if allowed.is_empty() {
            if request.contains_key(key) {
                return rej("WIRE.UNKNOWN_KEY", format!("`{res}.{tr}`에는 {key} 선언이 없음"));
            }
            continue;
        }
        let values = request.get(key).and_then(Value::as_object).ok_or_else(|| bad(format!("{key} 객체 필요")))?;
        if values.is_empty() || (key == "expect" && values.len() != allowed.len()) {
            return rej("WIRE.BAD_VALUE", format!("{key}는 비어 있을 수 없고 expect의 선언 키는 모두 필요"));
        }
        for (field, value) in values {
            if !allowed.contains(&field.as_str()) {
                return rej("WIRE.FIELD_NOT_EXPOSED", format!("`{res}.{field}`는 {key} 계약에 없음"));
            }
            dest.push((field.clone(), literal(facts, res, field, value, wire)?));
        }
    }
    Ok(input)
}

fn input_sql(cx: &mut Ctx<'_>, res: &str, field: &str, src: &Src) -> Result<String, Reject> {
    match src {
        Src::Lit(value, cast) => Ok(cx.params.bind(Some(value.clone()), cast)),
        // 테이블 행 타입에서 NULL의 SQL 타입을 얻는다. nullable scalar·Ref 모두 같은 경로다.
        Src::Null => Ok(format!("(NULL::{}).{}", table(res), column(cx.facts, res, field).unwrap())),
        Src::Item { .. } => Err(internal("apply input must be a literal")),
    }
}

async fn apply_op(
    tx: &Transaction<'_>,
    facts: &Value,
    o: &serde_json::Map<String, Value>,
    caller: &Caller,
    knobs: &Knobs,
    wire: IdWire,
) -> Result<Applied, Reject> {
    keys(o, &["apply", "target", "values", "expect"], "apply 요청")?;
    let (res, tr, ex) = exposed(facts, o.get("apply").and_then(Value::as_str).unwrap_or(""))?;
    let input = apply_input(facts, res, tr, o, wire)?;
    let target = parse_target(facts, res, ex, o.get("target").unwrap_or(&Value::Null), wire)?;
    let j = judge(tx, facts, res, tr, &target, ex["bulkMaxRows"].as_i64().unwrap_or(1), &ex["sameScope"], caller, knobs).await?;
    let decided = Caller { actor_id: caller.actor_id, now: j.now.clone() };
    ensure_refs_readable(tx, facts, res, &input.values, &decided, knobs).await?;
    update(tx, facts, res, tr, &j.change, &decided, knobs, Some(&input)).await?;
    effects(tx, facts, res, tr, &j.change, &decided).await?;
    Ok(Applied { changed: j.change, unchanged: j.unchanged })
}

async fn apply_inner(
    db: &mut Client,
    facts: &Value,
    req: &Value,
    caller: &Caller,
    knobs: &Knobs,
    ctl: Ctl,
    serializable: bool,
) -> Result<Applied, Reject> {
    let o = req.as_object().ok_or(bad("요청은 객체"))?;
    let tx = begin(db, serializable).await?;
    let a = apply_op(&tx, facts, o, caller, knobs, IdWire::Legacy).await.map_err(|e| before_commit_rejection(&tx, e))?;
    run_checks(&tx, facts, caller, knobs).await.map_err(|e| before_commit_rejection(&tx, e))?;
    commit(tx, &ctl, knobs).await?;
    Ok(a)
}

pub fn literal(facts: &Value, res: &str, field: &str, v: &Value, wire: IdWire) -> Result<Src, Reject> {
    let ty = facts["resources"][res]["fields"][field]["ty"]
        .as_str()
        .ok_or(Reject { code: "WIRE.UNKNOWN_FIELD", msg: format!("`{res}.{field}` 없음") })?;
    let base = ty.trim_end_matches('?');
    if v.is_null() {
        return if ty.ends_with('?') { Ok(Src::Null) } else { rej("WIRE.BAD_VALUE", format!("`{field}`는 null일 수 없음")) };
    }
    if base.starts_with("Ref<") || base.starts_with("Id<") {
        // Id wire(숫자·십진 문자열)에 맞는 표기만 받는다. 전이 대상 id와 같은 규칙이다.
        let (id, cast) = parse_typed_id(facts, base, v, wire)?;
        return Ok(Src::Lit(id, cast));
    }
    let (s, cast) = spike_v2_read::scalar::parse(facts, base, v)?;
    let range = &facts["resources"][res]["fields"][field]["range"];
    spike_v2_read::scalar::validate_range(&json!([field, base, range]), v, "WIRE.BAD_VALUE")?;
    Ok(Src::Lit(s, cast))
}

#[derive(Debug, PartialEq)]
pub struct BundleResult {
    pub applied: Vec<Applied>,
    pub created: usize,
    /// 만든 행의 (resource, id). 호출자가 만든 행을 곧바로 가리킬 수 있게 한다. `id` 열이 없는 resource는 None이다.
    pub created_rows: Vec<(String, Option<EntityId>)>,
}

pub use spike_v2_read::write_limits::{BUNDLE_MAX_OPS, BUNDLE_MAX_ROWS, COMPOSE_MAX_STEPS};

/// W0: 이미 공개된 동작(전이 apply, expose create)을 한 트랜잭션으로 묶는다. 연산 사이 값 전달은 없다.
pub async fn bundle(db: &mut Client, facts: &Value, req: &Value, caller: &Caller, knobs: &Knobs) -> Result<BundleResult, Reject> {
    let serializable = bundle_needs_serializable(facts, req);
    with_deadline(|ctl| async move {
        let mut attempt = 0;
        let retry_knobs = Knobs { pause_after_lock_ms: 0, pause_before_commit_ms: 0, ..knobs.clone() };
        loop {
            match bundle_once(&mut *db, facts, req, caller, if attempt == 0 { knobs } else { &retry_knobs }, ctl.clone(), serializable).await {
                Err(e) if serializable && is_retryable(&e) && attempt < SERIALIZATION_RETRIES => {
                    attempt += 1;
                    retry_backoff(attempt).await;
                }
                other => return other,
            }
        }
    })
    .await
}

async fn bundle_once(
    db: &mut Client,
    facts: &Value,
    req: &Value,
    caller: &Caller,
    knobs: &Knobs,
    ctl: Ctl,
    serializable: bool,
) -> Result<BundleResult, Reject> {
    let ops = bundle_ops(req)?;
    let tx = begin(db, serializable).await?;
    let out = bundle_run(&tx, facts, ops, caller, knobs, IdWire::Legacy).await.map_err(|e| before_commit_rejection(&tx, e))?;
    run_checks(&tx, facts, caller, knobs).await.map_err(|e| before_commit_rejection(&tx, e))?;
    commit(tx, &ctl, knobs).await?;
    Ok(out)
}

/// 요청 형식과 크기 상한을 트랜잭션 전에 검사한다.
pub fn bundle_ops(req: &Value) -> Result<&Vec<Value>, Reject> {
    let o = req.as_object().ok_or(bad("요청은 객체"))?;
    keys(o, &["atomic"], "묶음 요청")?;
    let ops = o.get("atomic").and_then(Value::as_array).ok_or(bad("atomic 배열 필요"))?;
    if ops.is_empty() {
        return Err(bad("atomic에 연산이 하나는 있어야 함"));
    }
    if ops.len() > BUNDLE_MAX_OPS {
        return rej("WIRE.BULK_LIMIT", format!("묶음의 연산 {}개 > 상한 {BUNDLE_MAX_OPS}", ops.len()));
    }
    Ok(ops)
}

/// 쓰기 확장·HTTP 쓰기용: 호출자가 가진 트랜잭션 안에서 묶음을 실행한다. 커밋 시점 검사(`checks_in`)와 커밋은 호출자가 한다.
pub async fn bundle_in(
    tx: &Transaction<'_>,
    facts: &Value,
    req: &Value,
    caller: &Caller,
    knobs: &Knobs,
    wire: IdWire,
) -> Result<BundleResult, Reject> {
    bundle_run(tx, facts, bundle_ops(req)?, caller, knobs, wire).await
}

async fn bundle_run(
    tx: &Transaction<'_>,
    facts: &Value,
    ops: &[Value],
    caller: &Caller,
    knobs: &Knobs,
    wire: IdWire,
) -> Result<BundleResult, Reject> {
    // 대상 행 수 상한은 DB에 닿기 전에 본다. where 대상은 실제 수를 모르므로 선언한 상한으로 센다.
    let mut rows = 0usize;
    for op in ops {
        let m = op.as_object().ok_or(bad("연산은 객체"))?;
        rows += if m.contains_key("apply") {
            let name = m.get("apply").and_then(Value::as_str).unwrap_or("");
            let (res, tr) = name.split_once('.').unwrap_or(("", ""));
            let ex = &facts["resources"][res]["exposeApply"][tr];
            match m.get("target").and_then(|t| t.get("ids")).and_then(Value::as_array) {
                Some(ids) => ids.len(),
                None => ex["bulkMaxRows"].as_u64().unwrap_or(1) as usize,
            }
        } else {
            1
        };
    }
    if rows > BUNDLE_MAX_ROWS {
        return rej("WIRE.BULK_LIMIT", format!("묶음의 대상 행 {rows}개 > 상한 {BUNDLE_MAX_ROWS}"));
    }
    let mut out = BundleResult { applied: vec![], created: 0, created_rows: vec![] };
    for op in ops {
        let m = op.as_object().ok_or(bad("연산은 객체"))?;
        if m.contains_key("apply") {
            out.applied.push(apply_op(tx, facts, m, caller, knobs, wire).await?);
        } else {
            keys(m, &["create", "values"], "create 연산")?;
            let res = m.get("create").and_then(Value::as_str).unwrap_or("");
            let ec = &facts["resources"][res]["exposeCreate"];
            if ec.is_null() {
                return rej("WIRE.NOT_EXPOSED", format!("`{res}` 생성은 공개되지 않음"));
            }
            let res = facts["resources"].as_object().unwrap().keys().find(|k| *k == res).unwrap().as_str();
            let vals = m.get("values").and_then(Value::as_object).ok_or(bad("values 객체 필요"))?;
            let allowed: Vec<&str> = ec["fields"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
            let mut row = vec![];
            for (f, v) in vals {
                if !allowed.contains(&f.as_str()) {
                    return rej("WIRE.FIELD_NOT_EXPOSED", format!("`{res}.{f}`는 생성 계약에 없음"));
                }
                row.push((f.clone(), literal(facts, res, f, v, wire)?));
            }
            ensure_refs_readable(tx, facts, res, &row, caller, knobs).await?;
            let id = create_row(tx, facts, res, &row, caller, knobs).await?;
            out.created += 1;
            out.created_rows.push((res.to_string(), id));
        }
    }
    Ok(out)
}

fn path_text(p: &Value) -> Option<String> {
    // facts의 출처가 경로가 아니면(의미 검사가 막지만) 비교 대상에서 뺀다. panic하지 않는다(R3-06).
    Some(p["path"]["segs"].as_array()?.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("."))
}

/// W1: 호출자가 대상 목록과 항목별 단계를 조합한다. 허용 단계·값 출처는 서버의 expose compose가 정한다.
pub async fn compose(db: &mut Client, facts: &Value, req: &Value, caller: &Caller, knobs: &Knobs) -> Result<BundleResult, Reject> {
    let serializable = compose_needs_serializable(facts, req);
    with_deadline(|ctl| async move {
        let mut attempt = 0;
        let retry_knobs = Knobs { pause_after_lock_ms: 0, pause_before_commit_ms: 0, ..knobs.clone() };
        loop {
            match compose_once(&mut *db, facts, req, caller, if attempt == 0 { knobs } else { &retry_knobs }, ctl.clone(), serializable).await {
                Err(e) if serializable && is_retryable(&e) && attempt < SERIALIZATION_RETRIES => {
                    attempt += 1;
                    retry_backoff(attempt).await;
                }
                other => return other,
            }
        }
    })
    .await
}

/// 조합 요청을 트랜잭션 전에 해석하고 허용 단계·값 출처·크기를 모두 검사한다.
fn compose_plan<'a>(facts: &'a Value, req: &'a Value, wire: IdWire) -> Result<ComposePlan<'a>, Reject> {
    let o = req.as_object().ok_or(bad("요청은 객체"))?;
    keys(o, &["compose", "targets", "steps"], "조합 요청")?;
    let res = o.get("compose").and_then(Value::as_str).unwrap_or("");
    let cp = &facts["resources"][res]["exposeCompose"];
    if cp.is_null() {
        return rej("WIRE.NOT_EXPOSED", format!("`{res}` 조합은 공개되지 않음"));
    }
    let res = facts["resources"].as_object().unwrap().keys().find(|k| *k == res).unwrap().as_str();
    let bulk = cp["bulkMaxRows"].as_i64().unwrap_or(1);
    let tg = o.get("targets").and_then(Value::as_object).ok_or(bad("targets 객체 필요"))?;
    keys(tg, &["ids"], "targets")?;
    let ids = parse_ids(facts, res, tg.get("ids").unwrap_or(&Value::Null), bulk, wire)?;
    let steps = o.get("steps").and_then(Value::as_array).ok_or(bad("steps 배열 필요"))?;
    if steps.is_empty() {
        return Err(bad("steps에 단계가 하나는 있어야 함"));
    }
    if steps.len() > COMPOSE_MAX_STEPS {
        return rej("WIRE.BULK_LIMIT", format!("조합의 단계 {}개 > 상한 {COMPOSE_MAX_STEPS}", steps.len()));
    }
    // 단계마다 대상 행 전체를 건드리므로 행 수는 대상 수 × 단계 수다.
    if ids.len() * steps.len() > BUNDLE_MAX_ROWS {
        return rej("WIRE.BULK_LIMIT", format!("조합의 대상 행 {}개 > 상한 {BUNDLE_MAX_ROWS}", ids.len() * steps.len()));
    }
    let mut plan = vec![];
    for st in steps {
        let m = st.as_object().ok_or(bad("단계는 객체"))?;
        if let Some(t) = m.get("transition").and_then(Value::as_str) {
            keys(m, &["transition", "on"], "transition 단계")?;
            let on_self = match m.get("on").and_then(Value::as_str) {
                None | Some("targets") => false,
                Some("self") if !cp["selfRow"].is_null() => true,
                _ => return rej("WIRE.STEP_NOT_ALLOWED", "on은 targets 또는 계약에 selfRow가 있을 때 self"),
            };
            let allowed = cp["transitions"].as_array().unwrap();
            let Some(t) = allowed.iter().filter_map(Value::as_str).find(|x| *x == t) else {
                return rej("WIRE.STEP_NOT_ALLOWED", format!("전이 `{t}`는 조합 계약에 없음"));
            };
            plan.push(Step::Tr(t, on_self));
        } else {
            keys(m, &["create", "values"], "create 단계")?;
            let target = m.get("create").and_then(Value::as_str).unwrap_or("");
            let decl = &cp["creates"][target];
            if decl.is_null() {
                return rej("WIRE.STEP_NOT_ALLOWED", format!("`{target}` 생성은 조합 계약에 없음"));
            }
            let target = cp["creates"].as_object().unwrap().keys().find(|k| *k == target).unwrap().as_str();
            let allowed_fields = &facts["resources"][target]["exposeCreate"]["fields"];
            let mut vals = vec![];
            for (f, v) in m.get("values").and_then(Value::as_object).ok_or(bad("values 객체 필요"))? {
                if !allowed_fields.as_array().unwrap().iter().any(|x| x == f) {
                    return rej("WIRE.FIELD_NOT_EXPOSED", format!("`{target}.{f}`는 생성 계약에 없음"));
                }
                let vo = v.as_object().ok_or(bad("값은 {item} 또는 {const}"))?;
                let fty = facts["resources"][target]["fields"][f]["ty"].as_str().unwrap();
                if let Some(p) = vo.get("item").and_then(Value::as_str) {
                    // 값은 서버가 선언한 출처 경로에서만 온다. 호출자가 임의 경로로 숨은 값을 옮기지 못하게 한다.
                    let Some(src) = decl["sources"].as_array().unwrap().iter().find(|s| path_text(s).as_deref() == Some(p)) else {
                        return rej("WIRE.VALUE_SOURCE_NOT_ALLOWED", format!("출처 `{p}`는 조합 계약에 없음"));
                    };
                    let sty = src["ty"].as_str().unwrap();
                    if sty.trim_end_matches('?') != fty.trim_end_matches('?') || (sty.ends_with('?') && !fty.ends_with('?')) {
                        return rej("WIRE.TYPE_MISMATCH", format!("`{p}`({sty})를 `{target}.{f}`({fty})에 넣을 수 없음"));
                    }
                    vals.push((f.clone(), Value::Null, Some(src.clone())));
                } else if let Some(c) = vo.get("const") {
                    // 참조 필드는 상수로 받지 않는다. 대상 행에서 온 값만 연결할 수 있다.
                    if fty.starts_with("Ref<") || fty.starts_with("Id<") {
                        return rej("WIRE.VALUE_SOURCE_NOT_ALLOWED", format!("참조 필드 `{f}`는 상수로 줄 수 없음"));
                    }
                    vals.push((f.clone(), c.clone(), None));
                } else {
                    return Err(bad("값은 {item} 또는 {const}"));
                }
            }
            plan.push(Step::Create(target, vals));
        }
    }
    Ok(ComposePlan { res, cp, bulk, ids, steps: plan })
}

/// 해석한 조합 요청.
struct ComposePlan<'a> {
    res: &'a str,
    cp: &'a Value,
    bulk: i64,
    ids: Vec<EntityId>,
    steps: Vec<Step<'a>>,
}

enum Step<'a> {
    Tr(&'a str, bool),
    Create(&'a str, Vec<(String, Value, Option<Value>)>),
}

async fn compose_once(
    db: &mut Client,
    facts: &Value,
    req: &Value,
    caller: &Caller,
    knobs: &Knobs,
    ctl: Ctl,
    serializable: bool,
) -> Result<BundleResult, Reject> {
    let plan = compose_plan(facts, req, IdWire::Legacy)?;
    let tx = begin(&mut *db, serializable).await?;
    let out = compose_exec(&tx, facts, &plan, caller, knobs, IdWire::Legacy).await.map_err(|e| before_commit_rejection(&tx, e))?;
    run_checks(&tx, facts, caller, knobs).await.map_err(|e| before_commit_rejection(&tx, e))?;
    commit(tx, &ctl, knobs).await?;
    Ok(out)
}

/// 쓰기 확장·HTTP 쓰기용: 호출자가 가진 트랜잭션 안에서 조합을 실행한다. 커밋 시점 검사와 커밋은 호출자가 한다.
pub async fn compose_in(
    tx: &Transaction<'_>,
    facts: &Value,
    req: &Value,
    caller: &Caller,
    knobs: &Knobs,
    wire: IdWire,
) -> Result<BundleResult, Reject> {
    let plan = compose_plan(facts, req, wire)?;
    compose_exec(tx, facts, &plan, caller, knobs, wire).await
}

async fn compose_exec(
    tx: &Transaction<'_>,
    facts: &Value,
    plan: &ComposePlan<'_>,
    caller: &Caller,
    knobs: &Knobs,
    wire: IdWire,
) -> Result<BundleResult, Reject> {
    let ComposePlan { res, cp, bulk, ids, steps: _ } = plan;
    let ids: &Vec<EntityId> = ids;
    let (res, bulk) = (*res, *bulk);
    let cp: &Value = cp;
    // 모든 대상을 먼저 잠그고 가시성을 확인한다. 같은 범위 검사는 단계별 권한 판정 뒤에 한다(R3-01).
    let scopes: Vec<Option<String>>;
    {
        let rf = &facts["resources"][res];
        let mut cx = Ctx::new(facts, caller.actor_id, &caller.now);
        cx.lock_reads = !knobs.skip_policy_lock;
        let env = Env { this: Some(("t".into(), res.to_string())), ..Default::default() };
        let vis = cx.cond(&rf["rowRead"], &env).map_err(internal)?;
        let scope =
            if cp["sameScope"].is_null() { "NULL::text".into() } else { format!("({})::text", cx.value(&cp["sameScope"], &env).map_err(internal)?) };
        let p = cx.params.bind(Some(id_array(ids)), id_array_cast(facts, res));
        let sql = format!("SELECT t.id, {scope} FROM {} t WHERE t.id = ANY({p}) AND {vis} ORDER BY t.id FOR UPDATE OF t", table(res));
        let params = cx.params.values.clone();
        let rows = typed_query(tx, sql.as_str(), &params).await.map_err(|e| db_err("대상 조회", e))?;
        if rows.len() != ids.len() {
            return rej("WIRE.MISSING_TARGET", format!("대상 {}개를 찾을 수 없음", ids.len() - rows.len()));
        }
        scopes = rows.iter().map(|r| r.get(1)).collect();
    }
    let mut out = BundleResult { applied: vec![], created: 0, created_rows: vec![] };
    for st in &plan.steps {
        match st {
            Step::Tr(t, on_self) => {
                let step_ids = if *on_self {
                    // 자기 행: actor를 가리키고 대상과 같은 범위의 행 하나. 없으면 대상 없음과 같게.
                    let sr = &cp["selfRow"];
                    let a_col = column(facts, res, sr["actorField"].as_str().unwrap()).unwrap();
                    let b_col = column(facts, res, sr["by"].as_str().unwrap()).unwrap();
                    let mut cx = Ctx::new(facts, caller.actor_id, &caller.now);
                    let pa = cx.params.bind(caller.actor_id.map(|x| x.to_string()), "bigint");
                    let pb = cx.params.bind(scopes[0].clone(), "text");
                    let sql = format!("SELECT t.id FROM {} t WHERE t.{a_col} = {pa} AND t.{b_col}::text = {pb}", table(res));
                    let params = cx.params.values.clone();
                    let rows = typed_query(tx, sql.as_str(), &params).await.map_err(|e| db_err("자기 행 조회", e))?;
                    if rows.len() != 1 {
                        return rej("WIRE.MISSING_TARGET", "같은 범위의 자기 행을 찾을 수 없음");
                    }
                    vec![rows[0].get::<_, EntityId>(0)]
                } else {
                    ids.clone()
                };
                let j = judge(tx, facts, res, t, &TargetSpec::Ids(step_ids), bulk, &Value::Null, caller, knobs).await?;
                let decided = Caller { actor_id: caller.actor_id, now: j.now.clone() };
                // parameterized actions use bundle.apply, whose values/expect are checked by apply_op.
                if facts["resources"][res]["transitions"][t].get("patch").is_some()
                    || facts["resources"][res]["transitions"][t].get("expect").is_some()
                {
                    return rej("WIRE.BAD_VALUE", "patch/expect 전이는 values/expect를 제공하는 bundle.apply로 호출해야 함");
                }
                update(tx, facts, res, t, &j.change, &decided, knobs, None).await?;
                effects(tx, facts, res, t, &j.change, &decided).await?;
                out.applied.push(Applied { changed: j.change, unchanged: j.unchanged });
            }
            Step::Create(target, vals) => {
                for id in ids {
                    let mut row = vec![];
                    for (f, c, src) in vals {
                        row.push((
                            f.clone(),
                            match src {
                                Some(p) => Src::Item { res: res.to_string(), id: *id, path: p.clone() },
                                None => literal(facts, target, f, c, wire)?,
                            },
                        ));
                    }
                    let created = create_row(tx, facts, target, &row, caller, knobs).await?;
                    out.created += 1;
                    out.created_rows.push((target.to_string(), created));
                }
            }
        }
    }
    if !cp["sameScope"].is_null() && (scopes[0].is_none() || scopes.iter().any(|s| *s != scopes[0])) {
        return rej("WIRE.NOT_SAME_SCOPE", "대상들이 같은 범위가 아님");
    }
    Ok(out)
}

/// 쓰기 확장(V4)용: 호출자가 가진 트랜잭션 안에서 공개 전이 하나를 실행한다. 커밋은 호출자가 한다.
pub async fn apply_in(tx: &Transaction<'_>, facts: &Value, req: &Value, caller: &Caller, knobs: &Knobs) -> Result<Applied, Reject> {
    let o = req.as_object().ok_or(bad("요청은 객체"))?;
    apply_op(tx, facts, o, caller, knobs, IdWire::Legacy).await
}

pub async fn apply_in_with_wire(
    tx: &Transaction<'_>,
    facts: &Value,
    req: &Value,
    caller: &Caller,
    knobs: &Knobs,
    wire: IdWire,
) -> Result<Applied, Reject> {
    let o = req.as_object().ok_or(bad("요청은 객체"))?;
    apply_op(tx, facts, o, caller, knobs, wire).await
}

/// 쓰기 확장(V4)용: 이 트랜잭션이 쓴 행의 사후조건 검사.
pub async fn checks_in(tx: &Transaction<'_>, facts: &Value, caller: &Caller, knobs: &Knobs) -> Result<(), Reject> {
    run_checks(tx, facts, caller, knobs).await
}

/// 커밋 오류 분류(지연 제약 위반 등)를 쓰기 확장에서도 같게 쓴다.
pub fn classify_write_error(stage: &str, e: tokio_postgres::Error) -> Reject {
    write_err(stage, e)
}

/// UUID-aware audit keeps numeric deployments unchanged; mixed identities are stored losslessly as text.
pub fn audit_ddl_with_uuid(schema: &str) -> String {
    audit_ddl(schema)
        .replace("target_id bigint NOT NULL", "target_id text NOT NULL")
        .replace(" COMMIT", &format!(" ALTER TABLE {schema}.aip_audit ALTER COLUMN target_id TYPE text USING target_id::text; COMMIT"))
}
