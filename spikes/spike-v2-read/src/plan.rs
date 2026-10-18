//! 호출자 정형 읽기 요청을 facts의 허용 범위로 검증하고 매개변수 SQL 하나로 만든다.
use crate::sqlgen::{self, column, Ctx, Dialect, Env, Handle};
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
    Reject { code: "WIRE.INTERNAL", msg: e }
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
    deps_of_in(Dialect::Postgres, facts, sql)
}

pub fn deps_of_in(dialect: Dialect, facts: &Value, sql: &str) -> Vec<String> {
    let mut out: Vec<String> = facts["resources"]
        .as_object()
        .unwrap()
        .keys()
        .filter(|r| {
            let t = dialect.table(r);
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

/// `contains` 값의 최대 문자 수(바이트 아님). 기존 filter 값에 길이 관례가 없어 새로 정했다. 요청·SQL 매개변수 크기를 막는다.
pub const MAX_CONTAINS_CHARS: usize = 200;

pub struct Caller {
    pub actor_id: Option<i64>,
    pub now: String,
}

fn keys(o: &Map<String, Value>, allowed: &[&str], what: &str) -> Result<(), Reject> {
    for k in o.keys() {
        if !allowed.contains(&k.as_str()) {
            return rej("WIRE.UNKNOWN_KEY", format!("{what}에 알 수 없는 키 `{k}`. 허용: {}", allowed.join(", ")));
        }
    }
    Ok(())
}

/// 거부 메시지에 붙이는 허용 목록. 계약(facts)에서 계산한 이름만 받으므로 이미 공개된 이름이다. 목록이 길어져도 메시지가 커지지 않게 자른다.
fn allowed_names<'a>(names: impl Iterator<Item = &'a str>) -> String {
    const SHOWN: usize = 40;
    let all: Vec<&str> = names.collect();
    if all.is_empty() {
        return "없음".into();
    }
    let mut text = all.iter().take(SHOWN).copied().collect::<Vec<_>>().join(", ");
    if all.len() > SHOWN {
        text.push_str(&format!(" 외 {}개", all.len() - SHOWN));
    }
    text
}

/// 비용 추정은 자리표시 휴리스틱이다: 행 상한 × (1 + 관계 + 집계 + 필드 정책 수). 실제 DB 비용과 같지 않다.
/// offset은 건너뛰는 행을 읽는 비용이라 행 수만큼 더한다(관계·집계 가중은 반환 행에만 곱한다).
fn estimate(limit: i64, offset: i64, traverse: usize, aggs: usize, field_policies: usize) -> i64 {
    limit * (1 + traverse as i64 + aggs as i64 + field_policies as i64) + offset
}

/// select 종류가 대상 행 정책으로 가려지는 참조인지(sema의 `refVisible*`). `refIds`로 선언한 참조는 해당하지 않는다.
fn ref_guarded(kind: Option<&str>) -> bool {
    matches!(kind, Some("refVisible" | "refVisibleWithPolicy"))
}

/// 참조 select의 가시성 가드(창시자 결정 2026-10-13, 보안 감사 O-01). 호출자가 대상 행을 읽을 수 없으면 원시 FK id 대신 null이다.
/// traverse·Ref filter가 안 보이는 대상을 null로 취급하는 것과 같은 규칙이라, select로 숨은 대상의 id를 확인할 수 없다.
fn guard_ref(facts: &Value, cx: &mut Ctx<'_>, ty: &str, col: &str, value: &str) -> Result<String, Reject> {
    let target = sqlgen::ref_target(ty).ok_or_else(|| internal(format!("참조 가드 대상 아님 `{ty}`")))?;
    let ta = cx.alias("v");
    let tenv = Env { this: Some((ta.clone(), target.to_string())), ..Default::default() };
    let row = cx.cond(&facts["resources"][target]["rowRead"], &tenv).map_err(internal)?;
    Ok(format!("CASE WHEN EXISTS (SELECT 1 FROM {} {ta} WHERE {} = {col} AND {row}) THEN {value} END", cx.table(target), cx.qualified(&ta, "id")))
}

fn out_ty(ty: &str, redactable: bool) -> Value {
    let t = ty.trim_end_matches('?');
    let nullable = ty.ends_with('?') || redactable;
    json!({ "ty": t, "nullable": nullable, "redactable": redactable })
}

/// count는 행 수, sum은 빈 집합이 0, min/max는 빈 집합이 NULL이다(sema가 min/max를 nullable 행별 집계로 제한).
/// count는 sourceAccess 선언대로 원본 행 정책 없이 센다. sum/min/max는 값을 내보내므로 원본 행 정책을
/// FILTER로 다시 건다. 아니면 호출자에게 안 보이는 행의 값이 합계·최솟값으로 그대로 나온다.
fn measure(facts: &Value, cx: &mut Ctx<'_>, aggregate: &Value, alias: &str) -> Result<String, Reject> {
    let source = aggregate["source"].as_str().unwrap();
    let field = aggregate["releaseField"].as_str();
    let col = |f: &str| column(facts, source, f).ok_or_else(|| internal(format!("집계 필드 `{f}` 없음")));
    let release = aggregate["release"].as_str();
    if matches!(release, Some("count") | None) && field.is_none() {
        return Ok("count(*)".into());
    }
    let env = Env { this: Some((alias.to_string(), source.to_string())), ..Default::default() };
    let visible = cx.cond(&facts["resources"][source]["rowRead"], &env).map_err(internal)?;
    match (release, field) {
        // bigint 합은 numeric이 된다. 범위를 넘으면 캐스트가 실패해 잘린 값 대신 오류가 난다.
        (Some("sum"), Some(f)) if cx.dialect.is_mysql() => {
            // MySQL은 FILTER가 없고 DECIMAL을 SIGNED로 줄일 때 범위 밖 값을 조용히 자른다. 합은 DECIMAL 그대로 내보내 정규화 단계가 i64를 확인한다.
            Ok(format!("COALESCE(SUM(CASE WHEN {visible} THEN {} END), 0)", cx.qualified(alias, &col(f)?)))
        }
        (Some(op @ ("min" | "max")), Some(f)) if cx.dialect.is_mysql() => {
            Ok(format!("{op}(CASE WHEN {visible} THEN {} END)", cx.qualified(alias, &col(f)?)))
        }
        (Some("sum"), Some(f)) => Ok(format!("coalesce(sum({alias}.{}) FILTER (WHERE {visible}), 0)::bigint", col(f)?)),
        (Some(op @ ("min" | "max")), Some(f)) => Ok(format!("{op}({alias}.{}) FILTER (WHERE {visible})", col(f)?)),
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
    let mut where_sql = format!("{} = {}", cx.qualified(&source_alias, &source_group_key), cx.qualified(owner_alias, &owner_id));
    if !aggregate["where"].is_null() {
        let source_env = Env { this: Some((source_alias.clone(), source.to_string())), ..Default::default() };
        where_sql = format!("{where_sql} AND {}", cx.cond(&aggregate["where"], &source_env).map_err(internal)?);
    }
    // 집계 원본은 원본 행 정책을 자동 상속하지 않고 sourceAccess로만 읽는다.
    let count = format!("(SELECT {} FROM {} {source_alias} WHERE {where_sql})", measure(facts, cx, aggregate, &source_alias)?, cx.table(source));
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
    if (wire != crate::id_wire::IdWire::Legacy || crate::id_wire::id_cast(facts, base) == "uuid")
        && (base.starts_with("Id<") || base.starts_with("Ref<"))
    {
        return crate::id_wire::parse_typed_id(facts, base, v, wire);
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

/// JSON 객체 생성 식. PostgreSQL은 키/값 인수를 번갈아 받고, MySQL의 `JSON_OBJECT`도 같은 모양이다.
fn json_object(dialect: Dialect, pairs: &[String]) -> String {
    match dialect {
        Dialect::Postgres => format!("json_build_object({})", pairs.join(", ")),
        Dialect::MySql => format!("JSON_OBJECT({})", pairs.join(", ")),
    }
}

/// PostgreSQL은 ASC에서 NULL이 마지막, DESC에서 처음이다. MySQL은 반대라 같은 순서가 되도록 NULL 여부를 앞 정렬 키로 둔다.
fn order_term(dialect: Dialect, column_sql: &str, desc: bool) -> String {
    let dir = if desc { "DESC" } else { "ASC" };
    match dialect {
        Dialect::Postgres => format!("{column_sql} {dir}"),
        Dialect::MySql => format!("({column_sql} IS NULL) {dir}, {column_sql} {dir}"),
    }
}

pub fn plan_operation_authorization(facts: &Value, name: &str, input: &Value, caller: &Caller, wire: crate::id_wire::IdWire) -> Result<Plan, Reject> {
    plan_operation_authorization_in(Dialect::Postgres, facts, name, input, caller, wire)
}

pub fn plan_operation_authorization_in(
    dialect: Dialect,
    facts: &Value,
    name: &str,
    input: &Value,
    caller: &Caller,
    wire: crate::id_wire::IdWire,
) -> Result<Plan, Reject> {
    let operation = &facts["operations"][name];
    let declared = operation["input"].as_array().ok_or(Reject { code: "WIRE.NOT_EXPOSED", msg: "operation 계약 없음".into() })?;
    let given = input.as_object().ok_or(Reject { code: "WIRE.BAD_VALUE", msg: "operation 입력은 객체".into() })?;
    let mut cx = Ctx::new(facts, caller.actor_id, &caller.now).with_dialect(dialect);
    let mut env = Env::default();
    for key in given.keys() {
        if !declared.iter().any(|field| field[0] == *key) {
            return rej("WIRE.BAD_VALUE", "operation에 선언되지 않은 입력");
        }
    }
    for field in declared {
        let (name, ty) = (field[0].as_str().unwrap(), field[1].as_str().unwrap());
        let value = given.get(name).ok_or(Reject { code: "WIRE.BAD_VALUE", msg: "operation 입력 누락".into() })?;
        crate::scalar::validate_range(field, value, "WIRE.BAD_VALUE")?;
        let bound = if value.is_null() && ty.ends_with('?') {
            "NULL".to_string()
        } else {
            let (value, cast) = check_value(facts, ty, value, wire)?;
            cx.params.bind(Some(value), cast)
        };
        let handle = match sqlgen::ref_target(ty) {
            Some(target) => Handle::Id { sql: bound, res: target.to_string() },
            None => Handle::Scalar(bound),
        };
        env.input.insert(name.to_string(), handle);
    }
    let allow = cx.cond(&operation["allow"], &env).map_err(internal)?;
    // Even unused inputs need a text parameter type in PostgreSQL's parse phase. MySQL types a `?` by its use, so it needs no such row.
    let bindings = if dialect.is_mysql() {
        String::new()
    } else {
        (1..=cx.params.values.len()).map(|n| format!("${n}::text AS p{n}")).collect::<Vec<_>>().join(",")
    };
    let source = if bindings.is_empty() { String::new() } else { format!(" FROM (SELECT {bindings}) _aip_input") };
    let sql = format!("SELECT COALESCE(({allow}),false){source}");
    let (sql, params) = cx.params.finish(sql);
    Ok(Plan { deps: deps_of_in(dialect, facts, &sql), sql, params, deadline_ms: 2000, output_type: json!({}), cost: 1, null_is_denied: false })
}

pub fn plan_read(facts: &Value, req: &Value, caller: &Caller) -> Result<Plan, Reject> {
    plan_read_with_wire(facts, req, caller, crate::id_wire::IdWire::Legacy)
}

pub fn plan_read_with_wire(facts: &Value, req: &Value, caller: &Caller, wire: crate::id_wire::IdWire) -> Result<Plan, Reject> {
    plan_read_in(Dialect::Postgres, facts, req, caller, wire)
}

pub fn plan_read_in(dialect: Dialect, facts: &Value, req: &Value, caller: &Caller, wire: crate::id_wire::IdWire) -> Result<Plan, Reject> {
    let mut p = plan_read_inner(dialect, facts, req, caller, wire)?;
    p.deps = deps_of_in(dialect, facts, &p.sql);
    Ok(p)
}

const MAX_OR_CONDITIONS: usize = 10;

fn compile_filter_item(
    dialect: Dialect,
    facts: &Value,
    res: &str,
    rf: &Value,
    ex: &Value,
    cx: &mut Ctx<'_>,
    t: &str,
    fo: &Map<String, Value>,
    wire: crate::id_wire::IdWire,
) -> Result<(String, String), Reject> {
    keys(fo, &["field", "op", "value"], "filter")?;
    let (field, op) = (fo.get("field").and_then(Value::as_str).unwrap_or(""), fo.get("op").and_then(Value::as_str).unwrap_or(""));
    let key = format!("{field}.{op}");
    // 같은 조건을 반복하면 요청 크기만 커진다. 허용 목록 항목당 한 번만 받는다(R2B-03).
    if !ex["filter"].as_array().unwrap().iter().any(|x| x == &json!(key)) {
        let allowed = allowed_names(ex["filter"].as_array().unwrap().iter().filter_map(Value::as_str));
        return rej("WIRE.FILTER_NOT_ALLOWED", format!("`{key}` 필터는 계약에 없음(선택 가능 여부와 별개). 허용: {allowed}"));
    }
    // facts가 sema를 거치지 않고 들어와도, 정책으로 가려지는 값이 filter 결과 유무로 새지 않게 막는다.
    if !rf["fieldRead"][field].is_null() {
        return rej("WIRE.POLICY_FIELD_NOT_FILTERABLE", format!("`{field}`에는 field read 정책이 있어 filter에 쓸 수 없음"));
    }
    let fty = rf["fields"][field]["ty"].as_str().unwrap();
    let col = column(facts, res, field).unwrap();
    let tcol = cx.qualified(t, &col);
    let raw = fo.get("value").unwrap_or(&Value::Null);
    // Ref는 traverse와 같게 대상 행 정책을 통과한 경우에만 값이 있는 것으로 본다.
    // 아니면 traverse에서 null로 가린 대상 id를 eq/in으로 확인할 수 있다.
    let visible = match fty.trim_end_matches('?').strip_prefix("Ref<").and_then(|x| x.strip_suffix('>')) {
        Some(target) => {
            let ta = cx.alias("v");
            let tenv = Env { this: Some((ta.clone(), target.to_string())), ..Default::default() };
            let row = cx.cond(&facts["resources"][target]["rowRead"], &tenv).map_err(internal)?;
            Some(format!("EXISTS (SELECT 1 FROM {} {ta} WHERE {} = {tcol} AND {row})", cx.table(target), cx.qualified(&ta, "id")))
        }
        None => None,
    };
    let and_visible = |cond: String| match &visible {
        Some(v) => format!("({cond} AND {v})"),
        None => cond,
    };
    if op == "isNull" {
        // 값은 bool 하나. 조건 문자열(IS NULL)은 고정이고 true/false만 매개변수로 비교한다.
        let Value::Bool(b) = raw else { return rej("WIRE.BAD_VALUE", "isNull 값은 bool") };
        let p = cx.params.bind(Some(b.to_string()), "boolean");
        let is_null = match &visible {
            Some(v) => format!("({tcol} IS NULL OR NOT {v})"),
            None => format!("({tcol} IS NULL)"),
        };
        return Ok((key, format!("({is_null} = {p})")));
    }
    if op == "in" {
        let items = raw.as_array().ok_or(Reject { code: "WIRE.BAD_VALUE", msg: "in 값은 배열".into() })?;
        if items.is_empty() {
            return rej("WIRE.BAD_VALUE", "in 배열이 비었음. 조건이 없으면 필터를 빼야 함");
        }
        if items.len() > MAX_IN_VALUES {
            return rej("WIRE.IN_LIST_EXCEEDED", format!("in 배열 {}개 > 상한 {MAX_IN_VALUES}", items.len()));
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
        return Ok((key, and_visible(format!("({tcol} IN ({}))", marks.join(", ")))));
    }
    let (v, cast) = check_value(facts, fty, raw, wire)?;
    if op == "contains" {
        let Value::String(text) = raw else { return rej("WIRE.BAD_VALUE", "contains 값은 문자열") };
        let n = text.chars().count();
        if n > MAX_CONTAINS_CHARS {
            return rej("WIRE.VALUE_TOO_LONG", format!("contains 값 {n}자 > 상한 {MAX_CONTAINS_CHARS}자"));
        }
        let p = cx.params.bind(Some(v), cast);
        // Preserve the original contract: wildcards and backslashes are literal. PostgreSQL strpos
        // is case-sensitive; MySQL LOCATE follows the column's collation, as it did before WBS 7.2.3.
        let cond = if dialect.is_mysql() { format!("(LOCATE({p}, {tcol}) > 0)") } else { format!("(strpos({tcol}, {p}) > 0)") };
        return Ok((key, and_visible(cond)));
    }
    if op == "like" {
        let Value::String(text) = raw else { return rej("WIRE.BAD_VALUE", "like 값은 문자열") };
        let n = text.chars().count();
        if n > MAX_CONTAINS_CHARS {
            return rej("WIRE.VALUE_TOO_LONG", format!("like 값 {n}자 > 상한 {MAX_CONTAINS_CHARS}자"));
        }
        // This is the source application's LIKE '%keyword%' contract: %, _ and backslash retain
        // native SQL LIKE behavior. PostgreSQL ILIKE mirrors FaceReview's MySQL ai_ci collation.
        let p = cx.params.bind(Some(format!("%{v}%")), cast);
        let cond = if dialect.is_mysql() { format!("({tcol} LIKE {p})") } else { format!("({tcol} ILIKE {p})") };
        return Ok((key, and_visible(cond)));
    }
    if op == "prefix" {
        // contains와 같은 상한. 긴 값은 검색어가 아니라 비용만 키운다.
        let n = v.chars().count();
        if n > MAX_CONTAINS_CHARS {
            return rej("WIRE.VALUE_TOO_LONG", format!("prefix 값 {n}자 > 상한 {MAX_CONTAINS_CHARS}자"));
        }
        let p = cx.params.bind(Some(v), cast);
        // A prefix is a literal string; SQL wildcard and escape characters keep their meaning as text.
        let cond =
            if dialect.is_mysql() { format!("(LEFT({tcol}, CHAR_LENGTH({p})) = {p})") } else { format!("(left({tcol}, char_length({p})) = {p})") };
        return Ok((key, and_visible(cond)));
    }
    let sop = match op {
        "eq" => "=",
        "gte" => ">=",
        "lte" => "<=",
        "gt" => ">",
        "lt" => "<",
        _ => return rej("WIRE.FILTER_NOT_ALLOWED", format!("연산 `{op}` 미지원")),
    };
    let p = cx.params.bind(Some(v), cast);
    Ok((key, and_visible(format!("({tcol} {sop} {p})"))))
}

fn compile_filters(
    dialect: Dialect,
    facts: &Value,
    res: &str,
    rf: &Value,
    ex: &Value,
    cx: &mut Ctx<'_>,
    t: &str,
    fl: &Value,
    wire: crate::id_wire::IdWire,
) -> Result<Vec<String>, Reject> {
    let mut wh = vec![];
    let mut seen_filter = std::collections::HashSet::new();
    for f in fl.as_array().ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "filter 배열".into() })? {
        let fo = f.as_object().ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "filter 항목은 객체".into() })?;
        if fo.contains_key("or") {
            keys(fo, &["or"], "filter OR 조건")?;
            let or_items = fo["or"].as_array().ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "or 값은 배열".into() })?;
            if or_items.is_empty() {
                return rej("WIRE.BAD_VALUE", "or 배열이 비었음. 조건이 없으면 필터를 빼야 함");
            }
            if or_items.len() > MAX_OR_CONDITIONS {
                return rej("WIRE.OR_LIST_EXCEEDED", format!("or 조건 수 {}개 > 상한 {MAX_OR_CONDITIONS}", or_items.len()));
            }
            let mut or_parts = vec![];
            let mut or_seen = std::collections::HashSet::new();
            for item in or_items {
                let item_obj = item.as_object().ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "or 항목은 객체".into() })?;
                if item_obj.contains_key("or") {
                    return rej("WIRE.BAD_REQUEST", "중첩된 or 조건은 미지원");
                }
                let (k, cond) = compile_filter_item(dialect, facts, res, rf, ex, cx, t, item_obj, wire)?;
                let item_sig = serde_json::to_string(item_obj).unwrap_or_default();
                if !or_seen.insert(item_sig) {
                    return rej("WIRE.DUPLICATE", format!("or 항목 중복: {k}"));
                }
                or_parts.push(cond);
            }
            wh.push(format!("({})", or_parts.join(" OR ")));
        } else {
            let (key, cond) = compile_filter_item(dialect, facts, res, rf, ex, cx, t, fo, wire)?;
            if !seen_filter.insert(key.clone()) {
                return rej("WIRE.DUPLICATE", format!("filter `{key}` 중복"));
            }
            wh.push(cond);
        }
    }
    Ok(wh)
}

fn plan_read_inner(dialect: Dialect, facts: &Value, req: &Value, caller: &Caller, wire: crate::id_wire::IdWire) -> Result<Plan, Reject> {
    let o = req.as_object().ok_or_else(|| Reject { code: "WIRE.BAD_REQUEST", msg: "요청은 객체".into() })?;
    if o.contains_key("aggregate") {
        return plan_aggregate(dialect, facts, o, caller, wire);
    }
    // offset 키는 정의 budget이 opt-in(maxOffset)했을 때만 허용한다. 선언이 없으면 알 수 없는 키다.
    let max_offset = facts["resources"][o.get("read").and_then(Value::as_str).unwrap_or("")]["exposeRead"]["budget"]["maxOffset"].as_i64();
    // after(keyset cursor)도 같은 방식으로 budget `cursor` 선언이 있을 때만 키를 연다.
    let cursor_on = facts["resources"][o.get("read").and_then(Value::as_str).unwrap_or("")]["exposeRead"]["budget"]["cursor"] == true;
    let mut allowed: Vec<&str> = vec!["read", "select", "filter", "sort", "limit"];
    if max_offset.is_some() {
        allowed.push("offset");
    }
    if cursor_on {
        allowed.push("after");
    }
    keys(o, &allowed, "read 요청")?;
    let res = o.get("read").and_then(Value::as_str).ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "read 필요".into() })?;
    let rf = &facts["resources"][res];
    if rf.is_null() {
        return rej("WIRE.UNKNOWN_RESOURCE", format!("`{res}` 없음"));
    }
    let ex = &rf["exposeRead"];
    if ex.is_null() {
        return rej("WIRE.NOT_EXPOSED", format!("`{res}`는 읽기 노출이 없음"));
    }
    if ex["rootQueryable"] != true {
        return rej("WIRE.NOT_ROOT_QUERYABLE", format!("`{res}`는 관계 대상 전용(budget 없음)"));
    }
    let budget = &ex["budget"];
    let mut cx = Ctx::new(facts, caller.actor_id, &caller.now).with_dialect(dialect);
    let t = "t".to_string();
    let env = Env { this: Some((t.clone(), res.to_string())), ..Default::default() };

    let sel = o.get("select").and_then(Value::as_array).ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "select 배열 필요".into() })?;
    if sel.is_empty() {
        return rej("WIRE.BAD_REQUEST", "select가 비었음");
    }
    let (mut cols, mut joins, mut out) = (vec![], vec![], Map::new());
    let (mut n_trav, mut n_agg, mut n_fp) = (0usize, 0usize, 0usize);
    // 1:N traverse가 부모 행 하나당 더하는 비용 합(자식 상한 x (1 + 자식 필드 정책 수)). 부모 limit를 곱해 estimate에 더한다.
    let mut many_cost = 0i64;
    let mut seen_sel = std::collections::HashSet::new();
    for s in sel {
        let key = match s {
            Value::String(f) => f.clone(),
            Value::Object(m) => m.keys().next().cloned().unwrap_or_default(),
            _ => String::new(),
        };
        if !seen_sel.insert(key.clone()) {
            return rej("WIRE.DUPLICATE", format!("select `{key}` 중복"));
        }
        match s {
            Value::String(f) => {
                let kind = ex["select"][f].as_str();
                match kind {
                    None => return rej("WIRE.FIELD_NOT_EXPOSED", format!("`{res}.{f}`는 계약에 열려 있지 않음. 서버 정의의 expose 변경이 필요")),
                    Some("aggregate") => {
                        let a = &rf["aggregates"][f];
                        let (expr, output_type) = aggregate_select_expr(facts, &mut cx, a, res, &t, &env)?;
                        cols.push((f.clone(), expr));
                        out.insert(f.clone(), output_type);
                        n_agg += 1;
                    }
                    Some(k) => {
                        let col = cx.qualified(&t, &column(facts, res, f).unwrap());
                        let fty = rf["fields"][f]["ty"].as_str().unwrap();
                        let guarded = ref_guarded(Some(k));
                        let policy = matches!(k, "fieldWithPolicy" | "refVisibleWithPolicy");
                        let mut expr = col.clone();
                        if guarded {
                            n_fp += 1;
                            expr = guard_ref(facts, &mut cx, fty, &col, &expr)?;
                        }
                        if policy {
                            n_fp += 1;
                            let c = cx.cond(&rf["fieldRead"][f], &env).map_err(internal)?;
                            expr = format!("CASE WHEN {c} THEN {expr} END");
                        }
                        cols.push((f.clone(), cx.json_scalar(fty, &expr)));
                        out.insert(f.clone(), out_ty(fty, policy || guarded));
                    }
                }
            }
            Value::Object(m) if m.len() == 1 => {
                let (rel, sub) = m.iter().next().unwrap();
                let tr = &ex["traverse"][rel];
                if tr.is_null() && !ex["traverseMany"][rel].is_null() {
                    let (join, col_expr, ty, per_parent) = plan_traverse_many(facts, &mut cx, res, rel, sub, &t, budget)?;
                    joins.push(join);
                    cols.push((rel.clone(), col_expr));
                    out.insert(rel.clone(), ty);
                    many_cost += per_parent;
                    continue;
                }
                if tr.is_null() {
                    return rej("WIRE.TRAVERSE_NOT_ALLOWED", format!("`{res}.{rel}` 관계 탐색이 계약에 없음"));
                }
                let so = sub.as_object().ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "traverse 값은 객체".into() })?;
                keys(so, &["select"], "traverse")?;
                let tsel =
                    so.get("select").and_then(Value::as_array).ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "traverse select 필요".into() })?;
                let depth = budget["depth"].as_i64().unwrap_or(1);
                if tsel.iter().any(|x| x.is_object()) {
                    return rej("WIRE.DEPTH_EXCEEDED", format!("관계 깊이가 budget depth {depth}를 넘음"));
                }
                if depth < 2 {
                    return rej("WIRE.DEPTH_EXCEEDED", format!("budget depth {depth}에서는 관계 탐색 불가"));
                }
                let target = tr["target"].as_str().unwrap();
                let tf = &facts["resources"][target];
                let ta = cx.alias("r");
                let tenv = Env { this: Some((ta.clone(), target.to_string())), ..Default::default() };
                let mut pairs = vec![];
                let mut tout = Map::new();
                let mut seen_target = std::collections::HashSet::new();
                for x in tsel {
                    let f = x.as_str().ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "traverse select는 문자열".into() })?;
                    // 같은 필드를 반복하면 정책 없는 필드는 비용에 안 잡힌 채 SQL만 부풀고 json_build_object 인수 한도에 걸린다.
                    if !seen_target.insert(f) {
                        return rej("WIRE.DUPLICATE", format!("select `{f}` 중복"));
                    }
                    if !tr["select"].as_array().unwrap().iter().any(|y| y == f) {
                        return rej("WIRE.FIELD_NOT_EXPOSED", format!("`{target}.{f}`는 `{res}.{rel}` 경로에 열려 있지 않음. 서버 정의 변경 필요"));
                    }
                    if tf["exposeRead"]["select"][f] == "aggregate" {
                        let aggregate = &tf["aggregates"][f];
                        let (expression, output_type) = aggregate_select_expr(facts, &mut cx, aggregate, target, &ta, &tenv)?;
                        pairs.push(format!("'{f}', {expression}"));
                        tout.insert(f.to_string(), output_type);
                        n_agg += 1;
                        continue;
                    }
                    let col = cx.qualified(&ta, &column(facts, target, f).unwrap());
                    let fty = tf["fields"][f]["ty"].as_str().unwrap();
                    let redact = !tf["fieldRead"][f].is_null();
                    let guarded = ref_guarded(tf["exposeRead"]["select"][f].as_str());
                    let mut e = col.clone();
                    if guarded {
                        n_fp += 1;
                        e = guard_ref(facts, &mut cx, fty, &col, &e)?;
                    }
                    if redact {
                        n_fp += 1;
                        e = format!("CASE WHEN {} THEN {e} END", cx.cond(&tf["fieldRead"][f], &tenv).map_err(internal)?);
                    }
                    pairs.push(format!("'{f}', {}", cx.json_scalar(fty, &e)));
                    tout.insert(f.to_string(), out_ty(fty, redact || guarded));
                }
                // 대상 resource의 행 정책을 다시 적용한다. 안 보이는 관계는 null이 된다.
                let row = cx.cond(&tf["rowRead"], &tenv).map_err(internal)?;
                // Traversal must not reveal a relation whose source reference
                // field is redacted, even when the target row itself is public.
                let origin = if rf["fieldRead"][rel].is_null() {
                    "TRUE".to_string()
                } else {
                    n_fp += 1;
                    cx.cond(&rf["fieldRead"][rel], &env).map_err(internal)?
                };
                let fk = column(facts, res, rel).unwrap();
                let jv = cx.alias("j");
                joins.push(format!(
                    "LEFT JOIN LATERAL (SELECT {} AS v FROM {} {ta} WHERE {} = {} AND ({origin}) AND ({row})) {jv} ON TRUE",
                    json_object(dialect, &pairs),
                    cx.table(target),
                    cx.qualified(&ta, "id"),
                    cx.qualified(&t, &fk)
                ));
                cols.push((rel.clone(), format!("{jv}.v")));
                out.insert(rel.clone(), json!({ "object": tout, "nullable": true, "reason": "대상 행 정책" }));
                n_trav += 1;
            }
            _ => return rej("WIRE.BAD_REQUEST", format!("select 항목 형식 오류 {s}")),
        }
    }
    let mut wh = vec![cx.cond(&rf["rowRead"], &env).map_err(internal)?];
    if let Some(fl) = o.get("filter") {
        let f_wh = compile_filters(dialect, facts, res, rf, ex, &mut cx, &t, fl, wire)?;
        wh.extend(f_wh);
    }

    let mut order = vec![];
    // keyset 경계 계산용: (필드, 내림차순 여부). 마지막에 id 타이브레이커가 없으면 붙인다.
    let mut sort_keys: Vec<(String, bool)> = vec![];
    let mut seen_sort = std::collections::HashSet::new();
    if let Some(sl) = o.get("sort") {
        for s in sl.as_array().ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "sort 배열".into() })? {
            let so = s.as_object().ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "sort 항목은 객체".into() })?;
            keys(so, &["field", "dir"], "sort")?;
            let f = so.get("field").and_then(Value::as_str).unwrap_or("");
            if !seen_sort.insert(f.to_string()) {
                return rej("WIRE.DUPLICATE", format!("sort `{f}` 중복"));
            }
            if !ex["sort"].as_array().unwrap().iter().any(|x| x == f) {
                let allowed = allowed_names(ex["sort"].as_array().unwrap().iter().filter_map(Value::as_str));
                return rej("WIRE.SORT_NOT_ALLOWED", format!("`{f}` 정렬은 계약에 없음(선택 가능 여부와 별개). 허용: {allowed}"));
            }
            if !rf["fieldRead"][f].is_null() {
                return rej("WIRE.POLICY_FIELD_NOT_FILTERABLE", format!("`{f}`에는 field read 정책이 있어 sort에 쓸 수 없음"));
            }
            let dir = match so.get("dir").and_then(Value::as_str).unwrap_or("asc") {
                "asc" => "ASC",
                "desc" => "DESC",
                d => return rej("WIRE.BAD_REQUEST", format!("정렬 방향 `{d}`")),
            };
            order.push(order_term(dialect, &cx.qualified(&t, &column(facts, res, f).unwrap()), dir == "DESC"));
            sort_keys.push((f.to_string(), dir == "DESC"));
        }
    }
    // 같은 값이 많을 때 페이지가 흔들리지 않게 항상 id를 마지막 정렬 키로 둔다.
    order.push(format!("{} ASC", cx.qualified(&t, "id")));
    if !sort_keys.iter().any(|(f, _)| f == "id") {
        sort_keys.push(("id".to_string(), false));
    }

    if let Some(after) = o.get("after") {
        if o.contains_key("offset") {
            return rej("WIRE.CURSOR_WITH_OFFSET", "after와 offset은 함께 쓸 수 없음");
        }
        let am = after.as_object().ok_or(Reject { code: "WIRE.BAD_CURSOR", msg: "after는 객체".into() })?;
        // 키 집합은 이 요청의 sort 필드 + id와 정확히 같아야 한다. 호출자가 다른 필드로 경계를 짜 맞출 수 없다.
        let want: std::collections::BTreeSet<&str> = sort_keys.iter().map(|(f, _)| f.as_str()).collect();
        let have: std::collections::BTreeSet<&str> = am.keys().map(String::as_str).collect();
        if want != have {
            return rej("WIRE.BAD_CURSOR", format!("after 키는 sort 필드와 id여야 함: 기대 {want:?}, 받음 {have:?}"));
        }
        // 경계로 쓰는 필드는 호출자가 select로 이미 볼 수 있는 정책 없는 non-null 필드뿐이다(정의 검사를 건너뛴 facts 방어).
        let mut bounds: Vec<(String, bool, String)> = vec![];
        for (f, desc) in &sort_keys {
            let fty = rf["fields"][f.as_str()]["ty"].as_str().unwrap_or("");
            if ex["select"][f.as_str()] != "field" || fty.ends_with('?') {
                return rej("WIRE.CURSOR_FIELD_NOT_ALLOWED", format!("`{f}`는 cursor 경계로 쓸 수 없음(select에 없거나 nullable)"));
            }
            if !rf["fieldRead"][f.as_str()].is_null() {
                return rej("WIRE.POLICY_FIELD_NOT_FILTERABLE", format!("`{f}`에는 field read 정책이 있어 cursor에 쓸 수 없음"));
            }
            let (v, cast) = check_value(facts, fty, &am[f.as_str()], wire)?;
            let p = cx.params.bind(Some(v), cast);
            bounds.push((cx.qualified(&t, &column(facts, res, f).unwrap()), *desc, p));
        }
        // (k1,k2,..) > (v1,v2,..)를 방향별로 전개한다: k1 op v1 OR (k1 = v1 AND k2 op v2) OR ...
        // 모든 키가 non-null이라 NULL 비교로 행이 빠지지 않는다. 마지막 키는 id라 전체가 유일하다.
        let mut ors = vec![];
        for i in 0..bounds.len() {
            let mut terms: Vec<String> = bounds[..i].iter().map(|(c, _, p)| format!("{c} = {p}")).collect();
            let (c, desc, p) = &bounds[i];
            terms.push(format!("{c} {} {p}", if *desc { "<" } else { ">" }));
            ors.push(format!("({})", terms.join(" AND ")));
        }
        wh.push(format!("({})", ors.join(" OR ")));
    }

    let max_rows = budget["rows"].as_i64().unwrap();
    let limit = match o.get("limit") {
        None => max_rows,
        Some(Value::Number(n)) if n.as_i64().is_some_and(|x| x >= 1) => n.as_i64().unwrap(),
        Some(v) => return rej("WIRE.BAD_VALUE", format!("limit {v}")),
    };
    if limit > max_rows {
        return rej("WIRE.ROWS_EXCEEDED", format!("limit {limit} > budget rows {max_rows}"));
    }
    let offset = match (o.get("offset"), max_offset) {
        (None, _) => None,
        (Some(Value::Number(n)), Some(max)) if n.as_i64().is_some_and(|x| x >= 0) => {
            let x = n.as_i64().unwrap();
            if x > max {
                return rej("WIRE.OFFSET_EXCEEDED", format!("offset {x} > budget offset {max}"));
            }
            Some(x)
        }
        (Some(v), _) => return rej("WIRE.BAD_VALUE", format!("offset {v}")),
    };
    let cost = estimate(limit, offset.unwrap_or(0), n_trav, n_agg, n_fp).saturating_add(limit.saturating_mul(many_cost));
    let max_cost = budget["cost"].as_i64().unwrap();
    if cost > max_cost {
        return rej("WIRE.COST_EXCEEDED", format!("추정 비용 {cost} > budget cost {max_cost}"));
    }
    let lp = cx.params.bind_count(limit, "bigint");
    let offset_sql = match offset {
        Some(x) => format!(" OFFSET {}", cx.params.bind_count(x, "bigint")),
        None => String::new(),
    };
    let obj: Vec<String> = cols.iter().map(|(k, v)| format!("'{k}', {v}")).collect();
    let projection = match dialect {
        Dialect::Postgres => format!("{}::text", json_object(dialect, &obj)),
        Dialect::MySql => format!("CAST({} AS CHAR CHARACTER SET utf8mb4)", json_object(dialect, &obj)),
    };
    let sql = format!(
        "SELECT {projection} FROM {} {t} {} WHERE {} ORDER BY {} LIMIT {lp}{offset_sql}",
        cx.table(res),
        joins.join(" "),
        wh.join(" AND "),
        order.join(", ")
    );
    let (sql, params) = cx.params.finish(sql);
    Ok(Plan {
        sql,
        params,
        deadline_ms: budget["deadlineMs"].as_u64().unwrap(),
        output_type: json!({ "rows": out, "maxRows": limit }),
        cost,
        null_is_denied: false,
        deps: vec![],
    })
}

/// 1:N traverse 하나를 LEFT JOIN LATERAL로 만든다. (join SQL, select 열 식, 출력 타입, 부모 행당 비용)을 돌려준다.
/// 자식 행은 대상 resource의 rowRead로 먼저 거르고(안쪽), 상한은 그 뒤에 적용한다. 그래서 안 보이는 행이 상한을 차지하지 않는다.
/// fieldRead는 바깥 alias에서 CASE로 가린다. 정렬은 정의가 고정하고 id가 마지막 키라 결정적이다.
fn plan_traverse_many(
    facts: &Value,
    cx: &mut Ctx<'_>,
    res: &str,
    rel: &str,
    sub: &Value,
    t: &str,
    budget: &Value,
) -> Result<(String, String, Value, i64), Reject> {
    let tm = &facts["resources"][res]["exposeRead"]["traverseMany"][rel];
    let so = sub.as_object().ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "traverse 값은 객체".into() })?;
    // 정렬·필터·offset은 정의가 고정한다. 호출자가 고를 수 있는 것은 select와 더 작은 limit뿐이다.
    keys(so, &["select", "limit"], "traverse")?;
    let tsel = so.get("select").and_then(Value::as_array).ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "traverse select 필요".into() })?;
    if tsel.is_empty() {
        return rej("WIRE.BAD_REQUEST", "traverse select가 비었음");
    }
    // 자식의 자식은 이번 범위 밖이다. 중첩 select 항목은 모양과 무관하게 명시적으로 거부한다.
    if tsel.iter().any(|x| x.is_object()) {
        return rej("WIRE.NESTED_TRAVERSE_NOT_ALLOWED", format!("`{res}.{rel}` 안에서 관계를 다시 탐색할 수 없음(1:N traverse 중첩 금지)"));
    }
    let depth = budget["depth"].as_i64().unwrap_or(1);
    if depth < 2 {
        return rej("WIRE.DEPTH_EXCEEDED", format!("budget depth {depth}에서는 관계 탐색 불가"));
    }
    let def_limit = tm["limit"].as_i64().ok_or_else(|| internal("traverseMany limit 없음".into()))?;
    let limit = match so.get("limit") {
        None => def_limit,
        Some(Value::Number(n)) if n.as_i64().is_some_and(|x| x >= 1) => n.as_i64().unwrap(),
        Some(v) => return rej("WIRE.BAD_VALUE", format!("limit {v}")),
    };
    if limit > def_limit {
        return rej("WIRE.ROWS_EXCEEDED", format!("`{res}.{rel}` limit {limit} > 정의 상한 {def_limit}"));
    }
    let target = tm["target"].as_str().unwrap();
    let tf = &facts["resources"][target];
    let (ta, ob) = (cx.alias("r"), cx.alias("o"));
    let inner_env = Env { this: Some((ta.clone(), target.to_string())), ..Default::default() };
    let outer_env = Env { this: Some((ob.clone(), target.to_string())), ..Default::default() };
    let (mut pairs, mut tout, mut n_fp) = (vec![], Map::new(), 0i64);
    let mut seen = std::collections::HashSet::new();
    for x in tsel {
        let f = x.as_str().ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "traverse select는 문자열".into() })?;
        if !seen.insert(f) {
            return rej("WIRE.DUPLICATE", format!("select `{f}` 중복"));
        }
        if !tm["select"].as_array().unwrap().iter().any(|y| y == f) {
            return rej("WIRE.FIELD_NOT_EXPOSED", format!("`{target}.{f}`는 `{res}.{rel}` 경로에 열려 있지 않음. 서버 정의 변경 필요"));
        }
        if tf["exposeRead"]["select"][f] == "aggregate" {
            return rej("WIRE.FIELD_NOT_EXPOSED", format!("`{target}.{f}`는 집계라 1:N traverse에서 쓸 수 없음"));
        }
        let col = cx.qualified(&ob, &column(facts, target, f).unwrap());
        let fty = tf["fields"][f]["ty"].as_str().unwrap();
        let redact = !tf["fieldRead"][f].is_null();
        let guarded = ref_guarded(tf["exposeRead"]["select"][f].as_str());
        let mut e = col.clone();
        if guarded {
            n_fp += 1;
            e = guard_ref(facts, cx, fty, &col, &e)?;
        }
        if redact {
            n_fp += 1;
            e = format!("CASE WHEN {} THEN {e} END", cx.cond(&tf["fieldRead"][f], &outer_env).map_err(internal)?);
        }
        pairs.push(format!("'{f}', {}", cx.json_scalar(fty, &e)));
        tout.insert(f.to_string(), out_ty(fty, redact || guarded));
    }
    let row = cx.cond(&tf["rowRead"], &inner_env).map_err(internal)?;
    let fk = column(facts, target, tm["via"].as_str().unwrap()).unwrap();
    let sort_col = column(facts, target, tm["sort"]["field"].as_str().unwrap_or("id")).unwrap();
    let desc = tm["sort"]["desc"] == true;
    let dialect = cx.dialect;
    let lp = cx.params.bind_count(limit, "bigint");
    let jv = cx.alias("j");
    let inner_order = format!("{}, {}", order_term(dialect, &cx.qualified(&ta, &sort_col), desc), cx.qualified(&ta, "id"));
    let from = format!(
        "FROM (SELECT {ta}.* FROM {tbl} {ta} WHERE {} = {} AND {row} ORDER BY {inner_order} ASC LIMIT {lp}) {ob}",
        cx.qualified(&ta, &fk),
        cx.qualified(t, "id"),
        tbl = cx.table(target),
    );
    let join = match dialect {
        Dialect::Postgres => {
            let dir = if desc { "DESC" } else { "ASC" };
            let outer_sort = cx.qualified(&ob, &sort_col);
            let outer_id = cx.qualified(&ob, "id");
            format!(
                "LEFT JOIN LATERAL (SELECT coalesce(json_agg(json_build_object({pairs}) ORDER BY {outer_sort} {dir}, {outer_id} ASC), '[]'::json) AS v \
                 {from}) {jv} ON TRUE",
                pairs = pairs.join(", "),
            )
        }
        // JSON_ARRAYAGG에는 ORDER BY가 없다. LIMIT가 붙은 파생 테이블은 병합되지 않고 정렬된 채 물질화되어 그 순서로 모인다.
        Dialect::MySql => {
            format!("LEFT JOIN LATERAL (SELECT COALESCE(JSON_ARRAYAGG({}), JSON_ARRAY()) AS v {from}) {jv} ON TRUE", json_object(dialect, &pairs))
        }
    };
    let ty = json!({ "array": tout, "nullable": false, "maxItems": limit, "reason": "대상 행 정책으로 줄어든 목록. 없으면 빈 배열" });
    Ok((join, format!("{jv}.v"), ty, limit.saturating_mul(1 + n_fp)))
}

fn plan_aggregate(dialect: Dialect, facts: &Value, o: &Map<String, Value>, caller: &Caller, wire: crate::id_wire::IdWire) -> Result<Plan, Reject> {
    let name = o.get("aggregate").and_then(Value::as_str).unwrap_or("");
    let (res, an) = name.split_once('.').ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: "aggregate는 `Resource.name`".into() })?;
    let rf = &facts["resources"][res];
    if !rf["exposeAggregates"].as_array().is_some_and(|v| v.iter().any(|x| x == an)) {
        return rej("WIRE.NOT_EXPOSED", format!("`{name}`는 노출된 집계가 아님"));
    }
    let a = &rf["aggregates"][an];
    let caller_filter_allowed = a["callerFilter"].as_str() != Some("none");
    if o.contains_key("filter") && !caller_filter_allowed {
        return rej("WIRE.FILTER_NOT_ALLOWED", "이 집계는 callerFilter를 허용하지 않음");
    }
    keys(o, if caller_filter_allowed { &["aggregate", "input", "filter"] } else { &["aggregate", "input"] }, "aggregate 요청")?;
    let mut cx = Ctx::new(facts, caller.actor_id, &caller.now).with_dialect(dialect);
    let empty = Map::new();
    let given = o.get("input").and_then(Value::as_object).unwrap_or(&empty);
    let mut input = HashMap::new();
    let declared = a["input"].as_array().unwrap();
    for k in given.keys() {
        if !declared.iter().any(|d| d[0] == json!(k)) {
            let allowed = allowed_names(declared.iter().filter_map(|d| d[0].as_str()));
            return rej("WIRE.UNKNOWN_KEY", format!("집계 입력 `{k}` 없음. 허용: {allowed}"));
        }
    }
    for d in declared {
        let (n, ty) = (d[0].as_str().unwrap(), d[1].as_str().unwrap());
        let v = given.get(n).ok_or(Reject { code: "WIRE.BAD_REQUEST", msg: format!("입력 `{n}` 필요") })?;
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
        return rej("WIRE.NOT_EXPOSED", "단독 집계는 guard access만 지원");
    }
    let outer = Env { this: None, vars: HashMap::new(), input: input.clone() };
    let mut genv = Env::default();
    for (p, arg) in acc["params"].as_array().unwrap().iter().zip(sa["args"].as_array().unwrap()) {
        let h = cx.handle(arg, &outer).map_err(internal)?;
        genv.vars.insert(p[0].as_str().unwrap().to_string(), h);
    }
    let guard = cx.cond(&acc["body"], &genv).map_err(internal)?;
    let src = a["source"].as_str().unwrap();
    let rf_src = &facts["resources"][src];
    let al = cx.alias("s");
    let wenv = Env { this: Some((al.clone(), src.to_string())), vars: HashMap::new(), input };
    let mut wh_parts = vec![];
    let row = cx.cond(&rf_src["rowRead"], &wenv).map_err(internal)?;
    wh_parts.push(row);
    if !a["where"].is_null() {
        let w_decl = cx.cond(&a["where"], &wenv).map_err(internal)?;
        wh_parts.push(w_decl);
    }
    if let Some(fl) = o.get("filter") {
        let ex = &rf_src["exposeRead"];
        if ex.is_null() {
            return rej("WIRE.FILTER_NOT_ALLOWED", format!("`{src}`에 exposeRead가 없어 filter를 적용할 수 없음"));
        }
        let filter_wh = compile_filters(dialect, facts, src, rf_src, ex, &mut cx, &al, fl, wire)?;
        wh_parts.extend(filter_wh);
    }
    let w = wh_parts.join(" AND ");
    // guard가 거짓이면 NULL을 돌려 ACCESS_DENIED로 바꾼다. 없는 동아리와 남의 동아리를 같은 결과로 만든다.
    let measured = measure(facts, &mut cx, a, &al)?;
    let value = format!("(CASE WHEN {guard} THEN (SELECT {measured} FROM {} {al} WHERE {w}) END)", cx.table(src));
    let sql = match dialect {
        Dialect::Postgres => format!("SELECT {value}::text"),
        Dialect::MySql => format!("SELECT CAST({value} AS CHAR CHARACTER SET utf8mb4)"),
    };
    let (sql, params) = cx.params.finish(sql);
    Ok(Plan {
        sql,
        params,
        // C의 단독 집계 노출에는 budget이 없다. spike는 고정 2s를 쓰고 이 공백을 결과 문서에 남긴다.
        deadline_ms: 2000,
        output_type: json!({ "value": out_ty(a["ty"].as_str().unwrap(), false) }),
        cost: 1,
        null_is_denied: true,
        deps: vec![],
    })
}
