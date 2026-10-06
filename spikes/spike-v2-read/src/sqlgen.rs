//! V1 typed facts의 정책 식을 PostgreSQL 조건으로 옮긴다. 호출자 값은 모두 바인딩 매개변수로만 들어간다.
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

pub const MAX_POLICY_DEPTH: usize = 64;
pub const MAX_POLICY_WORK: usize = 65_536;
pub const MAX_POLICY_SQL_BYTES: usize = 1 << 20;
pub const MAX_POLICY_SQL_WORK_BYTES: usize = 8 << 20;
pub const MAX_POLICY_DATA_BYTES: usize = 8 << 20;

static SCHEMA: OnceLock<String> = OnceLock::new();

/// 실험별 schema를 분리한다(V2 aip_v2_spike, V3 aip_v3_spike). 처음 쓰기 전에 한 번만 정한다.
pub fn set_schema(name: &str) {
    try_set_schema(name).expect("schema는 프로세스에서 한 번만 정한다");
}

pub fn try_set_schema(name: &str) -> Result<(), &'static str> {
    if SCHEMA.set(name.to_string()).is_ok() || SCHEMA.get().is_some_and(|current| current == name) {
        Ok(())
    } else {
        Err("프로세스당 한 schema만 사용할 수 있음")
    }
}

pub fn schema() -> &'static str {
    SCHEMA.get_or_init(|| "aip_v2_spike".to_string())
}

pub fn snake(s: &str) -> String {
    let mut o = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                o.push('_');
            }
            o.push(c.to_ascii_lowercase());
        } else {
            o.push(c);
        }
    }
    o
}

pub fn table(res: &str) -> String {
    format!("{}.{}", schema(), snake(res))
}

/// facts 타입 문자열에서 참조 대상 resource를 꺼낸다. `Ref<Club>?` → Club
pub fn ref_target(ty: &str) -> Option<&str> {
    let t = ty.trim_end_matches('?');
    t.strip_prefix("Ref<").or_else(|| t.strip_prefix("Id<")).and_then(|x| x.strip_suffix('>'))
}

pub fn column(facts: &Value, res: &str, field: &str) -> Option<String> {
    let ty = facts["resources"][res]["fields"][field]["ty"].as_str()?;
    Some(if ty.starts_with("Ref<") { format!("{}_id", snake(field)) } else { snake(field) })
}

#[derive(Default)]
pub struct Params {
    pub values: Vec<Option<String>>,
}

impl Params {
    pub fn bind(&mut self, v: Option<String>, cast: &str) -> String {
        self.values.push(v);
        format!("${}::text::{cast}", self.values.len())
    }
}

/// 식 안의 값이 가리키는 것. 행 별칭, 어떤 resource의 id 식, 그냥 스칼라.
#[derive(Clone, Debug)]
pub enum Handle {
    Row { alias: String, res: String },
    Id { sql: String, res: String },
    Scalar(String),
}

impl Handle {
    fn sql(&self) -> String {
        match self {
            Handle::Row { alias, .. } => format!("{alias}.id"),
            Handle::Id { sql, .. } | Handle::Scalar(sql) => sql.clone(),
        }
    }
}

pub struct Ctx<'a> {
    pub facts: &'a Value,
    pub params: Params,
    actor_id: Option<i64>,
    actor: Option<Handle>,
    now_val: String,
    now: Option<String>,
    pub inline: bool,
    /// 쓰기 판정용. 정책이 읽는 다른 행을 FOR SHARE로 잠가 판정 근거가 커밋 전까지 바뀌지 않게 한다(R2B-06).
    pub lock_reads: bool,
    next_alias: usize,
    depth: usize,
    work: usize,
    sql_work_bytes: usize,
    data_bytes: usize,
    predicate_body_bytes: HashMap<String, usize>,
}

#[derive(Clone, Default)]
pub struct Env {
    pub this: Option<(String, String)>,
    pub vars: HashMap<String, Handle>,
    pub input: HashMap<String, Handle>,
}

pub type R<T> = Result<T, String>;

impl<'a> Ctx<'a> {
    pub fn new(facts: &'a Value, actor_id: Option<i64>, now: &str) -> Ctx<'a> {
        // actor·now는 실제로 쓰일 때만 바인딩한다. 안 쓰인 매개변수는 PG가 타입을 추론하지 못한다(42P18).
        Ctx {
            facts,
            params: Params::default(),
            actor_id,
            actor: None,
            now_val: now.to_string(),
            now: None,
            inline: false,
            lock_reads: false,
            next_alias: 0,
            depth: 0,
            work: 0,
            sql_work_bytes: 0,
            data_bytes: 0,
            predicate_body_bytes: HashMap::new(),
        }
    }

    /// DDL(부분 인덱스 조건)용. 매개변수를 쓸 수 없으므로 facts에서 온 enum 값만 리터럴로 넣는다.
    pub fn for_ddl(facts: &'a Value) -> Ctx<'a> {
        Ctx {
            facts,
            params: Params::default(),
            actor_id: None,
            actor: Some(Handle::Scalar("NULL".into())),
            now_val: String::new(),
            now: Some("now()".into()),
            inline: true,
            lock_reads: false,
            next_alias: 0,
            depth: 0,
            work: 0,
            sql_work_bytes: 0,
            data_bytes: 0,
            predicate_body_bytes: HashMap::new(),
        }
    }

    pub fn alias(&mut self, p: &str) -> String {
        self.next_alias += 1;
        format!("{p}{}", self.next_alias)
    }

    fn field_ty(&self, res: &str, f: &str) -> R<String> {
        self.facts["resources"][res]["fields"][f]["ty"].as_str().map(str::to_string).ok_or_else(|| format!("facts에 {res}.{f} 없음"))
    }

    fn step(&self, h: Handle, seg: &str) -> R<Handle> {
        let (res, row_sql, id_sql) = match &h {
            Handle::Row { alias, res } => (res.clone(), Some(alias.clone()), format!("{alias}.id")),
            Handle::Id { sql, res } => (res.clone(), None, sql.clone()),
            Handle::Scalar(_) => return Err(format!("스칼라에서 `{seg}` 접근")),
        };
        let ty = self.field_ty(&res, seg)?;
        let col = column(self.facts, &res, seg).unwrap();
        let val = match (&row_sql, seg) {
            (_, "id") => id_sql,
            (Some(a), _) => format!("{a}.{col}"),
            (None, _) => {
                let lock = if self.lock_reads { " FOR SHARE OF z" } else { "" };
                format!("(SELECT z.{col} FROM {} z WHERE z.id = {id_sql}{lock})", table(&res))
            }
        };
        Ok(match ref_target(&ty) {
            Some(t) => Handle::Id { sql: val, res: t.to_string() },
            None => Handle::Scalar(val),
        })
    }

    fn actor(&mut self) -> Handle {
        if self.actor.is_none() {
            let res = self.facts["actor"].as_str().unwrap_or("Member").to_string();
            let sql = self.params.bind(self.actor_id.map(|x| x.to_string()), "bigint");
            self.actor = Some(Handle::Id { sql, res });
        }
        self.actor.clone().unwrap()
    }

    pub fn path(&mut self, p: &Value, env: &Env) -> R<Handle> {
        self.charge_work(1)?;
        let segments = p["path"]["segs"].as_array().ok_or("segs 없음")?;
        if segments.len() > MAX_POLICY_DEPTH {
            return Err("정책 경로 중첩 한도 초과".into());
        }
        let root = p["path"]["root"].as_str().ok_or("path root 없음")?;
        let mut h = match root {
            "this" => {
                let (alias, res) = env.this.clone().ok_or("this 없음")?;
                Handle::Row { alias, res }
            }
            "actor" => self.actor(),
            r if r.starts_with("input.") => env.input.get(&r[6..]).cloned().ok_or(format!("입력 {r} 없음"))?,
            r if r.starts_with("var.") => env.vars.get(&r[4..]).cloned().ok_or(format!("변수 {r} 없음"))?,
            r => return Err(format!("알 수 없는 root {r}")),
        };
        for s in segments {
            self.charge_work(1)?;
            h = self.step(h, s.as_str().ok_or("경로 segment 문자열 필요")?)?;
            let bytes = match &h {
                Handle::Row { alias, .. } => alias.len(),
                Handle::Id { sql, .. } | Handle::Scalar(sql) => sql.len(),
            };
            self.charge_sql_work(bytes)?;
        }
        Ok(h)
    }

    pub fn value(&mut self, e: &Value, env: &Env) -> R<String> {
        self.charge_work(1)?;
        if e.get("path").is_some() {
            return Ok(self.path(e, env)?.sql());
        }
        if let Some(en) = e.get("enum").and_then(Value::as_str) {
            let v = en.split_once('.').ok_or("enum 형식")?.1;
            if self.inline {
                if !v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Err("enum 값 형식".into());
                }
                return Ok(format!("'{v}'"));
            }
            self.charge_data_bytes(v.len())?;
            return Ok(self.params.bind(Some(v.to_string()), "text"));
        }
        if e.get("now").is_some() {
            if self.now.is_none() {
                self.charge_data_bytes(self.now_val.len())?;
                self.now = Some(self.params.bind(Some(self.now_val.clone()), "timestamptz"));
            }
            return Ok(self.now.clone().unwrap());
        }
        match e.get("lit") {
            Some(Value::Number(n)) => {
                let value = n.to_string();
                self.charge_data_bytes(value.len())?;
                Ok(self.params.bind(Some(value), "bigint"))
            }
            Some(Value::Null) => Ok("NULL".into()),
            Some(Value::Bool(b)) if self.inline => Ok(b.to_string()),
            Some(Value::Bool(b)) => {
                let value = b.to_string();
                self.charge_data_bytes(value.len())?;
                Ok(self.params.bind(Some(value), "boolean"))
            }
            Some(Value::String(s)) => {
                self.charge_data_bytes(s.len())?;
                Ok(self.params.bind(Some(s.clone()), "text"))
            }
            _ => Err(format!("값으로 쓸 수 없는 식 {e}")),
        }
    }

    fn charge_work(&mut self, amount: usize) -> R<()> {
        self.work = self.work.saturating_add(amount);
        if self.work > MAX_POLICY_WORK {
            return Err("정책 식 작업량 한도 초과".into());
        }
        Ok(())
    }

    fn charge_sql_work(&mut self, bytes: usize) -> R<()> {
        self.sql_work_bytes = self.sql_work_bytes.saturating_add(bytes);
        if bytes > MAX_POLICY_SQL_BYTES || self.sql_work_bytes > MAX_POLICY_SQL_WORK_BYTES {
            return Err("정책 SQL 크기·생성 작업량 한도 초과".into());
        }
        Ok(())
    }

    fn charge_data_bytes(&mut self, bytes: usize) -> R<()> {
        self.data_bytes = self.data_bytes.saturating_add(bytes);
        if self.data_bytes > MAX_POLICY_DATA_BYTES {
            return Err("정책 데이터 복사량 한도 초과".into());
        }
        Ok(())
    }

    pub fn cond(&mut self, e: &Value, env: &Env) -> R<String> {
        self.charge_work(1)?;
        if self.depth >= MAX_POLICY_DEPTH {
            return Err("정책 식 중첩 한도 초과".into());
        }
        self.depth += 1;
        let result = self.cond_inner(e, env);
        self.depth -= 1;
        let sql = result?;
        self.charge_sql_work(sql.len())?;
        Ok(sql)
    }

    fn cond_inner(&mut self, e: &Value, env: &Env) -> R<String> {
        if e.get("default").and_then(Value::as_str) == Some("denyAll") {
            return Ok("FALSE".into());
        }
        for op in ["and", "or"] {
            if let Some(v) = e.get(op).and_then(Value::as_array) {
                let mut parts = Vec::new();
                let mut bytes = 2usize;
                for expression in v {
                    let sql = self.cond(expression, env)?;
                    bytes = bytes.saturating_add(sql.len()).saturating_add(op.len() + 2);
                    if bytes > MAX_POLICY_SQL_BYTES {
                        return Err("정책 SQL 크기 한도 초과".into());
                    }
                    parts.push(sql);
                }
                return Ok(format!("({})", parts.join(&format!(" {} ", op.to_uppercase()))));
            }
        }
        if let Some(x) = e.get("not") {
            return Ok(format!("(NOT {})", self.cond(x, env)?));
        }
        // 단독 Bool 리터럴·Bool 경로도 조건이다(R2B-02). NULL은 IS TRUE로 거짓이 된다.
        if let Some(Value::Bool(b)) = e.get("lit") {
            return Ok(if *b { "TRUE".into() } else { "FALSE".into() });
        }
        if e.get("path").is_some() && e["ty"].as_str().is_some_and(|t| t.trim_end_matches('?') == "Bool") {
            return Ok(format!("({} IS TRUE)", self.value(e, env)?));
        }
        if let Some(op) = e.get("cmp").and_then(Value::as_str) {
            if !matches!(op, "=" | "!=" | "<" | "<=" | ">" | ">=") {
                return Err("허용되지 않은 정책 비교 연산자".into());
            }
            let null_r = e["r"].get("lit") == Some(&Value::Null);
            let null_l = e["l"].get("lit") == Some(&Value::Null);
            if null_r || null_l {
                let other = if null_r { &e["l"] } else { &e["r"] };
                let v = self.value(other, env)?;
                return Ok(format!("({v} {})", if op == "=" { "IS NULL" } else { "IS NOT NULL" }));
            }
            let l = self.value(&e["l"], env)?;
            let r = self.value(&e["r"], env)?;
            let sop = if op == "!=" { "<>" } else { op };
            // 두 값 중 하나가 NULL이면 SQL 3치 논리로 거짓 취급된다. 익명 actor가 학교 행에 매칭되지 않는 근거다.
            return Ok(format!("({l} {sop} {r})"));
        }
        if let Some(l) = e.get("in") {
            let lv = self.value(l, env)?;
            let items = e["items"].as_array().ok_or("in items")?.iter().map(|x| self.value(x, env)).collect::<R<Vec<_>>>()?;
            return Ok(format!("({lv} IN ({}))", items.join(", ")));
        }
        if let Some(name) = e.get("call").and_then(Value::as_str) {
            let pred = &self.facts["predicates"][name];
            let params = pred["params"].as_array().ok_or(format!("predicate {name} 없음"))?;
            let args = e["args"].as_array().ok_or("args")?;
            let mut inner = Env { this: None, vars: HashMap::new(), input: HashMap::new() };
            for (p, a) in params.iter().zip(args) {
                let h = self.handle(a, env)?;
                inner.vars.insert(p[0].as_str().unwrap().to_string(), h);
            }
            let body_ref = &pred["body"];
            let body_bytes = if let Some(&bytes) = self.predicate_body_bytes.get(name) {
                bytes
            } else {
                let bytes = payload_bytes(body_ref);
                self.charge_data_bytes(name.len())?;
                self.predicate_body_bytes.insert(name.to_string(), bytes);
                bytes
            };
            self.charge_data_bytes(body_bytes)?;
            let body = body_ref.clone();
            return self.cond(&body, &inner);
        }
        if let Some(r) = e.get("exists").and_then(Value::as_str) {
            let a = self.alias("x");
            let mut inner = env.clone();
            inner.this = Some((a.clone(), r.to_string()));
            let c = self.cond(&e["where"], &inner)?;
            let lock = if self.lock_reads { format!(" FOR SHARE OF {a}") } else { String::new() };
            return Ok(format!("EXISTS (SELECT 1 FROM {} {a} WHERE {c}{lock})", table(r)));
        }
        Err(format!("조건으로 쓸 수 없는 식 {e}"))
    }

    pub fn handle(&mut self, e: &Value, env: &Env) -> R<Handle> {
        if e.get("path").is_some() {
            return self.path(e, env);
        }
        Ok(Handle::Scalar(self.value(e, env)?))
    }
}

/// Estimates owned JSON payload bytes iteratively; a predicate body is measured once and cached.
fn payload_bytes(value: &Value) -> usize {
    let mut bytes = 0usize;
    let mut stack = vec![value];
    while let Some(value) = stack.pop() {
        match value {
            Value::String(text) => bytes = bytes.saturating_add(text.len()),
            Value::Number(number) => bytes = bytes.saturating_add(number.to_string().len()),
            Value::Array(items) => stack.extend(items),
            Value::Object(fields) => {
                for (key, value) in fields {
                    bytes = bytes.saturating_add(key.len());
                    stack.push(value);
                }
            }
            Value::Null | Value::Bool(_) => {}
        }
    }
    bytes
}

pub fn ddl(facts: &Value) -> R<Vec<String>> {
    let mut out = vec![format!("DROP SCHEMA IF EXISTS {} CASCADE", schema())];
    out.extend(create_ddl(facts)?);
    Ok(out)
}

/// Fresh-schema creation only; the caller owns the enclosing transaction.
pub fn create_ddl(facts: &Value) -> R<Vec<String>> {
    create_ddl_in(schema(), facts)
}

/// Generate deployment DDL without changing the request engine's namespace.
pub fn create_ddl_in(namespace: &str, facts: &Value) -> R<Vec<String>> {
    let mut bytes = namespace.bytes();
    if namespace.len() > 63
        || !bytes.next().is_some_and(|b| b.is_ascii_lowercase() || b == b'_')
        || !bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Err("잘못된 schema 식별자".into());
    }
    let table = |resource: &str| format!("{namespace}.{}", snake(resource));
    let res = facts["resources"].as_object().ok_or("resources")?;
    let mut done: Vec<String> = vec![];
    let mut out = vec![format!("CREATE SCHEMA {namespace}")];
    while done.len() < res.len() {
        let before = done.len();
        for (name, r) in res {
            if done.contains(name) {
                continue;
            }
            let fields = r["fields"].as_object().unwrap();
            let deps: Vec<&str> = fields.values().filter_map(|f| ref_target(f["ty"].as_str().unwrap())).collect();
            if deps.iter().any(|d| *d != name && !done.iter().any(|x| x == d)) {
                continue;
            }
            let mut cols = vec![];
            for (f, fd) in fields {
                let ty = fd["ty"].as_str().unwrap();
                let nullable = ty.ends_with('?');
                let base = ty.trim_end_matches('?');
                let col = column(facts, name, f).unwrap();
                let sql_ty = if f == "id" && base.starts_with("Id<") {
                    // id는 서버가 만든다. seed처럼 명시 값도 받는다.
                    "bigint GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY".to_string()
                } else if let Some(t) = base.strip_prefix("Ref<").and_then(|x| x.strip_suffix('>')) {
                    format!("bigint REFERENCES {}(id)", table(t))
                } else if let Some(e) = base.strip_prefix("Enum<").and_then(|x| x.strip_suffix('>')) {
                    let vs: Vec<String> = facts["enums"][e].as_array().unwrap().iter().map(|v| format!("'{}'", v.as_str().unwrap())).collect();
                    format!("text CHECK ({col} IN ({}))", vs.join(", "))
                } else {
                    match base {
                        "Text" | "Url" => match fd["range"].as_array() {
                            Some(r) => format!("text CHECK (char_length({col}) BETWEEN {} AND {})", r[0], r[1]),
                            None => "text".into(),
                        },
                        "Time" => "timestamptz".into(),
                        // 범위는 Text/Url과 같이 DDL CHECK로 집행한다. NULL은 CHECK가 통과시키므로 nullable도 그대로 둔다.
                        "Int" => match fd["range"].as_array().map(|r| (r.first().and_then(Value::as_i64), r.get(1).and_then(Value::as_i64))) {
                            Some((Some(lo), Some(hi))) => format!("bigint CHECK ({col} BETWEEN {lo} AND {hi})"),
                            Some(_) => return Err(format!("`{col}` Int 범위 형식")),
                            None => "bigint".into(),
                        },
                        "Bool" => "boolean".into(),
                        b => return Err(format!("DDL 미지원 타입 {b}")),
                    }
                };
                let nn = if nullable || sql_ty.contains("PRIMARY KEY") { "" } else { " NOT NULL" };
                cols.push(format!("{col} {sql_ty}{nn}"));
            }
            out.push(format!("CREATE TABLE {} ({})", table(name), cols.join(", ")));
            done.push(name.clone());
        }
        if done.len() == before {
            return Err("참조 순환으로 DDL 순서를 정할 수 없음".into());
        }
    }
    // 전이 효과의 notify는 같은 트랜잭션에 outbox 행으로 남긴다. 실제 전달은 별도 작업이다.
    out.push(format!(
        "CREATE TABLE {}.aip_outbox (id bigserial PRIMARY KEY, topic text NOT NULL, recipient_id bigint, source text NOT NULL, source_id bigint, created_at timestamptz NOT NULL DEFAULT now())",
        namespace
    ));
    for (name, r) in res {
        for (i, cols) in r["unique"].as_array().into_iter().flatten().enumerate() {
            let cs: Vec<String> = cols.as_array().unwrap().iter().map(|c| column(facts, name, c.as_str().unwrap()).unwrap()).collect();
            out.push(format!("CREATE UNIQUE INDEX {}_unique_{i} ON {} ({})", snake(name), table(name), cs.join(", ")));
        }
        for (inv, d) in r["invariants"].as_object().unwrap() {
            let enf = &d["enforcement"];
            if enf["kind"] == "partialUniqueIndex" {
                let mut cx = Ctx::for_ddl(facts);
                let env = Env { this: Some((snake(name), name.clone())), ..Default::default() };
                let cond = cx.cond(&enf["where"], &env)?;
                let cols: Vec<String> =
                    enf["columns"].as_array().unwrap().iter().map(|c| column(facts, name, c.as_str().unwrap()).unwrap()).collect();
                // 부분 인덱스 조건은 테이블 별칭을 쓸 수 없어 `recruitment.status` 꼴을 열 이름으로 바꾼다.
                let cond = cond.replace(&format!("{}.", snake(name)), "");
                if enf["deferred"] == true {
                    // 커밋 시점 검사. 교대(1명 → 잠시 2명 → 1명) 같은 중간 상태를 허용한다.
                    let ex: Vec<String> = cols.iter().map(|c| format!("{c} WITH =")).collect();
                    out.push(format!(
                        "ALTER TABLE {} ADD CONSTRAINT {}_{} EXCLUDE USING btree ({}) WHERE ({cond}) DEFERRABLE INITIALLY DEFERRED",
                        table(name),
                        snake(name),
                        snake(inv),
                        ex.join(", ")
                    ));
                } else {
                    out.push(format!("CREATE UNIQUE INDEX {}_{} ON {} ({}) WHERE {cond}", snake(name), snake(inv), table(name), cols.join(", ")));
                }
            } else {
                return Err(format!("{inv}: {} 집행은 V3 범위", enf["kind"]));
            }
        }
    }
    Ok(out)
}
