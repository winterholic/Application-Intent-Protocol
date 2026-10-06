//! V3-1 단일 표준 전이 쓰기. 대상 판정과 변경을 한 트랜잭션에서 하고, 일부만 바꾼 성공 응답을 만들지 않는다.
use serde_json::{json, Map, Value};
use spike_v2_read::id_wire::{parse_id, IdWire};
use spike_v2_read::plan::{Caller, Reject};
use spike_v2_read::sqlgen::{self, column, table, Ctx, Env};
use std::time::Duration;
use tokio_postgres::{types::ToSql, Client, Transaction};

#[derive(Debug, PartialEq)]
pub struct Applied {
    pub changed: Vec<i64>,
    pub unchanged: Vec<i64>,
}

/// 테스트용 조절 손잡이. 잠금 뒤 멈춰서 동시 실행 순서를 재현한다.
#[derive(Default, Clone)]
pub struct Knobs {
    pub pause_after_lock_ms: u64,
    pub skip_row_lock: bool,
    pub skip_policy_lock: bool,
    pub pause_before_commit_ms: u64,
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

fn db_err(stage: &str, e: tokio_postgres::Error) -> Reject {
    match e.code().map(|c| c.code()) {
        Some("57014") => Reject { code: "DEADLINE_EXCEEDED", msg: format!("{stage}: {WRITE_DEADLINE_MS}ms 안에 끝나지 않음") },
        // 교착·직렬화 실패는 전체 rollback된 상태라 다시 시도할 수 있다.
        Some("40P01") | Some("40001") => Reject { code: "CONFLICT", msg: format!("{stage}: 동시 쓰기와 충돌. 다시 시도 가능") },
        c => Reject { code: "INTERNAL", msg: format!("{stage} 실패(sqlstate {})", c.unwrap_or("?")) },
    }
}

fn rej<T>(code: &'static str, msg: impl Into<String>) -> Result<T, Reject> {
    Err(Reject { code, msg: msg.into() })
}
fn internal(e: impl ToString) -> Reject {
    Reject { code: "INTERNAL", msg: e.to_string() }
}

fn keys(o: &Map<String, Value>, allowed: &[&str], what: &str) -> Result<(), Reject> {
    match o.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => rej("UNKNOWN_KEY", format!("{what}에 알 수 없는 키 `{k}`")),
        None => Ok(()),
    }
}

fn filter_value(ty: &str, v: &Value) -> Result<(String, &'static str), Reject> {
    match (ty.trim_end_matches('?'), v) {
        ("Bool", Value::Bool(b)) => Ok((b.to_string(), "boolean")),
        ("Int", Value::Number(n)) if n.is_i64() => Ok((n.to_string(), "bigint")),
        ("Text", Value::String(s)) => Ok((s.clone(), "text")),
        (t, _) => rej("BAD_VALUE", format!("`{t}` 타입에 맞지 않는 값 {v}")),
    }
}

pub async fn apply(db: &mut Client, facts: &Value, req: &Value, caller: &Caller, knobs: &Knobs) -> Result<Applied, Reject> {
    with_deadline(|ctl| apply_inner(db, facts, req, caller, knobs, ctl)).await
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
        Err(_) if sent.load(std::sync::atomic::Ordering::SeqCst) => rej("COMMIT_UNKNOWN", "커밋 결과를 기한 안에 확인하지 못함. 상태 조회 필요"),
        // 커밋을 보내기 전이면 트랜잭션은 drop되며 rollback된다.
        Err(_) => rej("DEADLINE_EXCEEDED", format!("쓰기 요청이 {WRITE_DEADLINE_MS}ms 안에 끝나지 않음")),
    }
}

async fn commit(tx: Transaction<'_>, ctl: &Ctl, knobs: &Knobs) -> Result<(), Reject> {
    if knobs.pause_before_commit_ms > 0 {
        tokio::time::sleep(Duration::from_millis(knobs.pause_before_commit_ms)).await;
    }
    let left = WRITE_DEADLINE_MS.saturating_sub(ctl.started.elapsed().as_millis() as u64);
    if left < COMMIT_MARGIN_MS {
        tx.rollback().await.ok();
        return rej("DEADLINE_EXCEEDED", format!("커밋 전 남은 기한 {left}ms. rollback함"));
    }
    ctl.commit_sent.store(true, std::sync::atomic::Ordering::SeqCst);
    // 지연 제약(커밋 시점 불변식) 위반은 커밋에서 드러난다.
    tx.commit().await.map_err(|e| write_err("커밋", e))
}

fn bad(msg: impl Into<String>) -> Reject {
    Reject { code: "BAD_REQUEST", msg: msg.into() }
}

fn id_array(ids: &[i64]) -> String {
    format!("{{{}}}", ids.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","))
}

fn bind_all(params: &[Option<String>]) -> Vec<&(dyn ToSql + Sync)> {
    params.iter().map(|p| p as &(dyn ToSql + Sync)).collect()
}

pub enum TargetSpec {
    Ids(Vec<i64>),
    Where(Vec<(String, String, &'static str)>),
}

fn parse_ids(raw: &Value, bulk: i64, wire: IdWire) -> Result<Vec<i64>, Reject> {
    let raw = raw.as_array().ok_or(bad("ids는 배열"))?;
    if raw.is_empty() {
        return Err(bad("ids가 비었음"));
    }
    // 상한을 먼저 본다. 큰 배열의 중복 검사가 기한 밖 CPU 시간을 쓰지 않게 한다.
    if raw.len() as i64 > bulk {
        return rej("BULK_LIMIT", format!("대상 {}개 > 상한 {bulk}", raw.len()));
    }
    let mut v = vec![];
    for x in raw {
        let n = parse_id(x, wire)?;
        if v.contains(&n) {
            return rej("DUPLICATE_TARGET", format!("id {n} 중복"));
        }
        v.push(n);
    }
    Ok(v)
}

fn parse_target(facts: &Value, res: &str, ex: &Value, tg: &Value, wire: IdWire) -> Result<TargetSpec, Reject> {
    let rf = &facts["resources"][res];
    let bulk = ex["bulkMaxRows"].as_i64().unwrap_or(1);
    let forms: Vec<&str> = ex["target"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    let tg = tg.as_object().ok_or(bad("target 객체 필요"))?;
    keys(tg, &["ids", "where"], "target")?;
    match (tg.get("ids"), tg.get("where")) {
        (Some(ids), None) => {
            if !forms.contains(&"id") {
                return rej("TARGET_NOT_ALLOWED", "id 대상은 계약에 없음");
            }
            Ok(TargetSpec::Ids(parse_ids(ids, bulk, wire)?))
        }
        (None, Some(w)) => {
            if !forms.contains(&"where") {
                return rej("TARGET_NOT_ALLOWED", "where 대상은 계약에 없음");
            }
            let allow = rf["exposeRead"]["filter"].as_array().cloned().unwrap_or_default();
            let mut v = vec![];
            for f in w.as_array().ok_or(bad("where는 배열"))? {
                let fo = f.as_object().ok_or(bad("where 항목은 객체"))?;
                keys(fo, &["field", "op", "value"], "where")?;
                let (field, op) = (fo.get("field").and_then(Value::as_str).unwrap_or(""), fo.get("op").and_then(Value::as_str).unwrap_or(""));
                // 쓰기 대상 조건도 읽기 filter 허용 목록을 따른다. 아니면 변경 개수로 숨은 값을 추론할 수 있다.
                if !allow.iter().any(|x| x == &json!(format!("{field}.{op}"))) || op != "eq" {
                    return rej("FILTER_NOT_ALLOWED", format!("`{field}.{op}` 조건은 계약에 없음"));
                }
                // 같은 필드 eq를 반복하면 의미 없이 SQL만 커진다. 조건 수는 허용 목록 크기로 제한된다.
                if v.iter().any(|(c, _, _): &(String, String, &str)| Some(c) == column(facts, res, field).as_ref()) {
                    return rej("DUPLICATE_FILTER", format!("`{field}.{op}` 조건 중복"));
                }
                let ty = rf["fields"][field]["ty"].as_str().unwrap_or("").trim_end_matches('?');
                let value = fo.get("value").unwrap_or(&Value::Null);
                let (val, cast) = if wire != IdWire::Legacy && (ty.starts_with("Id<") || ty.starts_with("Ref<")) {
                    (parse_id(value, wire)?.to_string(), "bigint")
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
    change: Vec<i64>,
    unchanged: Vec<i64>,
    scopes: Vec<Option<String>>,
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
    let rf = &facts["resources"][res];
    let t = &rf["transitions"][tr];
    let mut cx = Ctx::new(facts, caller.actor_id, &caller.now);
    cx.lock_reads = !knobs.skip_policy_lock;
    let env = Env { this: Some(("t".into(), res.to_string())), ..Default::default() };
    let vis = cx.cond(&rf["rowRead"], &env).map_err(internal)?;
    let allow = cx.cond(&t["allow"], &env).map_err(internal)?;
    let from = cx.cond(&t["from"], &env).map_err(internal)?;
    let mut done = vec![];
    for (f, v) in t["to"].as_object().unwrap() {
        let col = column(facts, res, f).unwrap();
        let val = cx.value(v, &env).map_err(internal)?;
        done.push(format!("(t.{col} IS NOT DISTINCT FROM {val})"));
    }
    let done = format!("({})", done.join(" AND "));
    let scope_sql = if scope.is_null() { "NULL::text".to_string() } else { format!("({})::text", cx.value(scope, &env).map_err(internal)?) };
    let lock = if knobs.skip_row_lock { "" } else { " FOR UPDATE OF t" };
    let sel = match target {
        TargetSpec::Ids(ids) => {
            let p = cx.params.bind(Some(id_array(ids)), "bigint[]");
            // 행 정책을 WHERE에 둬서 안 보이는 행은 잠그지도 기다리지도 않는다(R2B-11).
            format!("SELECT t.id, {allow}, {from}, {done}, {scope_sql} FROM {} t WHERE t.id = ANY({p}) AND {vis}{lock}", table(res))
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
    let rows = tx.query(sel.as_str(), &bind_all(&params)).await.map_err(|e| db_err("대상 조회", e))?;
    if knobs.pause_after_lock_ms > 0 {
        tokio::time::sleep(Duration::from_millis(knobs.pause_after_lock_ms)).await;
    }
    let mut j = Judged { change: vec![], unchanged: vec![], scopes: vec![] };
    let mut seen = vec![];
    for r in &rows {
        let (id, a, f, d, sc): (i64, Option<bool>, Option<bool>, Option<bool>, Option<String>) = (r.get(0), r.get(1), r.get(2), r.get(3), r.get(4));
        seen.push(id);
        if a != Some(true) {
            return rej("FORBIDDEN", format!("{id}에 `{tr}` 권한 없음"));
        }
        // 이미 목표 상태면 from보다 먼저 본다(R2B-10).
        if d == Some(true) && t["repeat"] == "unchanged" {
            j.unchanged.push(id);
        } else if f == Some(true) {
            j.change.push(id);
        } else {
            return rej("INVALID_STATE", format!("{id}는 `{tr}` 이전 상태가 아님"));
        }
        j.scopes.push(sc);
    }
    match target {
        TargetSpec::Ids(ids) => {
            let missing = ids.iter().filter(|x| !seen.contains(x)).count();
            if missing > 0 {
                // 없는 행과 안 보이는 행을 구분하지 않는다.
                return rej("MISSING_TARGET", format!("대상 {missing}개를 찾을 수 없음"));
            }
        }
        TargetSpec::Where(_) if seen.len() as i64 > bulk => return rej("BULK_LIMIT", format!("대상이 상한 {bulk}을 넘음")),
        _ => {}
    }
    // 같은 범위(예: 같은 동아리) 검사는 가시성·권한 판정 뒤에 한다. 권한 없는 호출자는 범위 일치 여부를 알 수 없다.
    if !scope.is_null() {
        let first = j.scopes.first().cloned().flatten();
        if first.is_none() || j.scopes.iter().any(|x| *x != first) {
            return rej("NOT_SAME_SCOPE", "대상들이 같은 범위가 아님");
        }
    }
    j.change.sort();
    j.unchanged.sort();
    Ok(j)
}

async fn update(tx: &Transaction<'_>, facts: &Value, res: &str, tr: &str, change: &[i64], caller: &Caller) -> Result<(), Reject> {
    if change.is_empty() {
        return Ok(());
    }
    let t = &facts["resources"][res]["transitions"][tr];
    let mut cx = Ctx::new(facts, caller.actor_id, &caller.now);
    let env = Env { this: Some(("t".into(), res.to_string())), ..Default::default() };
    let from = cx.cond(&t["from"], &env).map_err(internal)?;
    let mut sets = vec![];
    for (f, v) in t["to"].as_object().unwrap() {
        sets.push(format!("{} = {}", column(facts, res, f).unwrap(), cx.value(v, &env).map_err(internal)?));
    }
    let ip = cx.params.bind(Some(id_array(change)), "bigint[]");
    // UPDATE에도 from 조건을 다시 건다. 잠금이 없거나 우회돼도 이미 바뀐 행을 두 번 바꾸지 않는다.
    let up = format!("UPDATE {} t SET {} WHERE t.id = ANY({ip}) AND {from} RETURNING t.id", table(res), sets.join(", "));
    let params = cx.params.values.clone();
    let n = tx.query(up.as_str(), &bind_all(&params)).await.map_err(|e| write_err("변경", e))?.len();
    if n != change.len() {
        return rej("CONFLICT", format!("판정 {}개와 실제 변경 {n}개가 다름. 전체 취소", change.len()));
    }
    Ok(())
}

fn write_err(stage: &str, e: tokio_postgres::Error) -> Reject {
    match e.code().map(|c| c.code()) {
        Some("23505") => Reject { code: "ALREADY_EXISTS", msg: format!("{stage}: 유일 제약 위반") },
        Some("23P01") => Reject { code: "INVARIANT_VIOLATED", msg: format!("{stage}: 불변식 위반") },
        // 정의의 값 제약(길이·범위·필수)은 호출자 값 오류다. 내부 오류로 보고하지 않는다.
        Some("23514") => Reject { code: "BAD_VALUE", msg: format!("{stage}: 값 제약 위반") },
        Some("23502") => Reject { code: "BAD_VALUE", msg: format!("{stage}: 필수 값 누락") },
        _ => db_err(stage, e),
    }
}

/// 서버가 정의한 전이 효과. 값은 바뀐 대상 행에서 계산한다(W2).
async fn effects(tx: &Transaction<'_>, facts: &Value, res: &str, tr: &str, change: &[i64], caller: &Caller) -> Result<(), Reject> {
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
            let ip = cx.params.bind(Some(id_array(change)), "bigint[]");
            let sql = format!(
                "UPDATE {} u SET {} FROM {} t WHERE t.id = ANY({ip}) AND {} RETURNING t.id",
                table(target),
                sets.join(", "),
                table(res),
                conds.join(" AND ")
            );
            let params = cx.params.values.clone();
            let mut got: Vec<i64> =
                tx.query(sql.as_str(), &bind_all(&params)).await.map_err(|e| write_err("전이 효과", e))?.iter().map(|r| r.get(0)).collect();
            got.sort();
            if got != change {
                return rej("EFFECT_TARGET_MISSING", format!("`{target}` 갱신 대상이 대상 행마다 하나가 아님"));
            }
            continue;
        }
        let sql = if let Some(target) = e["create"].as_str() {
            let (mut cols, mut vals) = (vec![], vec![]);
            for (f, v) in e["values"].as_object().unwrap() {
                cols.push(column(facts, target, f).unwrap());
                vals.push(cx.value(v, &env).map_err(internal)?);
            }
            let ip = cx.params.bind(Some(id_array(change)), "bigint[]");
            format!("INSERT INTO {} ({}) SELECT {} FROM {} t WHERE t.id = ANY({ip})", table(target), cols.join(", "), vals.join(", "), table(res))
        } else {
            let n = &e["notify"];
            let to = cx.value(&n["to"], &env).map_err(internal)?;
            let topic = cx.params.bind(Some(n["topic"].as_str().unwrap().to_string()), "text");
            let src = cx.params.bind(Some(res.to_string()), "text");
            let ip = cx.params.bind(Some(id_array(change)), "bigint[]");
            // 알림은 같은 트랜잭션의 outbox 행이다. 승인이 rollback되면 알림 기록도 없다.
            format!(
                "INSERT INTO {}.aip_outbox (topic, recipient_id, source, source_id) SELECT {topic}, {to}, {src}, t.id FROM {} t WHERE t.id = ANY({ip})",
                sqlgen::schema(),
                table(res)
            )
        };
        let params = cx.params.values.clone();
        tx.execute(sql.as_str(), &bind_all(&params)).await.map_err(|e| write_err("전이 효과", e))?;
    }
    Ok(())
}

/// 생성할 행의 값 출처. 호출자 리터럴(W0) 또는 서버가 읽은 대상 행의 경로(W1).
pub enum Src {
    Lit(String, &'static str),
    Item { res: String, id: i64, path: Value },
}

/// expose create의 allow를 새 행에 대해 평가하며 한 행씩 넣는다. allow 거짓이면 FORBIDDEN.
async fn create_row(
    tx: &Transaction<'_>,
    facts: &Value,
    target: &str,
    values: &[(String, Src)],
    caller: &Caller,
    knobs: &Knobs,
) -> Result<(), Reject> {
    let ec = &facts["resources"][target]["exposeCreate"];
    let mut cx = Ctx::new(facts, caller.actor_id, &caller.now);
    cx.lock_reads = !knobs.skip_policy_lock;
    let (mut cols, mut exprs) = (vec![], vec![]);
    let mut from = String::new();
    for (f, src) in values {
        let col = column(facts, target, f).unwrap();
        let e = match src {
            Src::Lit(v, cast) => cx.params.bind(Some(v.clone()), cast),
            Src::Item { res, id, path } => {
                if from.is_empty() {
                    let p = cx.params.bind(Some(id.to_string()), "bigint");
                    from = format!(" FROM {} s WHERE s.id = {p}", table(res));
                }
                let env = Env { this: Some(("s".into(), res.clone())), ..Default::default() };
                cx.value(path, &env).map_err(internal)?
            }
        };
        exprs.push(format!("{e} AS {col}"));
        cols.push(col);
    }
    let venv = Env { this: Some(("v".into(), target.to_string())), ..Default::default() };
    let allow = cx.cond(&ec["allow"], &venv).map_err(internal)?;
    let sql = format!(
        "INSERT INTO {} ({}) SELECT {} FROM (SELECT {}{from}) v WHERE {allow} RETURNING 1",
        table(target),
        cols.join(", "),
        cols.iter().map(|c| format!("v.{c}")).collect::<Vec<_>>().join(", "),
        exprs.join(", ")
    );
    let params = cx.params.values.clone();
    let n = tx.query(sql.as_str(), &bind_all(&params)).await.map_err(|e| write_err("생성", e))?.len();
    if n != 1 {
        return rej("FORBIDDEN", format!("`{target}` 생성 권한 없음"));
    }
    Ok(())
}

/// 커밋 검사 = 이 트랜잭션이 만들거나 바꾼 행(xmin = 현재 트랜잭션)의 사후조건이다. 전역 관계 불변식이 아니다.
/// 커밋 뒤 다른 쓰기가 근거 행을 바꿔도 다시 검사하지 않는다(R3-03). savepoint 안에서 쓴 행은 빠진다.
async fn run_checks(tx: &Transaction<'_>, facts: &Value, caller: &Caller, knobs: &Knobs) -> Result<(), Reject> {
    for (res, rf) in facts["resources"].as_object().unwrap() {
        for (name, cond) in rf["checks"].as_object().into_iter().flatten() {
            let mut cx = Ctx::new(facts, caller.actor_id, &caller.now);
            // 검사 근거가 된 다른 행(예: 승인된 Apply)을 커밋까지 공유 잠금한다. 검사 뒤 커밋 전 철회를 막는다(R3-03).
            cx.lock_reads = !knobs.skip_policy_lock;
            let env = Env { this: Some(("c".into(), res.clone())), ..Default::default() };
            let c = cx.cond(cond, &env).map_err(internal)?;
            let sql = format!("SELECT count(*) FROM {} c WHERE c.xmin = pg_current_xact_id()::xid AND ({c}) IS NOT TRUE", table(res));
            let params = cx.params.values.clone();
            let n: i64 = tx.query_one(sql.as_str(), &bind_all(&params)).await.map_err(|e| db_err("커밋 검사", e))?.get(0);
            if n > 0 {
                return rej("CHECK_FAILED", format!("`{res}.{name}`를 만족하지 않는 행 {n}개"));
            }
        }
    }
    Ok(())
}

async fn begin<'a>(db: &'a mut Client) -> Result<Transaction<'a>, Reject> {
    let tx = db.transaction().await.map_err(internal)?;
    tx.batch_execute(&format!("SET LOCAL statement_timeout = '{WRITE_DEADLINE_MS}ms'; SET LOCAL TimeZone = 'UTC'")).await.map_err(internal)?;
    Ok(tx)
}

fn exposed<'a>(facts: &'a Value, name: &str) -> Result<(&'a str, &'a str, &'a Value), Reject> {
    let (res, tr) = name.split_once('.').ok_or(bad("apply는 `Resource.transition`"))?;
    let ex = &facts["resources"][res]["exposeApply"][tr];
    if ex.is_null() {
        return rej("NOT_EXPOSED", format!("`{name}`는 공개된 쓰기 동작이 아님"));
    }
    let res = facts["resources"].as_object().unwrap().keys().find(|k| *k == res).unwrap();
    let tr = facts["resources"][res.as_str()]["transitions"].as_object().unwrap().keys().find(|k| *k == tr).unwrap();
    Ok((res, tr, ex))
}

async fn apply_op(
    tx: &Transaction<'_>,
    facts: &Value,
    o: &serde_json::Map<String, Value>,
    caller: &Caller,
    knobs: &Knobs,
    wire: IdWire,
) -> Result<Applied, Reject> {
    keys(o, &["apply", "target"], "apply 요청")?;
    let (res, tr, ex) = exposed(facts, o.get("apply").and_then(Value::as_str).unwrap_or(""))?;
    let target = parse_target(facts, res, ex, o.get("target").unwrap_or(&Value::Null), wire)?;
    let j = judge(tx, facts, res, tr, &target, ex["bulkMaxRows"].as_i64().unwrap_or(1), &ex["sameScope"], caller, knobs).await?;
    update(tx, facts, res, tr, &j.change, caller).await?;
    effects(tx, facts, res, tr, &j.change, caller).await?;
    Ok(Applied { changed: j.change, unchanged: j.unchanged })
}

async fn apply_inner(db: &mut Client, facts: &Value, req: &Value, caller: &Caller, knobs: &Knobs, ctl: Ctl) -> Result<Applied, Reject> {
    let o = req.as_object().ok_or(bad("요청은 객체"))?;
    let tx = begin(db).await?;
    let a = apply_op(&tx, facts, o, caller, knobs, IdWire::Legacy).await?;
    run_checks(&tx, facts, caller, knobs).await?;
    commit(tx, &ctl, knobs).await?;
    Ok(a)
}

fn literal(facts: &Value, res: &str, field: &str, v: &Value) -> Result<Src, Reject> {
    let ty =
        facts["resources"][res]["fields"][field]["ty"].as_str().ok_or(Reject { code: "UNKNOWN_FIELD", msg: format!("`{res}.{field}` 없음") })?;
    let base = ty.trim_end_matches('?');
    if let Some(e) = base.strip_prefix("Enum<").and_then(|x| x.strip_suffix('>')) {
        let s = v.as_str().filter(|s| facts["enums"][e].as_array().unwrap().iter().any(|x| x == s));
        return s.map(|s| Src::Lit(s.to_string(), "text")).ok_or(Reject { code: "BAD_VALUE", msg: format!("`{field}`는 {e} 값") });
    }
    if base.starts_with("Ref<") || base.starts_with("Id<") {
        let s = v.as_str().filter(|s| !s.is_empty() && s.len() <= 18 && s.chars().all(|c| c.is_ascii_digit()));
        return s.map(|s| Src::Lit(s.to_string(), "bigint")).ok_or(Reject { code: "BAD_VALUE", msg: format!("`{field}` id 형식") });
    }
    let (s, cast) = filter_value(base, v)?;
    Ok(Src::Lit(s, cast))
}

#[derive(Debug, PartialEq)]
pub struct BundleResult {
    pub applied: Vec<Applied>,
    pub created: usize,
}

/// W0: 이미 공개된 동작(전이 apply, expose create)을 한 트랜잭션으로 묶는다. 연산 사이 값 전달은 없다.
pub async fn bundle(db: &mut Client, facts: &Value, req: &Value, caller: &Caller, knobs: &Knobs) -> Result<BundleResult, Reject> {
    with_deadline(|ctl| async move {
        let o = req.as_object().ok_or(bad("요청은 객체"))?;
        keys(o, &["atomic"], "묶음 요청")?;
        let ops = o.get("atomic").and_then(Value::as_array).ok_or(bad("atomic 배열 필요"))?;
        if ops.is_empty() || ops.len() > 20 {
            return Err(bad("연산은 1~20개"));
        }
        let tx = begin(db).await?;
        let mut out = BundleResult { applied: vec![], created: 0 };
        for op in ops {
            let m = op.as_object().ok_or(bad("연산은 객체"))?;
            if m.contains_key("apply") {
                out.applied.push(apply_op(&tx, facts, m, caller, knobs, IdWire::Legacy).await?);
            } else {
                keys(m, &["create", "values"], "create 연산")?;
                let res = m.get("create").and_then(Value::as_str).unwrap_or("");
                let ec = &facts["resources"][res]["exposeCreate"];
                if ec.is_null() {
                    return rej("NOT_EXPOSED", format!("`{res}` 생성은 공개되지 않음"));
                }
                let vals = m.get("values").and_then(Value::as_object).ok_or(bad("values 객체 필요"))?;
                let allowed: Vec<&str> = ec["fields"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
                let mut row = vec![];
                for (f, v) in vals {
                    if !allowed.contains(&f.as_str()) {
                        return rej("FIELD_NOT_EXPOSED", format!("`{res}.{f}`는 생성 계약에 없음"));
                    }
                    row.push((f.clone(), literal(facts, res, f, v)?));
                }
                create_row(&tx, facts, res, &row, caller, knobs).await?;
                out.created += 1;
            }
        }
        run_checks(&tx, facts, caller, knobs).await?;
        commit(tx, &ctl, knobs).await?;
        Ok(out)
    })
    .await
}

fn path_text(p: &Value) -> Option<String> {
    // facts의 출처가 경로가 아니면(의미 검사가 막지만) 비교 대상에서 뺀다. panic하지 않는다(R3-06).
    Some(p["path"]["segs"].as_array()?.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("."))
}

/// W1: 호출자가 대상 목록과 항목별 단계를 조합한다. 허용 단계·값 출처는 서버의 expose compose가 정한다.
pub async fn compose(db: &mut Client, facts: &Value, req: &Value, caller: &Caller, knobs: &Knobs) -> Result<BundleResult, Reject> {
    with_deadline(|ctl| async move {
        let o = req.as_object().ok_or(bad("요청은 객체"))?;
        keys(o, &["compose", "targets", "steps"], "조합 요청")?;
        let res = o.get("compose").and_then(Value::as_str).unwrap_or("");
        let cp = &facts["resources"][res]["exposeCompose"];
        if cp.is_null() {
            return rej("NOT_EXPOSED", format!("`{res}` 조합은 공개되지 않음"));
        }
        let res = facts["resources"].as_object().unwrap().keys().find(|k| *k == res).unwrap().as_str();
        let bulk = cp["bulkMaxRows"].as_i64().unwrap_or(1);
        let tg = o.get("targets").and_then(Value::as_object).ok_or(bad("targets 객체 필요"))?;
        keys(tg, &["ids"], "targets")?;
        let ids = parse_ids(tg.get("ids").unwrap_or(&Value::Null), bulk, IdWire::Legacy)?;
        let steps = o.get("steps").and_then(Value::as_array).ok_or(bad("steps 배열 필요"))?;
        if steps.is_empty() || steps.len() > 10 {
            return Err(bad("단계는 1~10개"));
        }
        // 단계 형식을 트랜잭션 전에 모두 검사한다.
        enum Step<'a> {
            Tr(&'a str, bool),
            Create(&'a str, Vec<(String, Value, Option<Value>)>),
        }
        let mut plan = vec![];
        for st in steps {
            let m = st.as_object().ok_or(bad("단계는 객체"))?;
            if let Some(t) = m.get("transition").and_then(Value::as_str) {
                keys(m, &["transition", "on"], "transition 단계")?;
                let on_self = match m.get("on").and_then(Value::as_str) {
                    None | Some("targets") => false,
                    Some("self") if !cp["selfRow"].is_null() => true,
                    _ => return rej("STEP_NOT_ALLOWED", "on은 targets 또는 계약에 selfRow가 있을 때 self"),
                };
                let allowed = cp["transitions"].as_array().unwrap();
                let Some(t) = allowed.iter().filter_map(Value::as_str).find(|x| *x == t) else {
                    return rej("STEP_NOT_ALLOWED", format!("전이 `{t}`는 조합 계약에 없음"));
                };
                plan.push(Step::Tr(t, on_self));
            } else {
                keys(m, &["create", "values"], "create 단계")?;
                let target = m.get("create").and_then(Value::as_str).unwrap_or("");
                let decl = &cp["creates"][target];
                if decl.is_null() {
                    return rej("STEP_NOT_ALLOWED", format!("`{target}` 생성은 조합 계약에 없음"));
                }
                let target = cp["creates"].as_object().unwrap().keys().find(|k| *k == target).unwrap().as_str();
                let allowed_fields = &facts["resources"][target]["exposeCreate"]["fields"];
                let mut vals = vec![];
                for (f, v) in m.get("values").and_then(Value::as_object).ok_or(bad("values 객체 필요"))? {
                    if !allowed_fields.as_array().unwrap().iter().any(|x| x == f) {
                        return rej("FIELD_NOT_EXPOSED", format!("`{target}.{f}`는 생성 계약에 없음"));
                    }
                    let vo = v.as_object().ok_or(bad("값은 {item} 또는 {const}"))?;
                    let fty = facts["resources"][target]["fields"][f]["ty"].as_str().unwrap();
                    if let Some(p) = vo.get("item").and_then(Value::as_str) {
                        // 값은 서버가 선언한 출처 경로에서만 온다. 호출자가 임의 경로로 숨은 값을 옮기지 못하게 한다.
                        let Some(src) = decl["sources"].as_array().unwrap().iter().find(|s| path_text(s).as_deref() == Some(p)) else {
                            return rej("VALUE_SOURCE_NOT_ALLOWED", format!("출처 `{p}`는 조합 계약에 없음"));
                        };
                        let sty = src["ty"].as_str().unwrap();
                        if sty.trim_end_matches('?') != fty.trim_end_matches('?') || (sty.ends_with('?') && !fty.ends_with('?')) {
                            return rej("TYPE_MISMATCH", format!("`{p}`({sty})를 `{target}.{f}`({fty})에 넣을 수 없음"));
                        }
                        vals.push((f.clone(), Value::Null, Some(src.clone())));
                    } else if let Some(c) = vo.get("const") {
                        // 참조 필드는 상수로 받지 않는다. 대상 행에서 온 값만 연결할 수 있다.
                        if fty.starts_with("Ref<") || fty.starts_with("Id<") {
                            return rej("VALUE_SOURCE_NOT_ALLOWED", format!("참조 필드 `{f}`는 상수로 줄 수 없음"));
                        }
                        vals.push((f.clone(), c.clone(), None));
                    } else {
                        return Err(bad("값은 {item} 또는 {const}"));
                    }
                }
                plan.push(Step::Create(target, vals));
            }
        }
        let tx = begin(&mut *db).await?;
        // 모든 대상을 먼저 잠그고 가시성을 확인한다. 같은 범위 검사는 단계별 권한 판정 뒤에 한다(R3-01).
        let scopes: Vec<Option<String>>;
        {
            let rf = &facts["resources"][res];
            let mut cx = Ctx::new(facts, caller.actor_id, &caller.now);
            cx.lock_reads = !knobs.skip_policy_lock;
            let env = Env { this: Some(("t".into(), res.to_string())), ..Default::default() };
            let vis = cx.cond(&rf["rowRead"], &env).map_err(internal)?;
            let scope = if cp["sameScope"].is_null() {
                "NULL::text".into()
            } else {
                format!("({})::text", cx.value(&cp["sameScope"], &env).map_err(internal)?)
            };
            let p = cx.params.bind(Some(id_array(&ids)), "bigint[]");
            let sql = format!("SELECT t.id, {scope} FROM {} t WHERE t.id = ANY({p}) AND {vis} FOR UPDATE OF t", table(res));
            let params = cx.params.values.clone();
            let rows = tx.query(sql.as_str(), &bind_all(&params)).await.map_err(|e| db_err("대상 조회", e))?;
            if rows.len() != ids.len() {
                return rej("MISSING_TARGET", format!("대상 {}개를 찾을 수 없음", ids.len() - rows.len()));
            }
            scopes = rows.iter().map(|r| r.get(1)).collect();
        }
        let mut out = BundleResult { applied: vec![], created: 0 };
        for st in &plan {
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
                        let rows = tx.query(sql.as_str(), &bind_all(&params)).await.map_err(|e| db_err("자기 행 조회", e))?;
                        if rows.len() != 1 {
                            return rej("MISSING_TARGET", "같은 범위의 자기 행을 찾을 수 없음");
                        }
                        vec![rows[0].get::<_, i64>(0)]
                    } else {
                        ids.clone()
                    };
                    let j = judge(&tx, facts, res, t, &TargetSpec::Ids(step_ids), bulk, &Value::Null, caller, knobs).await?;
                    update(&tx, facts, res, t, &j.change, caller).await?;
                    effects(&tx, facts, res, t, &j.change, caller).await?;
                    out.applied.push(Applied { changed: j.change, unchanged: j.unchanged });
                }
                Step::Create(target, vals) => {
                    for id in &ids {
                        let mut row = vec![];
                        for (f, c, src) in vals {
                            row.push((
                                f.clone(),
                                match src {
                                    Some(p) => Src::Item { res: res.to_string(), id: *id, path: p.clone() },
                                    None => literal(facts, target, f, c)?,
                                },
                            ));
                        }
                        create_row(&tx, facts, target, &row, caller, knobs).await?;
                        out.created += 1;
                    }
                }
            }
        }
        if !cp["sameScope"].is_null() && (scopes[0].is_none() || scopes.iter().any(|s| *s != scopes[0])) {
            return rej("NOT_SAME_SCOPE", "대상들이 같은 범위가 아님");
        }
        run_checks(&tx, facts, caller, knobs).await?;
        commit(tx, &ctl, knobs).await?;
        Ok(out)
    })
    .await
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
