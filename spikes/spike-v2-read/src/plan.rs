//! 호출자 정형 읽기 요청을 facts의 허용 범위로 검증하고 매개변수 SQL 하나로 만든다.
use crate::sqlgen::{column, table, Ctx, Env, Handle};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Reject {
    pub code: &'static str,
    pub msg: String,
}

fn rej<T>(code: &'static str, msg: impl Into<String>) -> Result<T, Reject> {
    Err(Reject { code, msg: msg.into() })
}

fn internal(e: String) -> Reject {
    Reject { code: "INTERNAL", msg: e }
}

pub struct Plan {
    pub sql: String,
    pub params: Vec<Option<String>>,
    pub deadline_ms: u64,
    pub output_type: Value,
    pub cost: i64,
    /// 단독 집계에서 NULL 결과는 guard 거부다. 실행기가 ACCESS_DENIED로 바꾼다(R2B-01).
    pub null_is_denied: bool,
    /// 이 읽기가 실제로 읽는 resource. 생성 SQL이 참조하는 테이블에서 뽑는다(정책 하위조회 포함). 캐시 무효화 태그 후보.
    pub deps: Vec<String>,
}

/// SQL이 참조하는 schema 테이블을 resource 이름으로 되돌린다.
pub fn deps_of(facts: &Value, sql: &str) -> Vec<String> {
    let mut out: Vec<String> = facts["resources"]
        .as_object()
        .unwrap()
        .keys()
        .filter(|r| {
            let t = table(r);
            sql.match_indices(&t).any(|(i, _)| !sql[i + t.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
        })
        .cloned()
        .collect();
    out.sort();
    out
}

/// spike 출력 상한. budget에 출력 크기 위치가 아직 없어 고정값을 쓴다(R2B-03).
pub const MAX_OUTPUT_BYTES: usize = 1 << 20;

/// `in` 필터의 배열 길이 상한. 중복 제거 전 길이로 센다. 값마다 매개변수 하나를 쓰므로 요청 크기와 SQL 크기를 함께 막는다.
pub const MAX_IN_VALUES: usize = 50;

pub struct Caller {
    pub actor_id: Option<i64>,
    pub now: String,
}

fn keys(o: &Map<String, Value>, allowed: &[&str], what: &str) -> Result<(), Reject> {
    for k in o.keys() {
        if !allowed.contains(&k.as_str()) {
            return rej("UNKNOWN_KEY", format!("{what}에 알 수 없는 키 `{k}`"));
        }
    }
    Ok(())
}

/// 비용 추정은 자리표시 휴리스틱이다: 행 상한 × (1 + 관계 + 집계 + 필드 정책 수). 실제 DB 비용과 같지 않다.
fn estimate(limit: i64, traverse: usize, aggs: usize, field_policies: usize) -> i64 {
    limit * (1 + traverse as i64 + aggs as i64 + field_policies as i64)
}

fn out_ty(ty: &str, redactable: bool) -> Value {
    let t = ty.trim_end_matches('?');
    let nullable = ty.ends_with('?') || redactable;
    json!({ "ty": t, "nullable": nullable, "redactable": redactable })
}

/// count는 행 수, sum은 빈 집합이 0, min/max는 빈 집합이 NULL이다(sema가 min/max를 nullable 행별 집계로 제한).
fn measure(facts: &Value, aggregate: &Value, alias: &str) -> Result<String, Reject> {
    let source = aggregate["source"].as_str().unwrap();
    let field = aggregate["releaseField"].as_str();
    let col = |f: &str| column(facts, source, f).ok_or_else(|| internal(format!("집계 필드 `{f}` 없음")));
    match (aggregate["release"].as_str(), field) {
        (Some("count") | None, None) => Ok("count(*)".into()),
        // bigint 합은 numeric이 된다. 범위를 넘으면 캐스트가 실패해 잘린 값 대신 오류가 난다.
        (Some("sum"), Some(f)) => Ok(format!("coalesce(sum({alias}.{}), 0)::bigint", col(f)?)),
        (Some(op @ ("min" | "max")), Some(f)) => Ok(format!("{op}({alias}.{})", col(f)?)),
        (r, f) => Err(internal(format!("집계 release {r:?}({f:?}) 미지원"))),
    }
}

fn aggregate_select_expr(
    facts: &Value,
    cx: &mut Ctx<'_>,
    aggregate: &Value,
    owner: &str,
    owner_alias: &str,
    owner_env: &Env,
) -> Result<(String, Value), Reject> {
    let source = aggregate["source"].as_str().unwrap();
    let group_key = aggregate["groupKey"].as_str().unwrap();
    let source_group_key = column(facts, source, group_key).unwrap();
    let owner_id = column(facts, owner, "id").unwrap();
    let source_alias = cx.alias("g");
    let mut where_sql = format!("{source_alias}.{source_group_key} = {owner_alias}.{owner_id}");
    if !aggregate["where"].is_null() {
        let source_env = Env { this: Some((source_alias.clone(), source.to_string())), ..Default::default() };
        where_sql = format!("{where_sql} AND {}", cx.cond(&aggregate["where"], &source_env).map_err(internal)?);
    }
    // 집계 원본은 원본 행 정책을 자동 상속하지 않고 sourceAccess로만 읽는다.
    let count = format!("(SELECT {} FROM {} {source_alias} WHERE {where_sql})", measure(facts, aggregate, &source_alias)?, table(source));
    let access_ref = aggregate["sourceAccess"]["ref"].as_str().unwrap_or("");
    let access = &facts["accesses"][access_ref];
    let (expression, guarded) = match access["kind"].as_str() {
        Some("totalOfVisible") => (count, false),
        Some("guard") => {
            let mut guard_env = Env::default();
            for (parameter, argument) in access["params"].as_array().unwrap().iter().zip(aggregate["sourceAccess"]["args"].as_array().unwrap()) {
                let handle = cx.handle(argument, owner_env).map_err(internal)?;
                guard_env.vars.insert(parameter[0].as_str().unwrap().to_string(), handle);
            }
            let guard = cx.cond(&access["body"], &guard_env).map_err(internal)?;
            (format!("CASE WHEN {guard} THEN {count} END"), true)
        }
        kind => return Err(internal(format!("집계 sourceAccess 종류 {kind:?} 미지원"))),
    };
    let output_type = out_ty(aggregate["ty"].as_str().unwrap(), guarded);
    Ok((expression, output_type))
}

fn check_value(facts: &Value, ty: &str, v: &Value, wire: crate::id_wire::IdWire) -> Result<(String, &'static str), Reject> {
    let base = ty.trim_end_matches('?');
    if wire != crate::id_wire::IdWire::Legacy && (base.starts_with("Id<") || base.starts_with("Ref<")) {
        return crate::id_wire::parse_id(v, wire).map(|id| (id.to_string(), "bigint"));
    }
    match (base, v) {
        // 응답 Id는 숫자라 요청도 숫자를 받는다. 숫자 문자열도 받는다(r4b F04).
        (b, Value::Number(n)) if (b.starts_with("Id<") || b.starts_with("Ref<")) && n.as_i64().is_some_and(|x| x >= 0) => {
            Ok((n.to_string(), "bigint"))
        }
        (b, Value::String(s))
            if (b.starts_with("Id<") || b.starts_with("Ref<")) && !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) && s.len() <= 18 =>
        {
            Ok((s.clone(), "bigint"))
        }
        _ => crate::scalar::parse(facts, base, v),
    }
}

pub fn plan_read(facts: &Value, req: &Value, caller: &Caller) -> Result<Plan, Reject> {
    plan_read_with_wire(facts, req, caller, crate::id_wire::IdWire::Legacy)
}

pub fn plan_read_with_wire(facts: &Value, req: &Value, caller: &Caller, wire: crate::id_wire::IdWire) -> Result<Plan, Reject> {
    let mut p = plan_read_inner(facts, req, caller, wire)?;
    p.deps = deps_of(facts, &p.sql);
    Ok(p)
}

fn plan_read_inner(facts: &Value, req: &Value, caller: &Caller, wire: crate::id_wire::IdWire) -> Result<Plan, Reject> {
    let o = req.as_object().ok_or_else(|| Reject { code: "BAD_REQUEST", msg: "요청은 객체".into() })?;
    if o.contains_key("aggregate") {
        return plan_aggregate(facts, o, caller, wire);
    }
    keys(o, &["read", "select", "filter", "sort", "limit"], "read 요청")?;
    let res = o.get("read").and_then(Value::as_str).ok_or(Reject { code: "BAD_REQUEST", msg: "read 필요".into() })?;
    let rf = &facts["resources"][res];
    if rf.is_null() {
        return rej("UNKNOWN_RESOURCE", format!("`{res}` 없음"));
    }
    let ex = &rf["exposeRead"];
    if ex.is_null() {
        return rej("NOT_EXPOSED", format!("`{res}`는 읽기 노출이 없음"));
    }
    if ex["rootQueryable"] != true {
        return rej("NOT_ROOT_QUERYABLE", format!("`{res}`는 관계 대상 전용(budget 없음)"));
    }
    let budget = &ex["budget"];
    let mut cx = Ctx::new(facts, caller.actor_id, &caller.now);
    let t = "t".to_string();
    let env = Env { this: Some((t.clone(), res.to_string())), ..Default::default() };

    let sel = o.get("select").and_then(Value::as_array).ok_or(Reject { code: "BAD_REQUEST", msg: "select 배열 필요".into() })?;
    if sel.is_empty() {
        return rej("BAD_REQUEST", "select가 비었음");
    }
    let (mut cols, mut joins, mut out) = (vec![], vec![], Map::new());
    let (mut n_trav, mut n_agg, mut n_fp) = (0usize, 0usize, 0usize);
    let mut seen_sel = std::collections::HashSet::new();
    for s in sel {
        let key = match s {
            Value::String(f) => f.clone(),
            Value::Object(m) => m.keys().next().cloned().unwrap_or_default(),
            _ => String::new(),
        };
        if !seen_sel.insert(key.clone()) {
            return rej("DUPLICATE", format!("select `{key}` 중복"));
        }
        match s {
            Value::String(f) => {
                let kind = ex["select"][f].as_str();
                match kind {
                    None => return rej("FIELD_NOT_EXPOSED", format!("`{res}.{f}`는 계약에 열려 있지 않음. 서버 정의의 expose 변경이 필요")),
                    Some("aggregate") => {
                        let a = &rf["aggregates"][f];
                        let (expr, output_type) = aggregate_select_expr(facts, &mut cx, a, res, &t, &env)?;
                        cols.push((f.clone(), expr));
                        out.insert(f.clone(), output_type);
                        n_agg += 1;
                    }
                    Some(k) => {
                        let col = format!("{t}.{}", column(facts, res, f).unwrap());
                        let fty = rf["fields"][f]["ty"].as_str().unwrap();
                        let expr = if k == "fieldWithPolicy" {
                            n_fp += 1;
                            let c = cx.cond(&rf["fieldRead"][f], &env).map_err(internal)?;
                            format!("CASE WHEN {c} THEN {col} END")
                        } else {
                            col
                        };
                        cols.push((f.clone(), expr));
                        out.insert(f.clone(), out_ty(fty, k == "fieldWithPolicy"));
                    }
                }
            }
            Value::Object(m) if m.len() == 1 => {
                let (rel, sub) = m.iter().next().unwrap();
                let tr = &ex["traverse"][rel];
                if tr.is_null() {
                    return rej("TRAVERSE_NOT_ALLOWED", format!("`{res}.{rel}` 관계 탐색이 계약에 없음"));
                }
                let so = sub.as_object().ok_or(Reject { code: "BAD_REQUEST", msg: "traverse 값은 객체".into() })?;
                keys(so, &["select"], "traverse")?;
                let tsel = so.get("select").and_then(Value::as_array).ok_or(Reject { code: "BAD_REQUEST", msg: "traverse select 필요".into() })?;
                let depth = budget["depth"].as_i64().unwrap_or(1);
                if tsel.iter().any(|x| x.is_object()) {
                    return rej("DEPTH_EXCEEDED", format!("관계 깊이가 budget depth {depth}를 넘음"));
                }
                if depth < 2 {
                    return rej("DEPTH_EXCEEDED", format!("budget depth {depth}에서는 관계 탐색 불가"));
                }
                let target = tr["target"].as_str().unwrap();
                let tf = &facts["resources"][target];
                let ta = cx.alias("r");
                let tenv = Env { this: Some((ta.clone(), target.to_string())), ..Default::default() };
                let mut pairs = vec![];
                let mut tout = Map::new();
                for x in tsel {
                    let f = x.as_str().ok_or(Reject { code: "BAD_REQUEST", msg: "traverse select는 문자열".into() })?;
                    if !tr["select"].as_array().unwrap().iter().any(|y| y == f) {
                        return rej("FIELD_NOT_EXPOSED", format!("`{target}.{f}`는 `{res}.{rel}` 경로에 열려 있지 않음. 서버 정의 변경 필요"));
                    }
                    if tf["exposeRead"]["select"][f] == "aggregate" {
                        let aggregate = &tf["aggregates"][f];
                        let (expression, output_type) = aggregate_select_expr(facts, &mut cx, aggregate, target, &ta, &tenv)?;
                        pairs.push(format!("'{f}', {expression}"));
                        tout.insert(f.to_string(), output_type);
                        n_agg += 1;
                        continue;
                    }
                    let col = format!("{ta}.{}", column(facts, target, f).unwrap());
                    let fty = tf["fields"][f]["ty"].as_str().unwrap();
                    let redact = !tf["fieldRead"][f].is_null();
                    let e = if redact {
                        n_fp += 1;
                        format!("CASE WHEN {} THEN {col} END", cx.cond(&tf["fieldRead"][f], &tenv).map_err(internal)?)
                    } else {
                        col
                    };
                    pairs.push(format!("'{f}', {e}"));
                    tout.insert(f.to_string(), out_ty(fty, redact));
                }
                // 대상 resource의 행 정책을 다시 적용한다. 안 보이는 관계는 null이 된다.
                let row = cx.cond(&tf["rowRead"], &tenv).map_err(internal)?;
                let fk = column(facts, res, rel).unwrap();
                let jv = cx.alias("j");
                joins.push(format!(
                    "LEFT JOIN LATERAL (SELECT json_build_object({}) AS v FROM {} {ta} WHERE {ta}.id = {t}.{fk} AND {row}) {jv} ON TRUE",
                    pairs.join(", "),
                    table(target)
                ));
                cols.push((rel.clone(), format!("{jv}.v")));
                out.insert(rel.clone(), json!({ "object": tout, "nullable": true, "reason": "대상 행 정책" }));
                n_trav += 1;
            }
            _ => return rej("BAD_REQUEST", format!("select 항목 형식 오류 {s}")),
        }
    }

    let mut wh = vec![cx.cond(&rf["rowRead"], &env).map_err(internal)?];
    let mut seen_filter = std::collections::HashSet::new();
    if let Some(fl) = o.get("filter") {
        for f in fl.as_array().ok_or(Reject { code: "BAD_REQUEST", msg: "filter 배열".into() })? {
            let fo = f.as_object().ok_or(Reject { code: "BAD_REQUEST", msg: "filter 항목은 객체".into() })?;
            keys(fo, &["field", "op", "value"], "filter")?;
            let (field, op) = (fo.get("field").and_then(Value::as_str).unwrap_or(""), fo.get("op").and_then(Value::as_str).unwrap_or(""));
            let key = format!("{field}.{op}");
            // 같은 조건을 반복하면 요청 크기만 커진다. 허용 목록 항목당 한 번만 받는다(R2B-03).
            if !seen_filter.insert(key.clone()) {
                return rej("DUPLICATE", format!("filter `{key}` 중복"));
            }
            if !ex["filter"].as_array().unwrap().iter().any(|x| x == &json!(key)) {
                return rej("FILTER_NOT_ALLOWED", format!("`{key}` 필터는 계약에 없음(선택 가능 여부와 별개)"));
            }
            // facts가 sema를 거치지 않고 들어와도, 정책으로 가려지는 값이 filter 결과 유무로 새지 않게 막는다.
            if !rf["fieldRead"][field].is_null() {
                return rej("POLICY_FIELD_NOT_FILTERABLE", format!("`{field}`에는 field read 정책이 있어 filter에 쓸 수 없음"));
            }
            let fty = rf["fields"][field]["ty"].as_str().unwrap();
            let col = column(facts, res, field).unwrap();
            let raw = fo.get("value").unwrap_or(&Value::Null);
            // Ref는 traverse와 같게 대상 행 정책을 통과한 경우에만 값이 있는 것으로 본다.
            // 아니면 traverse에서 null로 가린 대상 id를 eq/in으로 확인할 수 있다.
            let visible = match fty.trim_end_matches('?').strip_prefix("Ref<").and_then(|x| x.strip_suffix('>')) {
                Some(target) => {
                    let ta = cx.alias("v");
                    let tenv = Env { this: Some((ta.clone(), target.to_string())), ..Default::default() };
                    let row = cx.cond(&facts["resources"][target]["rowRead"], &tenv).map_err(internal)?;
                    Some(format!("EXISTS (SELECT 1 FROM {} {ta} WHERE {ta}.id = {t}.{col} AND {row})", table(target)))
                }
                None => None,
            };
            let and_visible = |cond: String| match &visible {
                Some(v) => format!("({cond} AND {v})"),
                None => cond,
            };
            if op == "isNull" {
                // 값은 bool 하나. 조건 문자열(IS NULL)은 고정이고 true/false만 매개변수로 비교한다.
                let Value::Bool(b) = raw else { return rej("BAD_VALUE", "isNull 값은 bool") };
                let p = cx.params.bind(Some(b.to_string()), "boolean");
                let is_null = match &visible {
                    Some(v) => format!("({t}.{col} IS NULL OR NOT {v})"),
                    None => format!("({t}.{col} IS NULL)"),
                };
                wh.push(format!("({is_null} = {p})"));
                continue;
            }
            if op == "in" {
                let items = raw.as_array().ok_or(Reject { code: "BAD_VALUE", msg: "in 값은 배열".into() })?;
                if items.is_empty() {
                    return rej("BAD_VALUE", "in 배열이 비었음. 조건이 없으면 필터를 빼야 함");
                }
                if items.len() > MAX_IN_VALUES {
                    return rej("IN_LIST_EXCEEDED", format!("in 배열 {}개 > 상한 {MAX_IN_VALUES}", items.len()));
                }
                // 같은 값은 한 번만 바인딩한다. 비교는 검사를 거친 정규 문자열 기준이라 `10`과 `"10"`은 같은 값이다.
                let mut seen = std::collections::HashSet::new();
                let mut marks = vec![];
                for item in items {
                    let (v, cast) = check_value(facts, fty, item, wire)?;
                    if seen.insert(v.clone()) {
                        marks.push(cx.params.bind(Some(v), cast));
                    }
                }
                wh.push(and_visible(format!("({t}.{col} IN ({}))", marks.join(", "))));
                continue;
            }
            let (v, cast) = check_value(facts, fty, raw, wire)?;
            if op == "prefix" {
                let p = cx.params.bind(Some(v), cast);
                // A prefix is a literal string; SQL wildcard and escape characters keep their meaning as text.
                wh.push(format!("(left({t}.{col}, char_length({p})) = {p})"));
                continue;
            }
            let sop = match op {
                "eq" => "=",
                "gte" => ">=",
                "lte" => "<=",
                "gt" => ">",
                "lt" => "<",
                _ => return rej("FILTER_NOT_ALLOWED", format!("연산 `{op}` 미지원")),
            };
            let p = cx.params.bind(Some(v), cast);
            wh.push(and_visible(format!("({t}.{col} {sop} {p})")));
        }
    }

    let mut order = vec![];
    let mut seen_sort = std::collections::HashSet::new();
    if let Some(sl) = o.get("sort") {
        for s in sl.as_array().ok_or(Reject { code: "BAD_REQUEST", msg: "sort 배열".into() })? {
            let so = s.as_object().ok_or(Reject { code: "BAD_REQUEST", msg: "sort 항목은 객체".into() })?;
            keys(so, &["field", "dir"], "sort")?;
            let f = so.get("field").and_then(Value::as_str).unwrap_or("");
            if !seen_sort.insert(f.to_string()) {
                return rej("DUPLICATE", format!("sort `{f}` 중복"));
            }
            if !ex["sort"].as_array().unwrap().iter().any(|x| x == f) {
                return rej("SORT_NOT_ALLOWED", format!("`{f}` 정렬은 계약에 없음(선택 가능 여부와 별개)"));
            }
            if !rf["fieldRead"][f].is_null() {
                return rej("POLICY_FIELD_NOT_FILTERABLE", format!("`{f}`에는 field read 정책이 있어 sort에 쓸 수 없음"));
            }
            let dir = match so.get("dir").and_then(Value::as_str).unwrap_or("asc") {
                "asc" => "ASC",
                "desc" => "DESC",
                d => return rej("BAD_REQUEST", format!("정렬 방향 `{d}`")),
            };
            order.push(format!("{t}.{} {dir}", column(facts, res, f).unwrap()));
        }
    }
    // 같은 값이 많을 때 페이지가 흔들리지 않게 항상 id를 마지막 정렬 키로 둔다.
    order.push(format!("{t}.id ASC"));

    let max_rows = budget["rows"].as_i64().unwrap();
    let limit = match o.get("limit") {
        None => max_rows,
        Some(Value::Number(n)) if n.as_i64().is_some_and(|x| x >= 1) => n.as_i64().unwrap(),
        Some(v) => return rej("BAD_VALUE", format!("limit {v}")),
    };
    if limit > max_rows {
        return rej("ROWS_EXCEEDED", format!("limit {limit} > budget rows {max_rows}"));
    }
    let cost = estimate(limit, n_trav, n_agg, n_fp);
    let max_cost = budget["cost"].as_i64().unwrap();
    if cost > max_cost {
        return rej("COST_EXCEEDED", format!("추정 비용 {cost} > budget cost {max_cost}"));
    }
    let lp = cx.params.bind(Some(limit.to_string()), "bigint");
    let obj: Vec<String> = cols.iter().map(|(k, v)| format!("'{k}', {v}")).collect();
    let sql = format!(
        "SELECT json_build_object({})::text FROM {} {t} {} WHERE {} ORDER BY {} LIMIT {lp}",
        obj.join(", "),
        table(res),
        joins.join(" "),
        wh.join(" AND "),
        order.join(", ")
    );
    Ok(Plan {
        sql,
        params: cx.params.values,
        deadline_ms: budget["deadlineMs"].as_u64().unwrap(),
        output_type: json!({ "rows": out, "maxRows": limit }),
        cost,
        null_is_denied: false,
        deps: vec![],
    })
}

fn plan_aggregate(facts: &Value, o: &Map<String, Value>, caller: &Caller, wire: crate::id_wire::IdWire) -> Result<Plan, Reject> {
    keys(o, &["aggregate", "input"], "aggregate 요청")?;
    let name = o["aggregate"].as_str().unwrap_or("");
    let (res, an) = name.split_once('.').ok_or(Reject { code: "BAD_REQUEST", msg: "aggregate는 `Resource.name`".into() })?;
    let rf = &facts["resources"][res];
    if !rf["exposeAggregates"].as_array().is_some_and(|v| v.iter().any(|x| x == an)) {
        return rej("NOT_EXPOSED", format!("`{name}`는 노출된 집계가 아님"));
    }
    let a = &rf["aggregates"][an];
    let mut cx = Ctx::new(facts, caller.actor_id, &caller.now);
    let empty = Map::new();
    let given = o.get("input").and_then(Value::as_object).unwrap_or(&empty);
    let mut input = HashMap::new();
    let declared = a["input"].as_array().unwrap();
    for k in given.keys() {
        if !declared.iter().any(|d| d[0] == json!(k)) {
            return rej("UNKNOWN_KEY", format!("집계 입력 `{k}` 없음"));
        }
    }
    for d in declared {
        let (n, ty) = (d[0].as_str().unwrap(), d[1].as_str().unwrap());
        let v = given.get(n).ok_or(Reject { code: "BAD_REQUEST", msg: format!("입력 `{n}` 필요") })?;
        let (s, cast) = check_value(facts, ty, v, wire)?;
        let p = cx.params.bind(Some(s), cast);
        let h = match crate::sqlgen::ref_target(ty) {
            Some(t) => Handle::Id { sql: p, res: t.to_string() },
            None => Handle::Scalar(p),
        };
        input.insert(n.to_string(), h);
    }
    let sa = &a["sourceAccess"];
    let acc = &facts["accesses"][sa["ref"].as_str().unwrap()];
    if acc["kind"] != "guard" {
        return rej("NOT_EXPOSED", "단독 집계는 guard access만 지원");
    }
    let outer = Env { this: None, vars: HashMap::new(), input: input.clone() };
    let mut genv = Env::default();
    for (p, arg) in acc["params"].as_array().unwrap().iter().zip(sa["args"].as_array().unwrap()) {
        let h = cx.handle(arg, &outer).map_err(internal)?;
        genv.vars.insert(p[0].as_str().unwrap().to_string(), h);
    }
    let guard = cx.cond(&acc["body"], &genv).map_err(internal)?;
    let src = a["source"].as_str().unwrap();
    let al = cx.alias("s");
    let wenv = Env { this: Some((al.clone(), src.to_string())), vars: HashMap::new(), input };
    let w = if a["where"].is_null() { "TRUE".into() } else { cx.cond(&a["where"], &wenv).map_err(internal)? };
    // guard가 거짓이면 NULL을 돌려 ACCESS_DENIED로 바꾼다. 없는 동아리와 남의 동아리를 같은 결과로 만든다.
    let sql = format!("SELECT (CASE WHEN {guard} THEN (SELECT {} FROM {} {al} WHERE {w}) END)::text", measure(facts, a, &al)?, table(src));
    Ok(Plan {
        sql,
        params: cx.params.values,
        // C의 단독 집계 노출에는 budget이 없다. spike는 고정 2s를 쓰고 이 공백을 결과 문서에 남긴다.
        deadline_ms: 2000,
        output_type: json!({ "value": out_ty(a["ty"].as_str().unwrap(), false) }),
        cost: 1,
        null_is_denied: true,
        deps: vec![],
    })
}
