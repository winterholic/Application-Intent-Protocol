//! V7: facts 버전 비교 → 변경 분류 → 적용 전 DB 확인.
//! 분류: Safe(자동 적용 가능) / Breaking(기존 호출자 요청이 거부됨) / Destructive(데이터 손실, 사람 승인 필요)
//!      / SecurityReview(권한 범위 변화) / Blocked(기존 데이터가 새 규칙을 어김).
//! 판정은 기술 분류다. 사람 승인 절차 자체는 범위 밖이다.
use serde_json::{json, Value};
use spike_v2_read::sqlgen::{column, table, Ctx, Env};
use std::collections::BTreeSet;
use tokio_postgres::Client;

#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    pub class: &'static str,
    pub what: String,
    pub detail: Value,
}

fn ch(class: &'static str, what: String, detail: Value) -> Change {
    Change { class, what, detail }
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
}
fn arr(v: &Value) -> BTreeSet<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// 정적 비교. DB가 필요한 판정(정책 확대 정도, 기존 데이터 위반)은 `check`에서 한다.
pub fn diff(old: &Value, new: &Value) -> Vec<Change> {
    let mut out = vec![];
    for (e, vals) in new["enums"].as_object().into_iter().flatten() {
        let removed: Vec<String> = arr(&old["enums"][e]).difference(&arr(vals)).cloned().collect();
        if !removed.is_empty() {
            out.push(ch("Destructive", format!("enum {e} 값 제거"), json!({ "enum": e, "removed": removed })));
        }
    }
    let (or, nr) = (keys(&old["resources"]), keys(&new["resources"]));
    for r in or.difference(&nr) {
        out.push(ch("Destructive", format!("resource {r} 제거"), json!({ "resource": r })));
    }
    for r in nr.difference(&or) {
        out.push(ch("Safe", format!("resource {r} 추가"), json!({ "resource": r })));
    }
    for r in or.intersection(&nr) {
        let (o, n) = (&old["resources"][r], &new["resources"][r]);
        let (of, nf) = (keys(&o["fields"]), keys(&n["fields"]));
        for f in nf.difference(&of) {
            let ty = n["fields"][f]["ty"].as_str().unwrap_or("");
            // 기존 행이 있는 테이블에 NOT NULL 열을 기본값 없이 더할 수 없다.
            let class = if ty.ends_with('?') { "Safe" } else { "Blocked" };
            out.push(ch(class, format!("{r}.{f} 필드 추가({ty})"), json!({ "resource": r, "field": f, "ty": ty, "needsBackfill": !ty.ends_with('?') })));
        }
        for f in of.difference(&nf) {
            out.push(ch("Destructive", format!("{r}.{f} 필드 제거"), json!({ "resource": r, "field": f })));
        }
        for f in of.intersection(&nf) {
            if o["fields"][f]["range"] != n["fields"][f]["range"] {
                // 범위 제약 변경은 기존 값이 새 범위를 어기는지 세야 한다(r8 F7).
                out.push(ch("Blocked", format!("{r}.{f} 범위 변경"), json!({ "resource": r, "field": f, "range": n["fields"][f]["range"] })));
            }
            if o["fields"][f]["ty"] != n["fields"][f]["ty"] {
                let (ot, nt) = (o["fields"][f]["ty"].as_str().unwrap_or(""), n["fields"][f]["ty"].as_str().unwrap_or(""));
                let class = if ot.trim_end_matches('?') == nt.trim_end_matches('?') && ot.ends_with('?') && !nt.ends_with('?') {
                    "Blocked" // nullable → NOT NULL: 기존 null 행 확인 필요
                } else if ot.trim_end_matches('?') == nt.trim_end_matches('?') && nt.ends_with('?') {
                    "Safe" // NOT NULL → nullable
                } else {
                    "Destructive"
                };
                out.push(ch(class, format!("{r}.{f} 타입 변경 {ot} → {nt}"), json!({ "resource": r, "field": f, "from": ot, "to": nt })));
            }
        }
        // 호출자 계약: 줄면 기존 화면 요청이 거부된다. 늘면 안전(새 필드는 기본 비공개가 아니라 명시 공개라 보안 검토 대상).
        let (oe, ne) = (&o["exposeRead"], &n["exposeRead"]);
        for (part, ok, nk) in [
            ("select", keys(&oe["select"]), keys(&ne["select"])),
            ("filter", arr(&oe["filter"]), arr(&ne["filter"])),
            ("sort", arr(&oe["sort"]), arr(&ne["sort"])),
        ] {
            for x in ok.difference(&nk) {
                out.push(ch("Breaking", format!("{r} expose {part}에서 {x} 제거"), json!({ "resource": r, "part": part, "item": x })));
            }
            for x in nk.difference(&ok) {
                let class = if part == "select" { "SecurityReview" } else { "Safe" };
                out.push(ch(class, format!("{r} expose {part}에 {x} 추가"), json!({ "resource": r, "part": part, "item": x })));
            }
        }
        // 위에서 분류한 부분 밖의 차이는 의미를 모르므로 모두 보안 검토로 보낸다(r8 F1~F6). 빈 diff가 자동 적용 허가가 되지 않게.
        let mut oe2 = oe.clone();
        let mut ne2 = ne.clone();
        for k in ["select", "filter", "sort"] {
            if let Some(m) = oe2.as_object_mut() { m.remove(k); }
            if let Some(m) = ne2.as_object_mut() { m.remove(k); }
        }
        if oe2 != ne2 {
            out.push(ch("SecurityReview", format!("{r} expose read의 관계·budget·루트 조회 변경"), json!({ "resource": r, "from": oe2, "to": ne2 })));
        }
        for part in ["aggregates", "exposeAggregates", "transitions", "exposeApply", "exposeCreate", "exposeCompose", "extensions", "unique", "checks"] {
            if o[part] != n[part] {
                out.push(ch("SecurityReview", format!("{r} {part} 변경"), json!({ "resource": r, "part": part })));
            }
        }
        let (oi, ni) = (keys(&o["invariants"]), keys(&n["invariants"]));
        for inv in oi.intersection(&ni) {
            if o["invariants"][inv] != n["invariants"][inv] {
                out.push(ch("SecurityReview", format!("{r} 불변식 {inv} 변경"), json!({ "resource": r, "invariant": inv })));
            }
        }
        for inv in oi.difference(&ni) {
            out.push(ch("SecurityReview", format!("{r} 불변식 {inv} 제거"), json!({ "resource": r, "invariant": inv })));
        }
        if o["rowRead"] != n["rowRead"] {
            out.push(ch("SecurityReview", format!("{r} 행 정책 변경"), json!({ "resource": r })));
        }
        for (f, p) in n["fieldRead"].as_object().into_iter().flatten() {
            if o["fieldRead"][f] != *p {
                out.push(ch("SecurityReview", format!("{r}.{f} 필드 정책 변경"), json!({ "resource": r, "field": f })));
            }
        }
        for f in keys(&o["fieldRead"]).difference(&keys(&n["fieldRead"])) {
            out.push(ch("SecurityReview", format!("{r}.{f} 필드 정책 제거"), json!({ "resource": r, "field": f })));
        }
        for inv in keys(&n["invariants"]).difference(&keys(&o["invariants"])) {
            out.push(ch("Blocked", format!("{r} 불변식 {inv} 추가"), json!({ "resource": r, "invariant": inv })));
        }
    }
    // predicate·access·limit 본문은 여러 정책이 공유한다. 바뀌면 영향받는 정책을 알 수 없으므로 검토로 보낸다.
    // 새 이름 추가는 그것을 쓰는 정책·불변식 변경에서 판정된다. 기존 이름의 본문 변경·제거만 여기서 잡는다.
    for part in ["predicates", "accesses", "limits"] {
        for k in keys(&old[part]) {
            if old[part][&k] != new[part][&k] {
                out.push(ch("SecurityReview", format!("{part} {k} 변경 또는 제거"), json!({ "part": part, "name": k })));
            }
        }
    }
    if old["actor"] != new["actor"] {
        out.push(ch("SecurityReview", "actor 변경".into(), json!({ "part": "actor" })));
    }
    out
}

/// 행 정책 변경을 현재 데이터로 측정: 각 회원(+익명)이 보는 행 집합을 옛·새 정책으로 비교.
async fn policy_delta(db: &Client, old: &Value, new: &Value, res: &str, now: &str) -> Value {
    let members: Vec<i64> = db.query(format!("SELECT id FROM {} ORDER BY id", table("Member")).as_str(), &[]).await.unwrap().iter().map(|r| r.get(0)).collect();
    let mut actors: Vec<Option<i64>> = members.into_iter().map(Some).collect();
    actors.push(None);
    let (mut gained, mut lost) = (vec![], vec![]);
    for a in actors {
        let mut sets = vec![];
        for f in [old, new] {
            let mut cx = Ctx::new(f, a, now);
            let env = Env { this: Some(("t".into(), res.to_string())), ..Default::default() };
            let c = cx.cond(&f["resources"][res]["rowRead"], &env).unwrap();
            let sql = format!("SELECT t.id FROM {} t WHERE {c}", table(res));
            let params = cx.params.values.clone();
            let p: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params.iter().map(|x| x as _).collect();
            // 새 정책이 아직 없는 열(같이 추가되는 필드)을 쓰면 현재 DB로는 측정할 수 없다(r8 F12).
            let ids: BTreeSet<i64> = match db.query(sql.as_str(), &p).await {
                Ok(rows) => rows.iter().map(|r| r.get(0)).collect(),
                Err(_) => return json!({ "measurable": false }),
            };
            sets.push(ids);
        }
        for id in sets[1].difference(&sets[0]) {
            gained.push(json!({ "actor": a, "row": id }));
        }
        for id in sets[0].difference(&sets[1]) {
            lost.push(json!({ "actor": a, "row": id }));
        }
    }
    json!({ "measurable": true, "at": now, "gained": gained, "lost": lost })
}

/// DB가 필요한 판정을 채운다. 결과: (변경, 적용 가능 여부)
pub async fn check(db: &Client, old: &Value, new: &Value, now: &str) -> (Vec<Change>, bool) {
    let mut cs = diff(old, new);
    for c in cs.iter_mut() {
        let r = c.detail["resource"].as_str().unwrap_or("").to_string();
        if c.what.ends_with("행 정책 변경") {
            // 측정은 검토 근거일 뿐이다. 이 시각·이 데이터에서 새로 보는 행이 없어도 확대가 없다는 증명은 아니다(r8 F8).
            let d = policy_delta(db, old, new, &r, now).await;
            c.detail["delta"] = d;
        } else if c.what.contains("불변식") && !c.what.ends_with("제거") {
            let inv = &new["resources"][&r]["invariants"][c.detail["invariant"].as_str().unwrap()];
            let per = column(new, &r, inv["per"].as_str().unwrap()).unwrap();
            let mut cx = Ctx::for_ddl(new);
            let env = Env { this: Some(("t".into(), r.clone())), ..Default::default() };
            let cond = cx.cond(&inv["enforcement"]["where"], &env).unwrap();
            let max = inv["enforcement"]["max"].as_i64().unwrap_or(1);
            // DB 유일 인덱스처럼 NULL 그룹은 제한하지 않는다(r8 F9). nullable per면 그 사실을 남긴다.
            let per_nullable = new["resources"][&r]["fields"][inv["per"].as_str().unwrap()]["ty"].as_str().unwrap_or("").ends_with('?');
            c.detail["nullGroupsNotLimited"] = json!(per_nullable);
            let sql = format!("SELECT count(*) FROM (SELECT t.{per} FROM {} t WHERE ({cond}) AND t.{per} IS NOT NULL GROUP BY t.{per} HAVING count(*) > {max}) v", table(&r));
            let params = cx.params.values.clone();
            let p: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params.iter().map(|x| x as _).collect();
            let n: i64 = match db.query_one(sql.as_str(), &p).await {
                Ok(row) => row.get(0),
                Err(_) => {
                    c.detail["measurable"] = json!(false);
                    continue;
                }
            };
            c.detail["violatingGroups"] = json!(n);
            // 새 불변식 추가만 위반 0이면 안전. 기존 불변식 변경·제거는 측정과 무관하게 검토(r8 F6).
            if n == 0 && c.what.ends_with("추가") {
                c.class = "Safe";
            } else if n > 0 {
                c.class = "Blocked";
            }
        } else if c.what.contains("enum") {
            // 제거되는 값을 쓰는 행이 없으면 데이터 손실은 없다(계약은 여전히 바뀜).
            let e = c.detail["enum"].as_str().unwrap().to_string();
            let removed = c.detail["removed"].clone();
            // resource별로 제거 값을 쓰는 서로 다른 행 수(같은 행의 여러 필드를 중복으로 세지 않음, r8 F10).
            let mut using = 0i64;
            let vals: Vec<String> = removed.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
            for (res, rf) in old["resources"].as_object().unwrap() {
                let cols: Vec<String> = rf["fields"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .filter(|(_, fd)| fd["ty"].as_str().unwrap_or("").trim_end_matches('?') == format!("Enum<{e}>"))
                    .map(|(f, _)| format!("{} = ANY($1)", column(old, res, f).unwrap()))
                    .collect();
                if !cols.is_empty() {
                    let n: i64 = db.query_one(format!("SELECT count(*) FROM {} WHERE {}", table(res), cols.join(" OR ")).as_str(), &[&vals]).await.unwrap().get(0);
                    using += n;
                }
            }
            c.detail["rowsUsingRemoved"] = json!(using);
            c.class = if using > 0 { "Blocked" } else { "Breaking" };
        } else if c.what.ends_with("범위 변경") {
            let f = c.detail["field"].as_str().unwrap().to_string();
            let col = column(new, &r, &f).unwrap();
            let ty = new["resources"][&r]["fields"][&f]["ty"].as_str().unwrap_or("");
            let range = c.detail["range"].clone();
            let n: i64 = if let (Some(lo), Some(hi)) = (range[0].as_i64(), range[1].as_i64()) {
                let expr = if ty.starts_with("Text") { format!("char_length({col})") } else { col.clone() };
                db.query_one(format!("SELECT count(*) FROM {} WHERE {col} IS NOT NULL AND ({expr} < $1::bigint OR {expr} > $2::bigint)", table(&r)).as_str(), &[&lo, &hi]).await.unwrap().get(0)
            } else {
                0 // 범위 제거는 완화
            };
            c.detail["violatingRows"] = json!(n);
            if n == 0 {
                c.class = "Safe";
            }
        } else if c.what.contains("타입 변경") && c.class == "Blocked" {
            let col = column(old, &r, c.detail["field"].as_str().unwrap()).unwrap();
            let n: i64 = db.query_one(format!("SELECT count(*) FROM {} WHERE {col} IS NULL", table(&r)).as_str(), &[]).await.unwrap().get(0);
            c.detail["nullRows"] = json!(n);
            if n == 0 {
                c.class = "Safe";
            }
        } else if c.detail["needsBackfill"] == true {
            let n: i64 = db.query_one(format!("SELECT count(*) FROM {}", table(&r)).as_str(), &[]).await.unwrap().get(0);
            c.detail["existingRows"] = json!(n);
            if n == 0 {
                c.class = "Safe";
            }
        }
    }
    let applicable = cs.iter().all(|c| c.class == "Safe" || c.class == "Breaking");
    (cs, applicable)
}
