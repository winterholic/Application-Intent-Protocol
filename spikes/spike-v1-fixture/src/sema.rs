use crate::ast::*;
use crate::diag::{Diag, Span};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Id(String),
    Text,
    Email,
    Url,
    Time,
    Date,
    Int,
    Decimal(u8, u8),
    Bool,
    Json,
    Enum(String),
    Ref(String),
    Null,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TT {
    pub ty: Ty,
    pub nullable: bool,
}

impl TT {
    fn new(ty: Ty) -> TT {
        TT { ty, nullable: false }
    }
    pub fn show(&self) -> String {
        let s = match &self.ty {
            Ty::Id(r) => format!("Id<{r}>"),
            Ty::Text => "Text".into(),
            Ty::Email => "Email".into(),
            Ty::Url => "Url".into(),
            Ty::Time => "Time".into(),
            Ty::Date => "Date".into(),
            Ty::Int => "Int".into(),
            Ty::Decimal(precision, scale) => format!("Decimal<{precision},{scale}>"),
            Ty::Bool => "Bool".into(),
            Ty::Json => "Json".into(),
            Ty::Enum(e) => format!("Enum<{e}>"),
            Ty::Ref(r) => format!("Ref<{r}>"),
            Ty::Null => "Null".into(),
        };
        if self.nullable {
            format!("{s}?")
        } else {
            s
        }
    }
}

fn valid_email(s: &str) -> bool {
    if s.contains('\0') || s.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty() && !domain.is_empty() && !domain.starts_with('.') && !domain.ends_with('.') && domain.contains('.') && !domain.contains('@')
}

fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10
        || !b.is_ascii()
        || b[4] != b'-'
        || b[7] != b'-'
        || ![&b[..4], &b[5..7], &b[8..]].iter().all(|part| part.iter().all(u8::is_ascii_digit))
    {
        return false;
    }
    let number = |part: &[u8]| std::str::from_utf8(part).ok().and_then(|s| s.parse::<u32>().ok());
    let (Some(year), Some(month), Some(day)) = (number(&b[..4]), number(&b[5..7]), number(&b[8..])) else {
        return false;
    };
    if year == 0 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=days).contains(&day)
}

fn valid_decimal(s: &str, precision: u8, scale: u8) -> bool {
    if s.len() > 41 || !s.is_ascii() {
        return false;
    }
    let unsigned = s.strip_prefix('-').unwrap_or(s);
    if unsigned.is_empty() || unsigned.starts_with('+') {
        return false;
    }
    let mut parts = unsigned.split('.');
    let integer = parts.next().unwrap_or_default();
    let fraction = parts.next();
    if parts.next().is_some() || integer.is_empty() || !integer.bytes().all(|b| b.is_ascii_digit()) || !(integer == "0" || !integer.starts_with('0'))
    {
        return false;
    }
    if let Some(fraction) = fraction {
        if scale == 0 || fraction.is_empty() || fraction.len() > usize::from(scale) || !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    let integer_digits = if integer == "0" { 0 } else { integer.len() };
    integer_digits <= usize::from(precision - scale)
}

/// 참조와 그 참조의 Id는 같은 행을 가리키므로 비교·인자 전달에서 호환으로 본다(spike 규칙).
fn same_target(a: &Ty, b: &Ty) -> bool {
    match (a, b) {
        (Ty::Id(x), Ty::Ref(y)) | (Ty::Ref(x), Ty::Id(y)) => x == y,
        _ => a == b,
    }
}

pub struct Output {
    pub execution: Value,
    pub metadata: Value,
    pub spans: Value,
    /// Non-blocking diagnostics for definitions that compile but have a documented privacy risk.
    pub advisories: Vec<Diag>,
}

struct Ctx<'a> {
    spec: &'a Spec,
    res: BTreeMap<&'a str, &'a Resource>,
    enums: BTreeMap<&'a str, &'a EnumDecl>,
    preds: BTreeMap<&'a str, &'a Predicate>,
    accesses: BTreeMap<&'a str, &'a Access>,
    limits: BTreeMap<&'a str, &'a Limit>,
    actor: Option<String>,
    diags: Vec<Diag>,
    advisories: Vec<Diag>,
}

#[derive(Clone, Default)]
struct Scope {
    this: Option<String>,
    in_exists: bool,
    vars: Vec<(String, TT)>,
    input: Vec<(String, TT)>,
}

const UNRESOLVED: &str = "DEF.UNRESOLVED_NAME";
/// `expose apply`가 한 요청에서 다룰 수 있는 행 수의 상한.
const MAX_BULK_ROWS: i64 = 10_000;
const MAX_POLICY_EXPANDED_DEPTH: usize = 64;
const MAX_POLICY_EXPANDED_NODES: usize = 16_384;
const MAX_POLICY_PATH_SEGMENTS: usize = 64;

pub fn analyze(spec: &Spec) -> Result<Output, Vec<Diag>> {
    let mut cx = Ctx {
        spec,
        res: BTreeMap::new(),
        enums: BTreeMap::new(),
        preds: BTreeMap::new(),
        accesses: BTreeMap::new(),
        limits: BTreeMap::new(),
        actor: None,
        diags: vec![],
        advisories: vec![],
    };
    cx.collect();
    let execution = cx.build();
    if !cx.diags.is_empty() {
        return Err(cx.diags);
    }
    let (metadata, spans) = metadata_of(spec);
    Ok(Output { execution, metadata, spans, advisories: cx.advisories })
}

fn metadata_of(spec: &Spec) -> (Value, Value) {
    let mut md = Map::new();
    let mut spans = Map::new();
    for r in &spec.resources {
        if let Some(d) = &r.docs {
            let anchor = format!("resource:{}", r.name);
            md.insert(anchor.clone(), json!({ "summary": d.summary, "visibility": d.visibility }));
            spans.insert(anchor, json!({ "line": d.span.line, "col": d.span.col }));
        }
    }
    (Value::Object(md), Value::Object(spans))
}

impl<'a> Ctx<'a> {
    fn d(&mut self, code: &'static str, msg: impl Into<String>, span: Span) {
        self.diags.push(Diag::new(code, msg, span));
    }

    fn advisory(&mut self, code: &'static str, msg: impl Into<String>, span: Span) {
        self.advisories.push(Diag::new(code, msg, span));
    }

    /// JavaScript treats `__proto__` as the prototype accessor, not an own key, so a generated descriptor or SDK object
    /// would silently lose a member with that name. The definition may not use it.
    fn reserved_name(&mut self, name: &str, span: Span) {
        if name == "__proto__" {
            self.d("DEF.RESERVED_NAME", format!("이름 `{name}`은 예약되어 쓸 수 없음. 다른 이름을 쓴다"), span);
        }
    }

    fn collect(&mut self) {
        let mut names: BTreeSet<String> = BTreeSet::new();
        let spec = self.spec;
        let mut seen = |n: &str, sp: Span, diags: &mut Vec<Diag>| {
            if !names.insert(n.to_string()) {
                diags.push(Diag::new("DEF.DUPLICATE", format!("이름 `{n}` 중복 선언"), sp));
            }
        };
        for e in &spec.enums {
            seen(&e.name, e.span, &mut self.diags);
            self.reserved_name(&e.name, e.span);
            self.enums.insert(&e.name, e);
        }
        for r in &spec.resources {
            seen(&r.name, r.span, &mut self.diags);
            self.reserved_name(&r.name, r.span);
            self.res.insert(&r.name, r);
        }
        for operation in &spec.operations {
            seen(&operation.name, operation.span, &mut self.diags);
            self.reserved_name(&operation.name, operation.span);
        }
        for p in &spec.predicates {
            seen(&p.name, p.span, &mut self.diags);
            self.reserved_name(&p.name, p.span);
            self.preds.insert(&p.name, p);
        }
        for a in &spec.accesses {
            seen(&a.name, a.span, &mut self.diags);
            self.accesses.insert(&a.name, a);
        }
        for l in &spec.limits {
            seen(&l.name, l.span, &mut self.diags);
            self.limits.insert(&l.name, l);
        }
        self.diags.extend(validate_policy_expansion(spec));
        match &spec.actor {
            Some((a, _)) if a == "principal" && !self.res.contains_key("principal") => self.actor = Some(a.clone()),
            Some((a, sp)) if !self.res.contains_key(a.as_str()) => self.d("DEF.UNKNOWN_SYMBOL", format!("actor 대상 resource `{a}` 없음"), *sp),
            Some((a, _)) => self.actor = Some(a.clone()),
            None => self.d("DEF.MISSING_ITEM", "actor 선언 필요", Span::default()),
        }
    }

    fn type_of(&mut self, owner: Option<&str>, t: &TypeRef) -> Option<TT> {
        if t.database_default && owner.is_none() {
            self.d("DEF.TYPE_MISMATCH", "default database는 resource 필드에만 선언한다", t.span);
        }
        let base = if t.id_of {
            if self.res.contains_key(t.name.as_str()) || (t.name == "principal" && self.principal_actor()) {
                Ty::Id(t.name.clone())
            } else {
                self.d("DEF.UNKNOWN_TYPE", format!("`{}.Id`의 resource 없음", t.name), t.span);
                return None;
            }
        } else {
            match t.name.as_str() {
                "Id" | "Uuid" => match owner {
                    Some(o) => Ty::Id(o.to_string()),
                    None => {
                        self.d("DEF.UNKNOWN_TYPE", "resource 밖에서 맨 `Id`는 대상이 없음. `X.Id`를 쓴다", t.span);
                        return None;
                    }
                },
                "Text" => Ty::Text,
                "Email" => Ty::Email,
                "Url" => Ty::Url,
                "Time" => Ty::Time,
                "Date" => Ty::Date,
                "Int" => Ty::Int,
                "Decimal" => match t.decimal {
                    Some((precision, scale)) => Ty::Decimal(precision, scale),
                    None => {
                        self.d("DEF.BAD_DECIMAL_TYPE", "Decimal은 precision과 scale이 필요. `Decimal(p,s)` 형식을 쓴다", t.span);
                        return None;
                    }
                },
                "Bool" => Ty::Bool,
                "Json" => Ty::Json,
                n if self.enums.contains_key(n) => Ty::Enum(n.to_string()),
                n if self.res.contains_key(n) => Ty::Ref(n.to_string()),
                n => {
                    self.d("DEF.UNKNOWN_TYPE", format!("알 수 없는 타입 `{n}`"), t.span);
                    return None;
                }
            }
        };
        if t.range.is_some() && !matches!(base, Ty::Text | Ty::Int) {
            self.d("DEF.TYPE_MISMATCH", "범위 제약은 Text/Int에만", t.span);
        }
        if let Some((lo, hi)) = t.range {
            if lo > hi || (base == Ty::Text && lo < 0) {
                self.d("DEF.BAD_RANGE", format!("만족할 수 없는 범위 {lo}..{hi}"), t.span);
            }
        }
        Some(TT { ty: base, nullable: t.nullable })
    }

    fn field_tt(&self, r: &str, f: &str) -> Option<TT> {
        if r == "principal" && f == "id" && self.principal_actor() {
            return Some(TT::new(Ty::Id("principal".into())));
        }
        let res = self.res.get(r)?;
        let fd = res.fields.iter().find(|x| x.name == f)?;
        // 여기서는 진단을 내지 않는다. 필드 타입 오류는 resource 검사에서 한 번만 낸다.
        let t = &fd.ty;
        let base = if t.id_of {
            Ty::Id(t.name.clone())
        } else {
            match t.name.as_str() {
                "Id" | "Uuid" => Ty::Id(r.to_string()),
                "Text" => Ty::Text,
                "Email" => Ty::Email,
                "Url" => Ty::Url,
                "Time" => Ty::Time,
                "Date" => Ty::Date,
                "Int" => Ty::Int,
                "Decimal" => Ty::Decimal(t.decimal?.0, t.decimal?.1),
                "Bool" => Ty::Bool,
                "Json" => Ty::Json,
                n if self.enums.contains_key(n) => Ty::Enum(n.to_string()),
                n => Ty::Ref(n.to_string()),
            }
        };
        Some(TT { ty: base, nullable: t.nullable })
    }

    fn params(&mut self, ps: &Params) -> (Vec<(String, TT)>, Value) {
        let mut v = vec![];
        let mut j = vec![];
        let mut seen = BTreeSet::new();
        for (n, t) in ps {
            // 이름 바인딩이 앞 인자를 덮어쓰지 않게 매개변수·입출력 이름은 유일해야 한다(R2-02).
            if !seen.insert(n.clone()) {
                self.d("DEF.DUPLICATE", format!("매개변수·입출력 이름 `{n}` 중복"), t.span);
            }
            self.reserved_name(n, t.span);
            if let Some(tt) = self.type_of(None, t) {
                // 범위 제약을 facts에 남긴다. 빠지면 다른 입출력 계약이 같은 digest가 된다(F04).
                j.push(json!([n, tt.show(), t.range.map(|(a, b)| json!([a, b]))]));
                v.push((n.clone(), tt));
            }
        }
        (v, Value::Array(j))
    }

    fn base_scope(&self) -> Scope {
        let mut s = Scope::default();
        if let Some(a) = &self.actor {
            s.vars.push(("actor".into(), TT::new(Ty::Ref(a.clone()))));
        }
        s
    }

    fn principal_actor(&self) -> bool {
        self.actor.as_deref() == Some("principal") && !self.res.contains_key("principal")
    }

    fn path(&self, segs: &[String], sp: Span, sc: &Scope) -> Result<(Value, TT), Diag> {
        let first = segs[0].as_str();
        let this_field = sc.this.as_ref().and_then(|t| self.field_tt(t, first));
        let var = sc.vars.iter().find(|(n, _)| n == first).map(|x| x.1.clone());
        let (root, mut tt, rest) = match first {
            "this" if sc.in_exists => {
                return Err(Diag::new(
                    "DEF.THIS_IN_EXISTS",
                    "exists 조건 안의 `this`는 안쪽 행으로 다시 묶여 `team = this.team`이 항상 참이 된다. 안쪽 필드는 맨 이름으로 쓰고, 바깥 행 값은 predicate 인자로 넘겨라",
                    sp,
                ));
            }
            "this" => match &sc.this {
                Some(t) => ("this".to_string(), TT::new(Ty::Ref(t.clone())), &segs[1..]),
                None => return Err(Diag::new(UNRESOLVED, "이 위치에는 `this`가 없음", sp)),
            },
            "input" => {
                let n = segs.get(1).ok_or_else(|| Diag::new("DEF.PARSE_EXPECTED", "`input.<이름>` 필요", sp))?;
                match sc.input.iter().find(|(x, _)| x == n) {
                    Some((_, t)) => (format!("input.{n}"), t.clone(), &segs[2..]),
                    None => return Err(Diag::new("DEF.UNKNOWN_FIELD", format!("input에 `{n}` 없음"), sp)),
                }
            }
            _ => match (this_field, var) {
                (Some(_), Some(_)) => return Err(Diag::new("DEF.AMBIGUOUS_NAME", format!("`{first}`가 필드와 변수 둘 다로 해석됨"), sp)),
                (Some(t), None) => ("this".to_string(), t, &segs[1..]),
                (None, Some(t)) => (if first == "actor" { "actor".to_string() } else { format!("var.{first}") }, t, &segs[1..]),
                (None, None) => return Err(Diag::new(UNRESOLVED, format!("`{}`를 해석할 수 없음", segs.join(".")), sp)),
            },
        };
        let mut walked: Vec<String> = if root == "this" && first != "this" { vec![first.to_string()] } else { vec![] };
        for s in rest {
            let target = match &tt.ty {
                Ty::Ref(r) => r.clone(),
                _ => return Err(Diag::new("DEF.UNKNOWN_FIELD", format!("`{}` 타입에는 필드 `{s}`가 없음", tt.show()), sp)),
            };
            match self.field_tt(&target, s) {
                Some(f) => {
                    let nullable = tt.nullable || f.nullable;
                    tt = TT { ty: f.ty, nullable };
                }
                None => {
                    return Err(Diag::new("DEF.UNKNOWN_FIELD", format!("`{target}`에 필드 `{s}` 없음"), sp));
                }
            }
            walked.push(s.clone());
        }
        // `x.id`는 x가 가리키는 행 자체와 같다(V1-R5). 같은 의미가 다른 facts가 되지 않게 접는다(F05).
        if walked.last().map(String::as_str) == Some("id") {
            if let Ty::Id(r) = &tt.ty {
                walked.pop();
                tt = TT { ty: Ty::Ref(r.clone()), nullable: tt.nullable };
            }
        }
        Ok((json!({ "path": { "root": root, "segs": walked }, "ty": tt.show() }), tt))
    }

    fn expect(&self, e: &Expr, sc: &Scope, want: Option<&TT>) -> Result<(Value, TT), Diag> {
        if let (Expr::Str(value), Some(expected)) = (e, want) {
            match &expected.ty {
                Ty::Email if valid_email(value) => return Ok((json!({ "lit": value, "ty": "Email" }), TT::new(Ty::Email))),
                Ty::Date if valid_date(value) => return Ok((json!({ "lit": value, "ty": "Date" }), TT::new(Ty::Date))),
                Ty::Decimal(precision, scale) if valid_decimal(value, *precision, *scale) => {
                    let ty = format!("Decimal<{precision},{scale}>");
                    return Ok((json!({ "lit": value, "ty": ty }), TT::new(Ty::Decimal(*precision, *scale))));
                }
                Ty::Email | Ty::Date => {
                    return Err(Diag::new("DEF.BAD_VALUE", format!("`{}` 형식의 문자열 리터럴 필요", expected.show()), span_of(e)));
                }
                Ty::Decimal(_, _) => {
                    return Err(Diag::new("DEF.BAD_VALUE", format!("`{}` 형식의 문자열 리터럴 필요", expected.show()), span_of(e)));
                }
                _ => {}
            }
        }
        if let (Expr::Path(segs, sp), Some(TT { ty: Ty::Enum(en), .. })) = (e, want) {
            if segs.len() == 1 {
                match self.path(segs, *sp, sc) {
                    Err(d) if d.code == UNRESOLVED => {
                        let decl = self.enums[en.as_str()];
                        if decl.variants.contains(&segs[0]) {
                            return Ok((json!({ "enum": format!("{en}.{}", segs[0]) }), TT::new(Ty::Enum(en.clone()))));
                        }
                        return Err(Diag::new("DEF.UNKNOWN_SYMBOL", format!("`{en}`에 값 `{}` 없음", segs[0]), *sp));
                    }
                    other => return other,
                }
            }
        }
        self.ex(e, sc)
    }

    /// `f = f + n`/`f = f - n`. 대상 행 자신의 Int 필드를 정의에 적힌 상수만큼만 바꾼다.
    /// 호출자 값이나 다른 필드를 섞으면 전이가 서버 정의 밖의 계산이 되므로 받지 않는다.
    fn counter(&self, f: &str, ft: &TT, op: &str, l: &Expr, r: &Expr, sp: Span) -> Result<Value, Diag> {
        if ft.ty != Ty::Int || ft.nullable {
            return Err(Diag::new("DEF.TYPE_MISMATCH", format!("증감 대입은 null이 아닌 Int 필드만, `{f}`는 `{}`", ft.show()), sp));
        }
        let same = match l {
            Expr::Path(segs, _) => (segs.len() == 1 && segs[0] == f) || (segs.len() == 2 && segs[0] == "this" && segs[1] == f),
            _ => false,
        };
        let n = match r {
            Expr::Int(n) => *n,
            _ => 0,
        };
        if !same || n == 0 {
            return Err(Diag::new("DEF.ARITH_NOT_ALLOWED", format!("증감 대입은 `{f} = {f} + 정수` 또는 `{f} = {f} - 정수`(0 제외)만"), sp));
        }
        let delta = if op == "-" { n.checked_neg().ok_or_else(|| Diag::new("DEF.BAD_RANGE", "증감 값 범위", sp))? } else { n };
        Ok(json!({ "increment": delta }))
    }

    fn bool_of(&self, e: &Expr, sc: &Scope) -> Result<Value, Diag> {
        let (v, t) = self.ex(e, sc)?;
        if t.ty != Ty::Bool {
            return Err(Diag::new("DEF.TYPE_MISMATCH", format!("조건식은 Bool이어야 함, `{}`", t.show()), span_of(e)));
        }
        Ok(v)
    }

    fn ex(&self, e: &Expr, sc: &Scope) -> Result<(Value, TT), Diag> {
        let b = TT::new(Ty::Bool);
        match e {
            Expr::Or(v) | Expr::And(v) => {
                let op = if matches!(e, Expr::Or(_)) { "or" } else { "and" };
                let args = v.iter().map(|x| self.bool_of(x, sc)).collect::<Result<Vec<_>, _>>()?;
                Ok((json!({ op: args }), b))
            }
            Expr::Not(x) => Ok((json!({ "not": self.bool_of(x, sc)? }), b)),
            Expr::Arith(_, _, _, sp) => {
                Err(Diag::new("DEF.ARITH_NOT_ALLOWED", "산술식은 transition `to`의 `필드 = 필드 ± 정수`에서만 쓸 수 있음", *sp))
            }
            Expr::Cmp(op, l, r) => {
                let (lv, lt, rv, rt) = if matches!(l.as_ref(), Expr::Str(_)) {
                    let (rv, rt) = self.ex(r, sc)?;
                    let (lv, lt) = self.expect(l, sc, Some(&rt))?;
                    (lv, lt, rv, rt)
                } else {
                    match self.ex(l, sc) {
                        Ok((lv, lt)) => {
                            let (rv, rt) = self.expect(r, sc, Some(&lt))?;
                            (lv, lt, rv, rt)
                        }
                        Err(d) if d.code == UNRESOLVED => {
                            let (rv, rt) = self.ex(r, sc)?;
                            let (lv, lt) = self.expect(l, sc, Some(&rt))?;
                            (lv, lt, rv, rt)
                        }
                        Err(d) => return Err(d),
                    }
                };
                let sp = span_of(e);
                if lt.ty == Ty::Null || rt.ty == Ty::Null {
                    let other = if lt.ty == Ty::Null { &rt } else { &lt };
                    if !matches!(*op, "=" | "!=") {
                        return Err(Diag::new("DEF.TYPE_MISMATCH", "null은 = 또는 != 로만 비교", sp));
                    }
                    // 익명 요청에서 actor는 실제로 NULL이다. `actor != null`이 "로그인한 사용자"를 표현하는 유일한 방법이다.
                    let bare_actor = [l, r].iter().any(|x| matches!(x.as_ref(), Expr::Path(p, _) if p.len() == 1 && p[0] == "actor"));
                    if !other.nullable && !bare_actor {
                        return Err(Diag::new("DEF.NULL_COMPARE_NON_NULLABLE", format!("null이 될 수 없는 `{}`를 null과 비교", other.show()), sp));
                    }
                } else if !same_target(&lt.ty, &rt.ty) {
                    return Err(Diag::new("DEF.TYPE_MISMATCH", format!("`{}` {op} `{}`", lt.show(), rt.show()), sp));
                } else if !matches!(*op, "=" | "!=") && !matches!(lt.ty, Ty::Time | Ty::Date | Ty::Int | Ty::Decimal(_, _) | Ty::Text) {
                    return Err(Diag::new("DEF.TYPE_MISMATCH", format!("`{}`는 크기 비교 불가", lt.show()), sp));
                }
                Ok((json!({ "cmp": op, "l": lv, "r": rv }), b))
            }
            Expr::In(l, items) => {
                let (lv, lt) = self.ex(l, sc)?;
                let mut iv = vec![];
                for it in items {
                    let (v, t) = self.expect(it, sc, Some(&lt))?;
                    if !same_target(&t.ty, &lt.ty) {
                        return Err(Diag::new("DEF.TYPE_MISMATCH", format!("in 목록 `{}` vs `{}`", lt.show(), t.show()), span_of(it)));
                    }
                    iv.push(v);
                }
                // in은 집합 소속이라 원소 순서를 정규화한다(F05).
                iv.sort_by_key(|v| v.to_string());
                iv.dedup();
                Ok((json!({ "in": lv, "items": iv }), b))
            }
            Expr::Path(segs, sp) => self.path(segs, *sp, sc),
            Expr::Null => Ok((json!({ "lit": null }), TT { ty: Ty::Null, nullable: true })),
            Expr::Now => Ok((json!({ "now": true }), TT::new(Ty::Time))),
            Expr::Int(n) => Ok((json!({ "lit": n }), TT::new(Ty::Int))),
            Expr::Bool(v) => Ok((json!({ "lit": v }), TT::new(Ty::Bool))),
            Expr::Str(s) => Ok((json!({ "lit": s }), TT::new(Ty::Text))),
            Expr::Call(n, args, sp) => {
                let p = match self.preds.get(n.as_str()) {
                    Some(p) => *p,
                    None => return Err(Diag::new("DEF.UNKNOWN_SYMBOL", format!("predicate `{n}` 없음"), *sp)),
                };
                self.args_against(n, &p.params, args, sc, *sp).map(|av| (json!({ "call": n, "args": av }), b))
            }
            Expr::Exists(r, cond, sp) => {
                if !self.res.contains_key(r.as_str()) {
                    return Err(Diag::new("DEF.UNKNOWN_SYMBOL", format!("resource `{r}` 없음"), *sp));
                }
                let mut inner = sc.clone();
                inner.this = Some(r.clone());
                inner.in_exists = true;
                let cv = self.bool_of(cond, &inner)?;
                // 정책 안 하위 조회는 서버 정의로서 대상 resource의 행 정책 없이 평가된다(spike 규칙, 명시 사실로 남김).
                Ok((json!({ "exists": r, "where": cv, "evaluatedAs": "serverPolicy" }), b))
            }
        }
    }

    fn args_against(&self, n: &str, params: &Params, args: &[Expr], sc: &Scope, sp: Span) -> Result<Vec<Value>, Diag> {
        if params.len() != args.len() {
            return Err(Diag::new("DEF.ARITY_MISMATCH", format!("`{n}` 인자 {}개 필요, {}개", params.len(), args.len()), sp));
        }
        let mut av = vec![];
        for ((pn, pt), a) in params.iter().zip(args) {
            let want = self.plain_type(pt).ok_or_else(|| Diag::new("DEF.UNKNOWN_TYPE", format!("`{n}.{pn}` 타입"), sp))?;
            let (v, t) = self.expect(a, sc, Some(&want))?;
            if !same_target(&t.ty, &want.ty) || (t.nullable && !want.nullable) {
                return Err(Diag::new("DEF.TYPE_MISMATCH", format!("`{n}`의 `{pn}`는 `{}`, 받은 값 `{}`", want.show(), t.show()), span_of(a)));
            }
            av.push(v);
        }
        Ok(av)
    }

    fn plain_type(&self, t: &TypeRef) -> Option<TT> {
        let ty = if t.id_of {
            Ty::Id(t.name.clone())
        } else {
            match t.name.as_str() {
                "Text" => Ty::Text,
                "Email" => Ty::Email,
                "Url" => Ty::Url,
                "Time" => Ty::Time,
                "Date" => Ty::Date,
                "Int" => Ty::Int,
                "Decimal" => Ty::Decimal(t.decimal?.0, t.decimal?.1),
                "Bool" => Ty::Bool,
                n if self.enums.contains_key(n) => Ty::Enum(n.into()),
                n if self.res.contains_key(n) => Ty::Ref(n.into()),
                _ => return None,
            }
        };
        Some(TT { ty, nullable: t.nullable })
    }

    /// 전이의 `from`·`to` 값 식이 field read 정책 필드를 읽으면 막는다(감사 O-02). 결과(`WIRE.INVALID_STATE`, `changed`/`unchanged`,
    /// 복사된 값)로 가려진 값이 추론되기 때문이다. facts의 path를 따라 참조 이동과 exists 안쪽 resource까지 본다.
    fn reject_hidden_reads(&mut self, v: &Value, res: &str, what: &str, sp: Span) {
        let mut hits = vec![];
        self.hidden_reads(v, res, &mut hits);
        hits.sort();
        hits.dedup();
        for h in hits {
            self.d(
                "DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION",
                format!("{what}가 field read 정책이 있는 `{h}`를 읽을 수 없음(결과로 가려진 값이 추론됨). 가려지지 않은 필드로 조건을 쓴다"),
                sp,
            );
        }
    }

    fn hidden_reads(&self, v: &Value, cur: &str, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                if let Some(p) = o.get("path") {
                    if p["root"] == "this" {
                        let mut at = cur.to_string();
                        for seg in p["segs"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                            let Some(rs) = self.res.get(at.as_str()) else { break };
                            if rs.field_read.iter().any(|x| x.0 == seg) {
                                out.push(format!("{at}.{seg}"));
                                break;
                            }
                            match self.field_tt(&at, seg) {
                                Some(TT { ty: Ty::Ref(next), .. }) => at = next,
                                _ => break,
                            }
                        }
                    }
                    return;
                }
                if let Some(inner) = o.get("exists").and_then(Value::as_str) {
                    if let Some(w) = o.get("where") {
                        self.hidden_reads(w, inner, out);
                    }
                    return;
                }
                for x in o.values() {
                    self.hidden_reads(x, cur, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| self.hidden_reads(x, cur, out)),
            _ => {}
        }
    }

    fn run<T>(&mut self, r: Result<T, Diag>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(d) => {
                self.diags.push(d);
                None
            }
        }
    }

    fn build(&mut self) -> Value {
        let spec = self.spec;
        let mut out = Map::new();
        out.insert("actor".into(), json!(self.actor));
        // 값 순서는 의미가 없다(DB는 `CHECK (col IN (...))`). 순서만 바꾼 변경이 계약 변경이 되지 않도록 정렬해
        // digest, DDL, 생성 타입이 모두 같은 순서를 본다(창시자 결정 2026-10-13, 감사 F12).
        out.insert(
            "enums".into(),
            Value::Object(
                spec.enums
                    .iter()
                    .map(|e| {
                        let mut values = e.variants.clone();
                        values.sort();
                        (e.name.clone(), json!(values))
                    })
                    .collect(),
            ),
        );

        let mut preds = Map::new();
        for p in &spec.predicates {
            let (vars, pj) = self.params(&p.params);
            let mut sc = self.base_scope();
            sc.vars.extend(vars);
            let body = self.bool_of(&p.body, &sc);
            if let Some(b) = self.run(body) {
                preds.insert(p.name.clone(), json!({ "params": pj, "body": b }));
            }
        }
        out.insert("predicates".into(), Value::Object(preds));

        let mut acc = Map::new();
        for a in &spec.accesses {
            let (vars, pj) = self.params(&a.params);
            match &a.body {
                AccessBody::TotalOfVisible(r, sp) => {
                    if !a.params.is_empty() {
                        self.d("DEF.TYPE_MISMATCH", "totalOfVisible access는 매개변수를 받지 않음", a.span);
                    }
                    if !self.res.contains_key(r.as_str()) {
                        self.d("DEF.UNKNOWN_SYMBOL", format!("resource `{r}` 없음"), *sp);
                    }
                    acc.insert(a.name.clone(), json!({ "kind": "totalOfVisible", "parent": r }));
                }
                AccessBody::Guard(e) => {
                    let mut sc = Scope::default();
                    sc.vars.extend(vars);
                    let body = self.bool_of(e, &sc);
                    if let Some(b) = self.run(body) {
                        acc.insert(a.name.clone(), json!({ "kind": "guard", "params": pj, "body": b }));
                    }
                }
            }
        }
        out.insert("accesses".into(), Value::Object(acc));

        let mut lim = Map::new();
        for l in &spec.limits {
            if !self.res.contains_key(l.on.as_str()) {
                self.d("DEF.UNKNOWN_SYMBOL", format!("limit 대상 resource `{}` 없음", l.on), l.span);
                continue;
            }
            if l.at_most < 1 {
                self.d("DEF.TYPE_MISMATCH", "atMost는 1 이상", l.span);
            }
            let sc = Scope { this: Some(l.on.clone()), ..Default::default() };
            let c = self.bool_of(&l.cond, &sc);
            if let Some(c) = self.run(c) {
                lim.insert(l.name.clone(), json!({ "on": l.on, "atMost": l.at_most, "where": c }));
            }
        }
        out.insert("limits".into(), Value::Object(lim.clone()));

        let mut rs = Map::new();
        for r in &spec.resources {
            let v = self.resource(r, &lim);
            rs.insert(r.name.clone(), v);
        }
        out.insert("resources".into(), Value::Object(rs));
        if self.principal_actor() {
            out.insert("actorMode".into(), json!("principal"));
        }
        if !spec.operations.is_empty() {
            let mut operations = Map::new();
            for operation in &spec.operations {
                if let Some(value) = self.operation(operation) {
                    operations.insert(operation.name.clone(), value);
                }
            }
            out.insert("operations".into(), Value::Object(operations));
        }
        let mut facts = Value::Object(out);
        crate::isolation::mark(&mut facts);
        facts
    }

    fn resource(&mut self, r: &Resource, limits: &Map<String, Value>) -> Value {
        let mut o = Map::new();
        if let Some(id) = r.fields.iter().find(|f| f.name == "id") {
            if id.ty.name == "Uuid" {
                o.insert("idKind".into(), json!("uuid"));
            } else if !id.ty.id_of && self.res.contains_key(id.ty.name.as_str()) {
                // `id: User` explicitly opts this resource into User's identity space (a shared PK/FK).
                let target = self.res.get(id.ty.name.as_str()).expect("checked above");
                let target_uuid = target.fields.iter().any(|field| field.name == "id" && field.ty.name == "Uuid");
                if target_uuid {
                    o.insert("idKind".into(), json!("uuid"));
                }
                o.insert("sharedPrimaryKey".into(), json!(id.ty.name));
            }
        }
        // 같은 resource 안 이름 충돌은 마지막 선언이 앞을 덮어쓰지 않게 거부한다(F09).
        let mut names: BTreeSet<String> = r.fields.iter().map(|f| f.name.clone()).collect();
        for (n, sp) in r.aggregates.iter().map(|a| (&a.name, a.span)) {
            if !names.insert(n.clone()) {
                self.d("DEF.DUPLICATE", format!("`{n}`이 필드/다른 집계와 이름 충돌"), sp);
            }
        }
        let mut seen = BTreeSet::new();
        for (n, sp) in r.transitions.iter().map(|t| (&t.name, t.span)).chain(r.extensions.iter().map(|x| (&x.name, x.span))) {
            if !seen.insert(n.clone()) {
                self.d("DEF.DUPLICATE", format!("동작 이름 `{n}` 중복"), sp);
            }
            self.reserved_name(n, sp);
        }
        let mut seen = BTreeSet::new();
        for (f, _, sp) in &r.field_read {
            if !seen.insert(f.clone()) {
                self.d("DEF.DUPLICATE", format!("필드 `{f}`의 read 정책 중복"), *sp);
            }
        }
        let mut seen = BTreeSet::new();
        for (n, _, sp) in &r.invariants {
            if !seen.insert(n.clone()) {
                self.d("DEF.DUPLICATE", format!("invariant `{n}` 중복"), *sp);
            }
        }
        let mut seen = BTreeSet::new();
        for (n, sp) in &r.expose_aggregates {
            if !seen.insert(n.clone()) {
                self.d("DEF.DUPLICATE", format!("expose aggregate `{n}` 중복"), *sp);
            }
        }
        if let Some(e) = &r.expose_read {
            let mut seen = BTreeSet::new();
            let items = e
                .select
                .iter()
                .map(|(n, sp)| (format!("select {n}"), *sp))
                .chain(e.filter.iter().map(|(f, op, sp)| (format!("filter {f}.{op}"), *sp)))
                .chain(e.sort.iter().map(|(n, sp)| (format!("sort {n}"), *sp)))
                .chain(e.traverse.iter().map(|(n, _, sp)| (format!("traverse {n}"), *sp)))
                .chain(e.traverse_many.iter().map(|m| (format!("traverse {}", m.name), m.span)));
            for (k, sp) in items {
                if !seen.insert(k.clone()) {
                    self.d("DEF.DUPLICATE", format!("expose read `{k}` 중복"), sp);
                }
            }
        }
        let mut fields = Map::new();
        let mut seen = BTreeSet::new();
        for f in &r.fields {
            if !seen.insert(&f.name) {
                self.d("DEF.DUPLICATE", format!("필드 `{}` 중복", f.name), f.span);
            }
            self.reserved_name(&f.name, f.span);
            if f.ty.name == "Uuid" && f.name != "id" {
                self.d("DEF.TYPE_MISMATCH", "Uuid는 기본키 id에만 선언한다. 참조 필드는 resource 이름으로 선언한다", f.span);
            }
            if f.ty.id_of {
                // 창시자 결정(2026-10-13): 객체 그래프 탐색이 되므로 "X에 속한다"의 철자는 참조 `X` 하나만 둔다.
                // 기본 키도 같은 원칙이다(같은 뜻에 두 표기 금지): `id: Note.Id`는 맨 `Id`와 같은 열이라 철자만 둘이 된다.
                let msg = if f.name == "id" {
                    format!("기본 키는 `id: Id` 한 가지로만 선언한다. `id: {}.Id`는 쓸 수 없음", f.ty.name)
                } else {
                    format!("필드 `{}`는 `{}.Id`로 선언할 수 없음. `{}: {}`(참조)로 쓴다", f.name, f.ty.name, f.name, f.ty.name)
                };
                self.d("DEF.ID_FIELD_NEEDS_REFERENCE", msg, f.ty.span);
                continue;
            }
            if let Some(tt) = self.type_of(Some(&r.name), &f.ty) {
                if let Ty::Ref(target) = &tt.ty {
                    if !self.res[target.as_str()].fields.iter().any(|field| field.name == "id") {
                        self.d("DEF.TYPE_MISMATCH", format!("Ref 대상 resource `{target}`에는 명시적 id가 필요"), f.ty.span);
                        continue;
                    }
                }
                fields.insert(f.name.clone(), json!({ "ty": tt.show(), "range": f.ty.range.map(|(a, b)| json!([a, b])) }));
                if f.ty.database_default {
                    fields.get_mut(&f.name).unwrap()["databaseDefault"] = json!(true);
                }
            }
        }
        // 비공개 관계 테이블은 id 없이도 exists의 원본이 될 수 있다.
        if let Some(id) = fields.get("id") {
            let valid_self_id = id["ty"] == format!("Id<{}>", r.name);
            let valid_shared_id = id["ty"].as_str().is_some_and(|ty| {
                !ty.ends_with('?') && ty.strip_prefix("Ref<").and_then(|inner| inner.strip_suffix('>')).is_some_and(|target| target != r.name)
            });
            if !valid_self_id && !valid_shared_id {
                self.d("DEF.TYPE_MISMATCH", format!("`{}.id`는 non-null 자기 Id 또는 공유 기본키 참조여야 함", r.name), r.span);
            }
        }
        o.insert("fields".into(), Value::Object(fields));

        let mut sc = self.base_scope();
        sc.this = Some(r.name.clone());
        let rows = match &r.row_read {
            // 행 정책이 없는 resource는 전부 거부가 기본이다. 허용을 기본으로 두면 정책 누락이 곧 노출이 된다.
            None => json!({ "default": "denyAll" }),
            Some(e) => {
                let v = self.bool_of(e, &sc);
                self.run(v).unwrap_or(Value::Null)
            }
        };
        o.insert("rowRead".into(), rows);

        let mut fr = Map::new();
        for (f, e, sp) in &r.field_read {
            if !r.fields.iter().any(|x| &x.name == f) {
                self.d("DEF.UNKNOWN_FIELD", format!("field 정책 대상 `{f}` 없음"), *sp);
                continue;
            }
            let v = self.bool_of(e, &sc);
            if let Some(v) = self.run(v) {
                fr.insert(f.clone(), v);
            }
        }
        o.insert("fieldRead".into(), Value::Object(fr));

        let mut aggs = Map::new();
        for a in &r.aggregates {
            if let Some(v) = self.aggregate(r, a) {
                aggs.insert(a.name.clone(), v);
            }
        }

        if let Some(e) = &r.expose_read {
            let v = self.expose(r, e);
            o.insert("exposeRead".into(), v);
        }
        let mut ea = vec![];
        for (n, sp) in &r.expose_aggregates {
            if !r.aggregates.iter().any(|a| &a.name == n) {
                self.d("DEF.UNKNOWN_SYMBOL", format!("노출할 aggregate `{n}` 없음"), *sp);
            }
            ea.push(n.clone());
        }
        ea.sort();
        o.insert("exposeAggregates".into(), json!(ea));
        o.insert("aggregates".into(), Value::Object(aggs));

        let mut tr = Map::new();
        for t in &r.transitions {
            let from = self.bool_of(&t.from, &sc);
            let from = self.run(from);
            if let Some(from) = &from {
                self.reject_hidden_reads(from, &r.name, &format!("transition `{}`의 from", t.name), t.span);
            }
            let allow = self.bool_of(&t.allow, &sc);
            let allow = self.run(allow);
            let mut to = Map::new();
            let mut counters = false;
            for (f, val) in &t.to {
                if to.contains_key(f) {
                    self.d("DEF.DUPLICATE", format!("transition `{}`의 `{f}` 대입 중복", t.name), t.span);
                    continue;
                }
                let Some(ft) = self.field_tt(&r.name, f) else {
                    self.d("DEF.UNKNOWN_FIELD", format!("transition 대상 필드 `{f}` 없음"), t.span);
                    continue;
                };
                if let Expr::Arith(op, l, r, sp) = val {
                    if let Some(v) = self.run(self.counter(f, &ft, op, l, r, *sp)) {
                        counters = true;
                        to.insert(f.clone(), v);
                    }
                    continue;
                }
                let range = r.fields.iter().find(|x| &x.name == f).and_then(|x| x.ty.range);
                if let (Expr::Int(n), Some((lo, hi))) = (val, range) {
                    if *n < lo || *n > hi {
                        self.d("DEF.BAD_RANGE", format!("transition `{}`: `{f}` = {n}은 범위 {lo}..{hi} 밖", t.name), t.span);
                        continue;
                    }
                }
                let res = self.expect(val, &sc, Some(&ft)).and_then(|(v, vt)| {
                    if (same_target(&vt.ty, &ft.ty) && (!vt.nullable || ft.nullable)) || (vt.ty == Ty::Null && ft.nullable) {
                        Ok(v)
                    } else {
                        Err(Diag::new("DEF.TYPE_MISMATCH", format!("`{f}`에 `{}` 대입 불가", vt.show()), t.span))
                    }
                });
                if let Some(v) = self.run(res) {
                    // 값 쪽이 가려진 필드를 읽으면 그 값이 보이는 필드로 복사된다.
                    self.reject_hidden_reads(&v, &r.name, &format!("transition `{}`의 to `{f}` 값", t.name), t.span);
                    to.insert(f.clone(), v);
                }
            }
            // repeat unchanged는 "이미 목표 값"을 `unchanged`로 돌려주므로 가려진 필드에 쓰면 같음 여부가 새어 나간다.
            if t.repeat.as_ref().is_some_and(|(x, _)| x == "unchanged") {
                if let Some(f) = t.to.iter().map(|(f, _)| f).find(|f| r.field_read.iter().any(|x| &x.0 == *f)) {
                    self.d(
                        "DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION",
                        format!("transition `{}`: `{f}`에는 field read 정책이 있어 repeat unchanged와 함께 쓸 수 없음(`unchanged`가 가려진 값과 같음을 알려 줌). repeat를 빼거나 reject로 쓴다", t.name),
                        t.span,
                    );
                }
            }
            if counters && t.repeat.as_ref().is_some_and(|(r, _)| r == "unchanged") {
                // 증감은 매번 값이 달라져 "이미 목표 상태"가 없다. unchanged는 재시도를 조용히 성공시키는 것으로 오해된다.
                self.d("DEF.UNSUPPORTED", format!("transition `{}`: 증감 대입에는 repeat unchanged를 쓸 수 없음", t.name), t.span);
            }
            let repeat = match &t.repeat {
                None => "reject".to_string(),
                Some((r, _)) if r == "unchanged" || r == "reject" => r.clone(),
                Some((r, sp)) => {
                    self.d("DEF.UNSUPPORTED", format!("repeat `{r}`는 unchanged/reject만"), *sp);
                    r.clone()
                }
            };
            let mut effects = vec![];
            for e in &t.effects {
                if let Some(v) = self.effect(r, e, &sc) {
                    effects.push(v);
                }
            }
            if to.contains_key("id") {
                self.d("DEF.UNSUPPORTED_EFFECT", "전이는 잠근 대상의 기본키 id를 바꿀 수 없음", t.span);
            }
            if t.delete {
                if !t.to.is_empty() {
                    self.d("DEF.UNSUPPORTED", format!("transition `{}`: delete 전이는 to 대입과 함께 쓸 수 없음", t.name), t.span);
                }
                if !t.patch.is_empty() {
                    self.d("DEF.UNSUPPORTED", format!("transition `{}`: delete 전이는 patch와 함께 쓸 수 없음", t.name), t.span);
                }
                if !t.expect.is_empty() {
                    self.d("DEF.UNSUPPORTED", format!("transition `{}`: delete 전이는 expect와 함께 쓸 수 없음", t.name), t.span);
                }
                if t.repeat.as_ref().is_some_and(|(r, _)| r == "unchanged") {
                    self.d("DEF.UNSUPPORTED", format!("transition `{}`: delete 전이는 repeat unchanged를 지원하지 않음", t.name), t.span);
                }
            }
            let mut declaration = json!({ "from": from, "to": to, "allow": allow, "repeat": repeat, "effects": effects });
            if t.delete {
                declaration["delete"] = json!(true);
            }
            for (key, names) in [("patch", &t.patch), ("expect", &t.expect)] {
                let mut fields = std::collections::BTreeSet::new();
                for (f, sp) in names {
                    if !fields.insert(f.clone()) {
                        self.d("DEF.DUPLICATE", format!("{key} 필드 `{f}` 중복"), *sp);
                    }
                    if self.field_tt(&r.name, f).is_none() {
                        self.d("DEF.UNKNOWN_FIELD", format!("{key} 필드 `{f}` 없음"), *sp);
                    }
                    if key == "patch" && (f == "id" || t.to.iter().any(|(fixed, _)| fixed == f)) {
                        self.d("DEF.UNSUPPORTED_EFFECT", format!("patch `{f}`는 기본키이거나 to 대입과 겹침"), *sp);
                    }
                    if key == "expect" && r.field_read.iter().any(|(hidden, _, _)| hidden == f) {
                        self.d(
                            "DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION",
                            format!("expect `{f}`에는 field read 정책이 있어 이전 값 비교를 공개할 수 없음"),
                            *sp,
                        );
                    }
                    if key == "expect" && self.field_tt(&r.name, f).is_some_and(|ft| matches!(ft.ty, Ty::Ref(_))) {
                        self.d(
                            "DEF.UNSUPPORTED",
                            "expect는 참조 필드를 비교할 수 없음. 읽기 정책에 가려진 외래 키의 추론을 막는다. version 또는 scalar 필드를 사용한다",
                            *sp,
                        );
                    }
                }
                if !fields.is_empty() {
                    declaration[key] = json!(fields);
                }
            }
            if (!t.patch.is_empty() || !t.expect.is_empty()) && repeat == "unchanged" {
                self.d("DEF.UNSUPPORTED", "patch/expect에는 repeat unchanged를 쓸 수 없음. 같은 key 재시도로 멱등 결과를 복구한다", t.span);
            }
            tr.insert(t.name.clone(), declaration);
        }
        o.insert("transitions".into(), Value::Object(tr));

        let mut ap = Map::new();
        for ea in &r.expose_apply {
            if !r.transitions.iter().any(|t| t.name == ea.transition) {
                self.d("DEF.UNKNOWN_SYMBOL", format!("공개할 전이 `{}` 없음", ea.transition), ea.span);
                continue;
            }
            if ap.contains_key(&ea.transition) {
                self.d("DEF.DUPLICATE", format!("expose apply `{}` 중복", ea.transition), ea.span);
                continue;
            }
            let mut targets = BTreeSet::new();
            for (t, sp) in &ea.targets {
                if t != "id" && t != "where" {
                    self.d("DEF.UNSUPPORTED", format!("target `{t}`는 id/where만"), *sp);
                }
                if !targets.insert(t.clone()) {
                    self.d("DEF.DUPLICATE", format!("target `{t}` 중복"), *sp);
                }
            }
            if targets.is_empty() {
                self.d("DEF.MISSING_ITEM", "expose apply에 target 필요", ea.span);
            }
            // where 대상은 읽기 filter 허용 목록 안에서만 쓴다. 쓰기 조건으로 숨은 값을 추론하지 못하게.
            if targets.contains("where") && r.expose_read.as_ref().is_none_or(|e| e.filter.is_empty()) {
                self.d("DEF.MISSING_ITEM", "where 대상에는 expose read filter 허용 목록이 필요", ea.span);
            }
            match ea.bulk {
                // 상한이 없으면 id 중복 검사와 행 잠금 비용이 요청 하나에 무한히 늘고 `bulk + 1`이 i64를 넘길 수 있다.
                Some(n) if (1..=MAX_BULK_ROWS).contains(&n) => {}
                Some(_) => self.d("DEF.BAD_BUDGET", format!("bulk maxRows는 1 이상 {MAX_BULK_ROWS} 이하"), ea.span),
                None => self.d("DEF.MISSING_ITEM", "expose apply에 bulk maxRows 필요(무제한 기본값 없음)", ea.span),
            }
            let scope = match &ea.same_scope {
                None => Value::Null,
                Some(e) => {
                    let v = self.ex(e, &sc);
                    self.run(v).map(|x| x.0).unwrap_or(Value::Null)
                }
            };
            ap.insert(ea.transition.clone(), json!({ "target": targets, "bulkMaxRows": ea.bulk, "sameScope": scope }));
        }
        o.insert("exposeApply".into(), Value::Object(ap));

        // 생성 공개(W0/W1): allow는 새로 만들 행(this)에 대해 평가한다. 필수 필드는 모두 호출자가 줄 수 있어야 한다.
        let ec = match &r.expose_create {
            None => Value::Null,
            Some(c) => {
                let allow = match &c.allow {
                    Some(e) => {
                        let v = self.bool_of(e, &sc);
                        self.run(v).unwrap_or(Value::Null)
                    }
                    None => {
                        self.d("DEF.MISSING_ITEM", "expose create에 allow 필요", c.span);
                        Value::Null
                    }
                };
                let mut fs = BTreeSet::new();
                for (f, sp) in &c.fields {
                    if !r.fields.iter().any(|x| &x.name == f) {
                        self.d("DEF.UNKNOWN_FIELD", format!("생성 필드 `{f}` 없음"), *sp);
                    }
                    fs.insert(f.clone());
                }
                for f in required_fields(r) {
                    if !fs.contains(&f) {
                        self.d("DEF.MISSING_ITEM", format!("필수 필드 `{f}`가 expose create fields에 없음"), c.span);
                    }
                }
                json!({ "allow": allow, "fields": fs })
            }
        };
        o.insert("exposeCreate".into(), ec);

        let cp = match &r.expose_compose {
            None => Value::Null,
            Some(c) => {
                let scope = c.same_scope.as_ref().and_then(|e| {
                    let v = self.ex(e, &sc);
                    self.run(v).map(|x| x.0)
                });
                let mut trs = BTreeSet::new();
                for (t, sp) in &c.transitions {
                    if !r.transitions.iter().any(|x| &x.name == t) {
                        self.d("DEF.UNKNOWN_SYMBOL", format!("compose 전이 `{t}` 없음"), *sp);
                    }
                    if r.transitions.iter().any(|x| &x.name == t && (!x.patch.is_empty() || !x.expect.is_empty())) {
                        self.d("DEF.UNSUPPORTED", "compose의 전이 단계는 patch/expect 입력을 제공하지 않는다. 이 전이는 bundle.apply로 묶는다", *sp);
                    }
                    trs.insert(t.clone());
                }
                let mut creates = Map::new();
                for (res, srcs, sp) in &c.creates {
                    match self.res.get(res.as_str()) {
                        Some(t) if t.expose_create.is_some() => {}
                        Some(_) => self.d("DEF.MISSING_ITEM", format!("compose create `{res}`에는 그 resource의 expose create(allow)가 필요"), *sp),
                        None => self.d("DEF.UNKNOWN_SYMBOL", format!("resource `{res}` 없음"), *sp),
                    }
                    let mut sv = vec![];
                    for e in srcs {
                        // 출처는 대상 행의 경로만 받는다. 상수·호출은 실행기가 해석하지 않는다(R3-06).
                        if !matches!(e, Expr::Path(..)) {
                            self.d("DEF.UNSUPPORTED", format!("compose create `{res}`의 출처는 대상 행 경로여야 함"), *sp);
                            continue;
                        }
                        let v = self.ex(e, &sc);
                        if let Some((v, _)) = self.run(v) {
                            sv.push(v);
                        }
                    }
                    creates.insert(res.clone(), json!({ "sources": sv }));
                }
                if c.bulk.is_none_or(|b| b < 1) {
                    self.d("DEF.MISSING_ITEM", "expose compose에 bulk maxRows(1 이상) 필요", c.span);
                }
                let self_row = match &c.self_row {
                    None => Value::Null,
                    Some((a, by, sp)) => {
                        let actor_ok = matches!((self.field_tt(&r.name, a), &self.actor), (Some(TT { ty: Ty::Ref(x), .. }), Some(ac)) if &x == ac);
                        let by_ok = matches!(self.field_tt(&r.name, by), Some(TT { ty: Ty::Ref(_), .. }));
                        if !actor_ok || !by_ok {
                            self.d("DEF.TYPE_MISMATCH", "selfRow는 `<actor 참조 필드> by <참조 필드>`", *sp);
                        }
                        // 자기 행은 대상과 같은 범위에서 찾아야 하므로 sameScope는 `by` 필드 그 자체여야 한다(r6 F01).
                        match &c.same_scope {
                            Some(Expr::Path(p, _)) if p.len() == 1 && &p[0] == by => {}
                            _ => self.d("DEF.TYPE_MISMATCH", format!("selfRow `by {by}`와 sameScope가 같은 필드여야 함"), *sp),
                        }
                        json!({ "actorField": a, "by": by })
                    }
                };
                json!({ "bulkMaxRows": c.bulk, "sameScope": scope, "transitions": trs, "creates": creates, "selfRow": self_row })
            }
        };
        o.insert("exposeCompose".into(), cp);

        let mut uq = vec![];
        for (cols, sp) in &r.uniques {
            for (c, csp) in cols {
                if !r.fields.iter().any(|x| &x.name == c) {
                    self.d("DEF.UNKNOWN_FIELD", format!("unique 필드 `{c}` 없음"), *csp);
                }
            }
            let _ = sp;
            uq.push(json!(cols.iter().map(|x| x.0.clone()).collect::<Vec<_>>()));
        }
        o.insert("unique".into(), Value::Array(uq));

        // 커밋 검사: 이 트랜잭션에서 만들거나 바꾼 행이 커밋 전에 만족해야 하는 조건.
        let mut ck = Map::new();
        for (n, e, sp) in &r.checks {
            if ck.contains_key(n) {
                self.d("DEF.DUPLICATE", format!("check `{n}` 중복"), *sp);
                continue;
            }
            let v = self.bool_of(e, &sc);
            if let Some(v) = self.run(v) {
                ck.insert(n.clone(), v);
            }
        }
        o.insert("checks".into(), Value::Object(ck));

        let mut inv = Map::new();
        for (n, per, sp) in &r.invariants {
            let Some(l) = limits.get(n) else {
                self.d("DEF.UNKNOWN_SYMBOL", format!("invariant `{n}` 정의 없음. 실행식 없는 불변식은 허용하지 않음"), *sp);
                continue;
            };
            if l["on"] != json!(r.name) {
                self.d("DEF.TYPE_MISMATCH", format!("`{n}`은 `{}` 대상 정의", l["on"]), *sp);
                continue;
            }
            match self.field_tt(&r.name, per) {
                Some(TT { ty: Ty::Ref(_), .. }) => {}
                _ => {
                    self.d("DEF.UNKNOWN_FIELD", format!("`per {per}`는 이 resource의 참조 필드여야 함"), *sp);
                    continue;
                }
            }
            let deferred = r.deferred_invariants.contains(n);
            // 조건은 DB가 행별로 평가할 수 있는 표현으로 제한한다. N=1은 부분 고유 인덱스,
            // 더 큰 N은 쓰기 경로의 직렬화된 집계 검사로 집행한다.
            if !index_safe(&l["where"]) {
                self.d(
                    "DEF.UNSUPPORTED_INVARIANT",
                    format!("`{n}`: 조건은 이 행의 필드와 리터럴만 사용해야 함(시간·actor·하위조회 의존은 미지원)"),
                    *sp,
                );
                continue;
            }
            let enforcement = if l["atMost"] == json!(1) {
                json!({ "kind": "partialUniqueIndex", "columns": [per], "where": l["where"], "deferred": deferred })
            } else {
                json!({ "kind": "lockedCountCheck", "columns": [per], "where": l["where"], "max": l["atMost"], "deferred": deferred })
            };
            inv.insert(n.clone(), json!({ "per": per, "enforcement": enforcement }));
        }
        o.insert("invariants".into(), Value::Object(inv));

        let mut ext = Map::new();
        for x in &r.extensions {
            if let Some(v) = self.extension(x) {
                ext.insert(x.name.clone(), v);
            }
        }
        o.insert("extensions".into(), Value::Object(ext));
        Value::Object(o)
    }

    fn effect(&mut self, r: &Resource, e: &Effect, sc: &Scope) -> Option<Value> {
        match e {
            Effect::Create { resource, values, span } => {
                let Some(target) = self.res.get(resource.as_str()).copied() else {
                    self.d("DEF.UNKNOWN_SYMBOL", format!("생성 대상 `{resource}` 없음"), *span);
                    return None;
                };
                let mut vals = Map::new();
                for (f, v) in values {
                    if vals.contains_key(f) {
                        self.d("DEF.DUPLICATE", format!("`{resource}.{f}` 값 중복"), *span);
                        continue;
                    }
                    let Some(ft) = self.field_tt(resource, f) else {
                        self.d("DEF.UNKNOWN_FIELD", format!("`{resource}`에 필드 `{f}` 없음"), *span);
                        continue;
                    };
                    let res = self.expect(v, sc, Some(&ft)).and_then(|(val, vt)| {
                        if same_target(&vt.ty, &ft.ty) && (!vt.nullable || ft.nullable) {
                            Ok(val)
                        } else {
                            Err(Diag::new("DEF.TYPE_MISMATCH", format!("`{resource}.{f}`({})에 `{}` 불가", ft.show(), vt.show()), *span))
                        }
                    });
                    if let Some(val) = self.run(res) {
                        vals.insert(f.clone(), val);
                    }
                }
                for f in required_fields(target) {
                    if !vals.contains_key(&f) {
                        self.d("DEF.MISSING_ITEM", format!("`{resource}` 생성에 필수 필드 `{f}` 값 필요"), *span);
                    }
                }
                let _ = r;
                Some(json!({ "create": resource, "values": vals }))
            }
            Effect::Update { resource, max_rows, matches, values, span } => {
                if !self.res.contains_key(resource.as_str()) {
                    self.d("DEF.UNKNOWN_SYMBOL", format!("갱신 대상 `{resource}` 없음"), *span);
                    return None;
                }
                let typed = |pairs: &Vec<(String, Expr)>, what: &str, cx: &mut Self| -> Map<String, Value> {
                    let mut out = Map::new();
                    for (f, v) in pairs {
                        if out.contains_key(f) {
                            cx.d("DEF.DUPLICATE", format!("`{resource}.{f}` {what} 중복"), *span);
                            continue;
                        }
                        let Some(ft) = cx.field_tt(resource, f) else {
                            cx.d("DEF.UNKNOWN_FIELD", format!("`{resource}`에 필드 `{f}` 없음"), *span);
                            continue;
                        };
                        let res = cx.expect(v, sc, Some(&ft)).and_then(|(val, vt)| {
                            if same_target(&vt.ty, &ft.ty) && (!vt.nullable || ft.nullable) {
                                Ok(val)
                            } else {
                                Err(Diag::new("DEF.TYPE_MISMATCH", format!("`{resource}.{f}`({})에 `{}` 불가", ft.show(), vt.show()), *span))
                            }
                        });
                        if let Some(val) = cx.run(res) {
                            out.insert(f.clone(), val);
                        }
                    }
                    out
                };
                let m = typed(matches, "조건", self);
                let v = typed(values, "값", self);
                if m.is_empty() || v.is_empty() {
                    self.d("DEF.MISSING_ITEM", "update 효과에는 찾을 조건과 바꿀 값이 필요", *span);
                }
                let mut effect = json!({ "update": resource, "match": m, "values": v });
                if let Some(max) = max_rows {
                    if !(1..=1000).contains(max) {
                        self.d("DEF.BAD_BUDGET", "many update maxRows는 1..1000", *span);
                    }
                    let scope = m.iter().find_map(|(field, value)| {
                        let ty = self.field_tt(resource, field)?;
                        let parent = matches!(&ty.ty, Ty::Ref(target) if target == &r.name) && !ty.nullable;
                        // this.id는 기존 경로 정규화에서 this와 같은 빈 segment가 된다.
                        (parent && value["path"]["root"] == "this" && value["path"]["segs"] == json!([])).then_some(field)
                    });
                    match scope {
                        None => self.d("DEF.UNSUPPORTED_EFFECT", "many update에는 non-null 부모 참조 = this.id 조건 필요", *span),
                        Some(field) if v.contains_key(field) => {
                            self.d("DEF.UNSUPPORTED_EFFECT", "many update의 부모 범위 참조는 바꿀 수 없음", *span);
                        }
                        Some(_) => {}
                    }
                    effect["maxRows"] = json!(max);
                }
                Some(effect)
            }
            Effect::Delete { resource, max_rows, matches, span } => {
                if !self.res.contains_key(resource.as_str()) {
                    self.d("DEF.UNKNOWN_SYMBOL", format!("삭제 대상 `{resource}` 없음"), *span);
                    return None;
                }
                let typed = |pairs: &Vec<(String, Expr)>, what: &str, cx: &mut Self| -> Map<String, Value> {
                    let mut out = Map::new();
                    for (f, v) in pairs {
                        if out.contains_key(f) {
                            cx.d("DEF.DUPLICATE", format!("`{resource}.{f}` {what} 중복"), *span);
                            continue;
                        }
                        let Some(ft) = cx.field_tt(resource, f) else {
                            cx.d("DEF.UNKNOWN_FIELD", format!("`{resource}`에 필드 `{f}` 없음"), *span);
                            continue;
                        };
                        let res = cx.expect(v, sc, Some(&ft)).and_then(|(val, vt)| {
                            if same_target(&vt.ty, &ft.ty) && (!vt.nullable || ft.nullable) {
                                Ok(val)
                            } else {
                                Err(Diag::new("DEF.TYPE_MISMATCH", format!("`{resource}.{f}`({})에 `{}` 불가", ft.show(), vt.show()), *span))
                            }
                        });
                        if let Some(val) = cx.run(res) {
                            out.insert(f.clone(), val);
                        }
                    }
                    out
                };
                let m = typed(matches, "조건", self);
                if m.is_empty() {
                    self.d("DEF.MISSING_ITEM", "delete 효과에는 찾을 조건이 필요", *span);
                }
                let mut effect = json!({ "delete": resource, "match": m });
                if let Some(max) = max_rows {
                    if !(1..=1000).contains(max) {
                        self.d("DEF.BAD_BUDGET", "many delete maxRows는 1..1000", *span);
                    }
                    let scope = m.iter().find_map(|(field, value)| {
                        let ty = self.field_tt(resource, field)?;
                        let parent = matches!(&ty.ty, Ty::Ref(target) if target == &r.name);
                        (parent && value["path"]["root"] == "this" && value["path"]["segs"] == json!([])).then_some(field)
                    });
                    if scope.is_none() {
                        self.d("DEF.UNSUPPORTED_EFFECT", "many delete에는 부모 참조 = this.id 조건 필요", *span);
                    }
                    effect["maxRows"] = json!(max);
                }
                Some(effect)
            }
            Effect::Notify { to, topic, span } => {
                let res = self.ex(to, sc).and_then(|(v, t)| match (&t.ty, &self.actor) {
                    (Ty::Ref(x), Some(a)) if x == a && !t.nullable => Ok(v),
                    _ => Err(Diag::new("DEF.TYPE_MISMATCH", format!("notify 대상은 actor 타입 참조여야 함, `{}`", t.show()), *span)),
                });
                let v = self.run(res)?;
                Some(json!({ "notify": { "to": v, "topic": topic } }))
            }
        }
    }

    fn aggregate(&mut self, r: &Resource, a: &Aggregate) -> Option<Value> {
        let tt = self.type_of(Some(&r.name), &a.ty)?;
        match a.caller_filter.as_deref() {
            None => self.d("DEF.MISSING_ITEM", format!("aggregate `{}`에 `callerFilter` 필요", a.name), a.span),
            Some("none" | "read") => {}
            Some(x) => self.d("DEF.UNSUPPORTED", format!("`callerFilter {x}`는 spike에서 지원하지 않음(허용: none, read)"), a.span),
        }
        match a.row_output.as_deref() {
            None => self.d("DEF.MISSING_ITEM", format!("aggregate `{}`에 `rowOutput` 필요", a.name), a.span),
            Some("none") => {}
            Some(x) => self.d("DEF.UNSUPPORTED", format!("`rowOutput {x}`는 spike에서 지원하지 않음(허용: none)"), a.span),
        }
        let source = a.source.clone().unwrap_or_else(|| r.name.clone());
        if !self.res.contains_key(source.as_str()) {
            self.d("DEF.UNKNOWN_SYMBOL", format!("집계 source `{source}` 없음"), a.span);
            return None;
        }
        if a.caller_filter.as_deref() == Some("read")
            && a.where_.is_none()
            && self.res.get(source.as_str()).is_some_and(|source| source.row_read.is_some())
        {
            self.advisory(
                "DEF.ADVISORY_CALLER_FILTER_POLICY",
                format!("aggregate `{}` uses `callerFilter read` without `where` although `{source}` has a row read policy; its count can reveal matching rows the caller cannot read", a.name),
                a.span,
            );
        }
        let (release, release_field) = match a.release.as_deref() {
            None => {
                self.d("DEF.MISSING_ITEM", format!("aggregate `{}`에 `release` 필요", a.name), a.span);
                return None;
            }
            Some("count") => ("count", None),
            Some(x) => match x.strip_suffix(')').and_then(|x| x.split_once('(')) {
                Some((f @ ("sum" | "min" | "max"), field)) => (f, Some(field.trim().to_string())),
                _ => {
                    self.d("DEF.UNSUPPORTED", format!("`release {x}`는 지원하지 않음(허용: count, sum(f), min(f), max(f))"), a.span);
                    return None;
                }
            },
        };
        match &release_field {
            None if tt.ty != Ty::Int => self.d("DEF.TYPE_MISMATCH", "count 집계 타입은 Int", a.span),
            None => {}
            Some(field) => {
                match self.field_tt(&source, field) {
                    Some(TT { ty: Ty::Int, .. }) => {}
                    Some(t) => self.d("DEF.TYPE_MISMATCH", format!("`{release}({field})`는 Int 필드만, `{}`", t.show()), a.span),
                    None => self.d("DEF.UNKNOWN_FIELD", format!("집계 대상 `{source}.{field}` 없음"), a.span),
                }
                // count는 행 수만 내보내지만 sum/min/max는 값을 드러낸다. 가려진 필드의 합계로 값이 새지 않게 한다.
                if self.res.get(source.as_str()).is_some_and(|s| s.field_read.iter().any(|x| &x.0 == field)) {
                    self.d(
                        "DEF.POLICY_FIELD_NOT_AGGREGATABLE",
                        format!("`{source}.{field}`에는 field read 정책이 있어 {release}로 집계할 수 없음"),
                        a.span,
                    );
                }
                if tt.ty != Ty::Int {
                    self.d("DEF.TYPE_MISMATCH", format!("{release} 집계 타입은 Int"), a.span);
                }
                // 빈 집합의 min/max는 NULL이다. 단독 집계는 NULL을 접근 거부로 읽으므로 행별(groupKey) 집계에서만 연다.
                if release != "sum" && (a.group_key.is_none() || !tt.nullable) {
                    self.d("DEF.UNSUPPORTED", format!("{release} 집계는 groupKey가 있고 타입이 `Int?`여야 함(빈 집합은 null)"), a.span);
                }
            }
        }
        if let Some(g) = &a.group_key {
            match self.field_tt(&source, g) {
                Some(TT { ty: Ty::Ref(t), .. }) if t == r.name => {}
                _ => self.d("DEF.GROUP_KEY_MISMATCH", format!("groupKey `{g}`는 `{source}`에서 `{}`를 가리키는 필드여야 함", r.name), a.span),
            }
        }
        let (input, ij) = self.params(&a.input);
        let access = match &a.source_access {
            None => {
                self.d("DEF.MISSING_ITEM", format!("aggregate `{}`에 sourceAccess 필요. 원본 행 정책을 자동으로 물려받지 않음", a.name), a.span);
                return None;
            }
            Some(AccessRef::Name(n)) => match self.accesses.get(n.as_str()).map(|x| &x.body) {
                Some(AccessBody::TotalOfVisible(p, _)) => {
                    if p != &r.name || a.group_key.is_none() {
                        self.d("DEF.ACCESS_MISMATCH", format!("`{n}`은 `{p}` 기준 groupKey 집계에만 사용"), a.span);
                    }
                    json!({ "ref": n })
                }
                Some(AccessBody::Guard(_)) => {
                    self.d("DEF.ARITY_MISMATCH", format!("guard access `{n}`에는 인자가 필요"), a.span);
                    return None;
                }
                None => {
                    self.d("DEF.UNKNOWN_SYMBOL", format!("access `{n}` 없음"), a.span);
                    return None;
                }
            },
            Some(AccessRef::Call(n, args)) => {
                let acc = match self.accesses.get(n.as_str()) {
                    Some(x) if matches!(x.body, AccessBody::Guard(_)) => *x,
                    Some(_) => {
                        self.d("DEF.ARITY_MISMATCH", format!("`{n}`은 인자를 받지 않음"), a.span);
                        return None;
                    }
                    None => {
                        self.d("DEF.UNKNOWN_SYMBOL", format!("access `{n}` 없음"), a.span);
                        return None;
                    }
                };
                let mut sc = self.base_scope();
                sc.input = input.clone();
                let av = self.args_against(n, &acc.params, args, &sc, a.span);
                json!({ "ref": n, "args": self.run(av)? })
            }
        };
        let where_ = match &a.where_ {
            None => Value::Null,
            Some(e) => {
                let mut sc = self.base_scope();
                sc.this = Some(source.clone());
                sc.input = input;
                let w = self.bool_of(e, &sc);
                self.run(w)?
            }
        };
        let mut out = json!({
            "ty": tt.show(), "tyRange": a.ty.range.map(|(x, y)| json!([x, y])), "input": ij, "source": source, "sourceAccess": access, "groupKey": a.group_key,
            "where": where_, "callerFilter": a.caller_filter, "rowOutput": a.row_output, "release": release,
        });
        // count 집계의 facts는 그대로 둔다. 키를 늘리면 기존 배포의 실행 digest가 바뀐다.
        if let Some(f) = release_field {
            out["releaseField"] = json!(f);
        }
        Some(out)
    }

    fn expose(&mut self, r: &Resource, e: &ExposeRead) -> Value {
        let mut kinds = Map::new();
        let mut open_ids = BTreeSet::new();
        for (n, sp) in &e.ref_ids {
            if !open_ids.insert(n.as_str()) {
                self.d("DEF.DUPLICATE", format!("refIds `{n}` 중복"), *sp);
            } else if !e.select.iter().any(|(s, _)| s == n) || !r.fields.iter().any(|f| &f.name == n) {
                self.d("DEF.UNKNOWN_FIELD", format!("refIds `{n}`: `{}`의 expose select에 있는 필드여야 함", r.name), *sp);
            } else if !matches!(self.field_tt(&r.name, n), Some(TT { ty: Ty::Ref(_), .. })) {
                self.d("DEF.TYPE_MISMATCH", format!("refIds `{n}`는 참조 필드가 아님"), *sp);
            }
        }
        for (s, sp) in &e.select {
            if r.fields.iter().any(|f| &f.name == s) {
                let policy = r.field_read.iter().any(|x| &x.0 == s);
                // 창시자 결정(2026-10-13, O-01): 참조를 직접 select하면 기본은 대상 행 정책을 적용한다(안 보이면 null).
                // refIds로 선언한 참조만 원시 id를 그대로 공개한다.
                let guarded = matches!(self.field_tt(&r.name, s), Some(TT { ty: Ty::Ref(_), .. })) && !open_ids.contains(s.as_str());
                let k = match (guarded, policy) {
                    (true, true) => "refVisibleWithPolicy",
                    (true, false) => "refVisible",
                    (false, true) => "fieldWithPolicy",
                    (false, false) => "field",
                };
                kinds.insert(s.clone(), json!(k));
            } else if let Some(a) = r.aggregates.iter().find(|a| &a.name == s) {
                if a.group_key.is_none() || !a.input.is_empty() {
                    self.d("DEF.AGG_NOT_SELECTABLE", format!("`{s}`는 행별 집계가 아니어서 select 불가"), *sp);
                }
                kinds.insert(s.clone(), json!("aggregate"));
            } else {
                self.d("DEF.UNKNOWN_FIELD", format!("select `{s}`: `{}`에 필드/집계 없음", r.name), *sp);
            }
        }
        let mut filters = BTreeSet::new();
        for (f, op, sp) in &e.filter {
            let Some(tt) = self.field_tt(&r.name, f) else {
                self.d("DEF.UNKNOWN_FIELD", format!("filter 대상 `{f}` 없음"), *sp);
                continue;
            };
            if r.field_read.iter().any(|x| &x.0 == f) {
                self.d(
                    "DEF.POLICY_FIELD_NOT_FILTERABLE",
                    format!("`{f}`에는 field read 정책이 있어 filter에 올릴 수 없음(가려진 값이 결과 유무로 추론됨)"),
                    *sp,
                );
            }
            let ok = match op.as_str() {
                // Ref는 대상 행의 Id로 비교한다(값은 Id wire를 따른다). 같은 검사를 필드 단위로 거치므로 filter 대상 규칙은 연산자와 무관하게 적용된다.
                "eq" => true,
                // in: 값 배열. 범위 비교 대상(Time/Int)과 Bool은 제외한다. 배열 길이 상한은 호출 시점(plan.rs MAX_IN_VALUES)에서 검사한다.
                "in" => matches!(tt.ty, Ty::Text | Ty::Email | Ty::Enum(_) | Ty::Id(_) | Ty::Ref(_)),
                // isNull: bool 값. 항상 값이 있는 필드에는 의미가 없으므로 nullable 필드에만 연다.
                "isNull" => tt.nullable,
                "gte" | "lte" | "gt" | "lt" => matches!(tt.ty, Ty::Time | Ty::Date | Ty::Int | Ty::Decimal(_, _) | Ty::Id(_)),
                "prefix" => tt.ty == Ty::Text,
                // contains: 리터럴 부분일치(Text만). 대소문자 무시(icontains)는 DB collation 의존이라 보류.
                "contains" => tt.ty == Ty::Text,
                // like: SQL LIKE pattern semantics; % and _ are wildcards (Text only).
                "like" => tt.ty == Ty::Text,
                _ => {
                    self.d("DEF.UNKNOWN_OPERATOR", format!("filter 연산 `{op}` 없음"), *sp);
                    continue;
                }
            };
            if !ok {
                self.d("DEF.OP_TYPE_MISMATCH", format!("`{f}`({})에 `{op}` 불가", tt.show()), *sp);
            }
            filters.insert(format!("{f}.{op}"));
        }
        let mut sorts = BTreeSet::new();
        for (s, sp) in &e.sort {
            if r.field_read.iter().any(|x| &x.0 == s) {
                self.d(
                    "DEF.POLICY_FIELD_NOT_FILTERABLE",
                    format!("`{s}`에는 field read 정책이 있어 sort에 올릴 수 없음(가려진 값이 정렬 순서로 추론됨)"),
                    *sp,
                );
            }
            match self.field_tt(&r.name, s) {
                Some(TT { ty: Ty::Ref(_) | Ty::Json, .. }) => self.d("DEF.OP_TYPE_MISMATCH", format!("`{s}`는 정렬 불가 타입"), *sp),
                Some(_) => {}
                None => self.d("DEF.UNKNOWN_FIELD", format!("sort 대상 `{s}` 없음"), *sp),
            }
            sorts.insert(s.clone());
        }
        let mut trav = Map::new();
        for (rel, sel, sp) in &e.traverse {
            let target = match self.field_tt(&r.name, rel) {
                Some(TT { ty: Ty::Ref(t), .. }) => t,
                _ => {
                    self.d("DEF.UNKNOWN_FIELD", format!("traverse `{rel}`는 참조 필드가 아님"), *sp);
                    continue;
                }
            };
            let tres = self.res[target.as_str()];
            let Some(te) = &tres.expose_read else {
                self.d("DEF.TRAVERSE_NOT_EXPOSED", format!("`{target}`에 expose read가 없어 traverse 불가"), *sp);
                continue;
            };
            let mut ss = BTreeSet::new();
            for (s, ssp) in sel {
                if !te.select.iter().any(|x| &x.0 == s) {
                    self.d("DEF.TRAVERSE_NOT_EXPOSED", format!("`{target}.{s}`는 `{target}`의 expose select에 없음"), *ssp);
                }
                ss.insert(s.clone());
            }
            trav.insert(rel.clone(), json!({ "target": target, "select": ss, "reapply": ["rowRead", "fieldRead"] }));
        }
        let mut many = Map::new();
        for m in &e.traverse_many {
            if self.field_tt(&r.name, &m.name).is_some() || e.select.iter().any(|(n, _)| n == &m.name) {
                self.d("DEF.DUPLICATE", format!("traverse `{}`는 `{}`의 필드·select 이름과 겹침", m.name, r.name), m.span);
                continue;
            }
            let Some(cres) = self.res.get(m.child.as_str()).copied() else {
                self.d("DEF.UNKNOWN_RESOURCE", format!("traverse `{}`: resource `{}` 없음", m.name, m.child), m.span);
                continue;
            };
            match self.field_tt(&m.child, &m.via) {
                None => {
                    self.d("DEF.UNKNOWN_FIELD", format!("traverse `{}`: `{}.{}` 필드 없음", m.name, m.child, m.via), m.span);
                    continue;
                }
                Some(TT { ty: Ty::Ref(t), .. }) if t == r.name => {}
                Some(_) => {
                    self.d("DEF.TRAVERSE_VIA_NOT_REF", format!("`{}.{}`는 `{}`를 가리키는 참조 필드가 아님", m.child, m.via, r.name), m.span);
                    continue;
                }
            }
            // 자식 목록에 들어갔다는 사실 자체가 via 값(어느 부모에 속하는지)을 드러낸다.
            if cres.field_read.iter().any(|x| x.0 == m.via) {
                self.d(
                    "DEF.POLICY_FIELD_NOT_FILTERABLE",
                    format!("`{}.{}`에는 field read 정책이 있어 1:N traverse 기준으로 쓸 수 없음", m.child, m.via),
                    m.span,
                );
                continue;
            }
            let Some(te) = &cres.expose_read else {
                self.d("DEF.TRAVERSE_NOT_EXPOSED", format!("`{}`에 expose read가 없어 traverse 불가", m.child), m.span);
                continue;
            };
            if m.select.is_empty() {
                self.d("DEF.MISSING_ITEM", format!("traverse `{}`에 select 필요", m.name), m.span);
            }
            let mut ss = BTreeSet::new();
            for (f, fsp) in &m.select {
                if !te.select.iter().any(|x| &x.0 == f) {
                    self.d("DEF.TRAVERSE_NOT_EXPOSED", format!("`{}.{f}`는 `{}`의 expose select에 없음", m.child, m.child), *fsp);
                } else if cres.aggregates.iter().any(|a| &a.name == f) {
                    // 자식 행마다 집계 하위조회가 붙으면 비용이 부모 수 x 상한 x 집계로 커진다. 보류.
                    self.d("DEF.TRAVERSE_MANY_AGGREGATE", format!("1:N traverse 자식 select에는 집계 `{f}`를 쓸 수 없음(보류)"), *fsp);
                }
                ss.insert(f.clone());
            }
            let sort = match &m.sort {
                None => json!({ "field": "id", "desc": false }),
                Some((f, desc, fsp)) => {
                    if cres.field_read.iter().any(|x| &x.0 == f) {
                        self.d("DEF.POLICY_FIELD_NOT_FILTERABLE", format!("`{f}`에는 field read 정책이 있어 정렬에 쓸 수 없음"), *fsp);
                    }
                    match self.field_tt(&m.child, f) {
                        Some(TT { ty: Ty::Ref(_) | Ty::Json, .. }) => self.d("DEF.OP_TYPE_MISMATCH", format!("`{f}`는 정렬 불가 타입"), *fsp),
                        Some(_) => {}
                        None => self.d("DEF.UNKNOWN_FIELD", format!("sort 대상 `{f}` 없음"), *fsp),
                    }
                    json!({ "field": f, "desc": desc })
                }
            };
            match m.limit {
                None => self.d("DEF.MISSING_ITEM", format!("traverse `{}`에는 자식 개수 상한 limit이 필요", m.name), m.span),
                Some(n) if n < 1 => self.d("DEF.BAD_LIMIT", "traverse limit는 1 이상", m.span),
                // 부모 행 수 × 자식 상한이 비용 추정에 곱해진다. 한 응답의 자식 목록으로 의미 있는 크기만 받는다.
                Some(n) if n > 1000 => self.d("DEF.BAD_LIMIT", "traverse limit는 1000 이하", m.span),
                _ => {}
            }
            many.insert(
                m.name.clone(),
                json!({ "target": m.child, "via": m.via, "select": ss, "sort": sort, "limit": m.limit, "reapply": ["rowRead", "fieldRead"] }),
            );
        }
        // budget이 없으면 무제한 기본값을 두지 않고, 다른 resource의 traverse 대상으로만 쓴다(루트 조회 불가).
        let budget = match &e.budget {
            None => Value::Null,
            Some(b) => {
                if b.rows.is_none() || b.depth.is_none() || b.deadline_ms.is_none() || b.cost.is_none() {
                    self.d("DEF.MISSING_ITEM", "budget에는 rows/depth/deadline/cost가 모두 필요", b.span);
                }
                for (k, v) in
                    [("rows", b.rows), ("depth", b.depth), ("cost", b.cost), ("deadline", b.deadline_ms.map(|x| x as i64)), ("offset", b.offset)]
                {
                    if v.is_some_and(|x| x < 1) {
                        self.d("DEF.BAD_BUDGET", format!("budget {k}는 1 이상"), b.span);
                    }
                }
                if b.cursor {
                    // cursor 경계로 쓰는 값은 호출자가 이미 select로 볼 수 있어야 한다. 아니면 after 비교로 숨은 값을 추론할 수 있다.
                    let selected = |n: &str| e.select.iter().any(|(s, _)| s == n) && kinds.get(n).is_some_and(|k| k == "field");
                    for (s, sp) in &e.sort {
                        if !selected(s) {
                            self.d("DEF.CURSOR_SORT_NOT_SELECTED", format!("cursor를 켜면 sort `{s}`도 정책 없는 select 필드여야 함"), *sp);
                        }
                        // NULL은 비교가 unknown이라 keyset 경계가 행을 빠뜨린다.
                        if self.field_tt(&r.name, s).is_some_and(|t| t.nullable) {
                            self.d("DEF.CURSOR_NULLABLE_SORT", format!("cursor를 켜면 nullable 필드 `{s}`는 sort에 쓸 수 없음"), *sp);
                        }
                    }
                    if !selected("id") {
                        self.d("DEF.CURSOR_ID_NOT_SELECTED", "cursor를 켜면 id가 select에 있어야 함(동률 경계)", b.span);
                    }
                }
                if (!e.traverse.is_empty() || !e.traverse_many.is_empty()) && b.depth.unwrap_or(0) < 2 {
                    self.d("DEF.BUDGET_DEPTH_TOO_SMALL", "traverse가 있으면 depth는 2 이상", b.span);
                }
                let mut bj = json!({ "rows": b.rows, "depth": b.depth, "deadlineMs": b.deadline_ms, "cost": b.cost });
                // offset은 opt-in이라 선언이 있을 때만 키를 넣는다(기존 facts 호환).
                if let Some(o) = b.offset {
                    bj["maxOffset"] = json!(o);
                }
                if b.cursor {
                    bj["cursor"] = json!(true);
                }
                bj
            }
        };
        let root = !budget.is_null();
        let mut out = json!({ "select": kinds, "filter": filters, "sort": sorts, "traverse": trav, "budget": budget, "rootQueryable": root });
        // 1:N traverse는 opt-in이라 선언이 있을 때만 키를 넣는다(기존 facts 호환).
        if !many.is_empty() {
            out["traverseMany"] = Value::Object(many);
        }
        out
    }

    fn operation(&mut self, operation: &Extension) -> Option<Value> {
        let job = operation.kind == "job";
        let managed = job && operation.effect == "managed";
        if (!job && operation.kind != "read") || (!managed && (operation.effect != "none" || !operation.access.is_empty())) {
            self.d("DEF.UNSUPPORTED", "operation은 read/none 또는 job/none·managed 계약이 필요", operation.span);
            return None;
        }
        let max_deadline = if job { 3_600_000 } else { 30_000 };
        if !operation.deadline_ms.is_some_and(|deadline| deadline > 0 && deadline <= max_deadline) {
            self.d("DEF.BAD_BUDGET", "operation deadline은 read 30s, job 1h 이내의 양수여야 함", operation.span);
        }
        for (_, ty) in operation.input.iter().chain(&operation.output) {
            if ty.range.is_some_and(|(lo, hi)| lo < -9_007_199_254_740_991 || hi > 9_007_199_254_740_991) {
                self.d("DEF.BAD_RANGE", "operation 범위 경계는 JavaScript 안전 정수로 표현 가능해야 함", ty.span);
            }
        }
        let (input, _) = self.params(&operation.input);
        let mut scope = self.base_scope();
        scope.input = input;
        let allow = match &operation.allow {
            Some(allow) => {
                let result = self.bool_of(allow, &scope);
                self.run(result)
            }
            None => {
                self.d("DEF.MISSING_ITEM", "operation에 allow 정책 필요", operation.span);
                None
            }
        };
        let mut base = operation.clone();
        base.kind = "read".into();
        base.effect = "none".into();
        base.access.clear();
        let mut value = self.extension(&base)?;
        value["kind"] = json!(operation.kind);
        value["effect"] = json!(operation.effect);
        if managed {
            let mut capabilities = Map::new();
            let mut data_access = false;
            for (domain, name) in &operation.access {
                // 업무 데이터 접근은 job 주체의 기존 read/apply 정책을 그대로 거친다. 선언 없이 열리지 않는다.
                if domain == "read" || self.res.contains_key(domain.as_str()) {
                    if ["http", "files", "events", "jobs", "messages", "read"].contains(&domain.as_str()) && self.res.contains_key(domain.as_str()) {
                        self.d("DEF.AMBIGUOUS", format!("resource 이름 `{domain}`이 managed capability 영역과 겹침"), operation.span);
                        continue;
                    }
                    let key = if domain == "read" {
                        if !self.res.contains_key(name.as_str()) {
                            self.d("DEF.UNKNOWN_SYMBOL", format!("read 대상 resource `{name}`이 없음"), operation.span);
                            continue;
                        }
                        format!("read.{name}")
                    } else {
                        let exposed = self.res.get(domain.as_str()).is_some_and(|r| r.expose_apply.iter().any(|e| &e.transition == name));
                        if !exposed {
                            self.d(
                                "DEF.UNKNOWN_SYMBOL",
                                format!("job 쓰기 access `{domain}.{name}`는 공개된 전이(expose apply)여야 함"),
                                operation.span,
                            );
                            continue;
                        }
                        format!("apply.{domain}.{name}")
                    };
                    data_access = true;
                    if capabilities.insert(key, json!(true)).is_some() {
                        self.d("DEF.DUPLICATE", "managed capability 중복", operation.span);
                    }
                    continue;
                }
                if !["http", "files", "events", "jobs", "messages"].contains(&domain.as_str()) {
                    self.d("DEF.UNKNOWN_SYMBOL", format!("알 수 없는 managed capability `{domain}.{name}`"), operation.span);
                }
                if domain == "messages" && !valid_message_queue(name) {
                    self.d("DEF.BAD_CAPABILITY", format!("메시지 queue 이름 `{name}`은 ASCII 식별자여야 함"), operation.span);
                }
                if domain == "jobs"
                    && !self.spec.operations.iter().any(|target| target.name == *name && matches!(target.kind.as_str(), "read" | "job"))
                {
                    self.d("DEF.UNKNOWN_SYMBOL", format!("managed jobs 대상 operation `{name}`이 없거나 read/job이 아님"), operation.span);
                }
                if capabilities.insert(format!("{domain}.{name}"), json!(true)).is_some() {
                    self.d("DEF.DUPLICATE", "managed capability 중복", operation.span);
                }
            }
            value["access"] = Value::Object(capabilities);
            if data_access {
                value["dependencies"] = json!({"worker":["database"],"authorization":["database","principal","deployment"]});
            }
        }
        value["allow"] = allow?;
        if value["dependencies"].is_null() {
            value["dependencies"] = json!({"worker":[],"authorization":["database","principal","deployment"]});
        }
        Some(value)
    }

    fn extension(&mut self, x: &Extension) -> Option<Value> {
        if x.kind == "write" {
            // 쓰기 확장: 서버 트랜잭션 안에서 공개 전이만 부른다. 효과는 DB 쓰기(db)로 명시한다.
            if x.effect != "db" {
                self.d("DEF.EFFECT_MISMATCH", format!("write extension의 effect는 db여야 함, `{}`", x.effect), x.span);
            }
            if x.deadline_ms.is_none() || x.implementation.is_empty() {
                self.d("DEF.MISSING_ITEM", "extension에 deadline·implementation 필요", x.span);
            }
            let (_, ij) = self.params(&x.input);
            let (_, oj) = self.params(&x.output);
            let mut acc = Map::new();
            for (rn, tn) in &x.access {
                let exposed = self.res.get(rn.as_str()).is_some_and(|r| r.expose_apply.iter().any(|e| &e.transition == tn));
                if !exposed {
                    self.d("DEF.UNKNOWN_SYMBOL", format!("쓰기 access `{rn}.{tn}`는 공개된 전이(expose apply)여야 함"), x.span);
                    continue;
                }
                acc.insert(format!("{rn}.{tn}"), json!("transition"));
            }
            return Some(json!({
                "kind": "write", "input": ij, "output": oj, "access": acc, "effect": x.effect,
                "deadlineMs": x.deadline_ms, "implementation": x.implementation,
            }));
        }
        if x.kind != "read" {
            self.d("DEF.UNSUPPORTED", format!("extension 종류 `{}`는 spike 범위 밖", x.kind), x.span);
            return None;
        }
        if x.effect != "none" {
            self.d("DEF.EFFECT_MISMATCH", format!("read extension의 effect는 none이어야 함, `{}`", x.effect), x.span);
        }
        if x.deadline_ms.is_none() {
            self.d("DEF.MISSING_ITEM", "extension에 deadline 필요", x.span);
        }
        if x.implementation.is_empty() {
            self.d("DEF.MISSING_ITEM", "extension에 implementation 필요", x.span);
        }
        let (input, ij) = self.params(&x.input);
        let (_, oj) = self.params(&x.output);
        let mut binding = Map::new();
        for (rn, an) in &x.access {
            let Some(agg) = self.res.get(rn.as_str()).and_then(|r| r.aggregates.iter().find(|a| &a.name == an)) else {
                self.d("DEF.UNKNOWN_SYMBOL", format!("access 대상 `{rn}.{an}` 없음"), x.span);
                continue;
            };
            let mut b = Map::new();
            for (pn, pt) in &agg.input {
                let want = self.plain_type(pt);
                match input.iter().find(|(n, _)| n == pn) {
                    Some((_, t)) if Some(t) == want.as_ref() => {
                        b.insert(pn.clone(), json!(format!("input.{pn}")));
                    }
                    _ => self.d("DEF.EXT_INPUT_UNBOUND", format!("`{rn}.{an}`의 입력 `{pn}`을 extension 입력에서 같은 타입으로 받지 않음"), x.span),
                }
            }
            binding.insert(format!("{rn}.{an}"), Value::Object(b));
        }
        Some(json!({
            "kind": x.kind, "input": ij, "output": oj, "access": binding, "effect": x.effect,
            "deadlineMs": x.deadline_ms, "implementation": x.implementation,
        }))
    }
}

/// 호출자나 효과가 반드시 값을 줘야 하는 필드. id는 서버가 만든다.
fn required_fields(r: &Resource) -> Vec<String> {
    r.fields.iter().filter(|f| !f.ty.nullable && f.name != "id" && !f.ty.database_default).map(|f| f.name.clone()).collect()
}

fn valid_message_queue(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else { return false };
    value.len() <= 128 && (first.is_ascii_alphabetic() || first == b'_') && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// 부분 인덱스 조건으로 쓸 수 있는 식인지: 자기 행의 직접 열과 상수 비교만.
fn index_safe(e: &Value) -> bool {
    if let Some(v) = e.get("and").or_else(|| e.get("or")).and_then(Value::as_array) {
        return v.iter().all(index_safe);
    }
    if let Some(x) = e.get("not") {
        return index_safe(x);
    }
    let atom = |x: &Value| -> bool {
        if let Some(p) = x.get("path") {
            return p["root"] == "this" && p["segs"].as_array().is_some_and(|s| s.len() == 1);
        }
        x.get("enum").is_some() || x.get("lit").is_some()
    };
    if e.get("cmp").is_some() {
        return atom(&e["l"]) && atom(&e["r"]);
    }
    if let Some(l) = e.get("in") {
        return atom(l) && e["items"].as_array().is_some_and(|v| v.iter().all(atom));
    }
    false
}

#[derive(Clone, Copy)]
struct CallSite<'a> {
    name: &'a str,
    depth: usize,
    span: Span,
}

struct ExprMetrics<'a> {
    nodes: usize,
    depth: usize,
    max_path_segments: usize,
    calls: Vec<CallSite<'a>>,
}

fn expression_metrics(root: &Expr) -> ExprMetrics<'_> {
    let mut nodes = 0usize;
    let mut depth = 0usize;
    let mut max_path_segments = 0usize;
    let mut calls = vec![];
    let mut stack = vec![(root, 0usize)];
    while let Some((expr, parent_depth)) = stack.pop() {
        nodes = nodes.saturating_add(1);
        let nested = matches!(expr, Expr::Or(_) | Expr::And(_) | Expr::Not(_) | Expr::Cmp(_, _, _) | Expr::In(_, _) | Expr::Exists(_, _, _));
        let this_depth = parent_depth + usize::from(nested);
        depth = depth.max(this_depth);
        match expr {
            Expr::Or(items) | Expr::And(items) => stack.extend(items.iter().map(|item| (item, this_depth))),
            Expr::Not(inner) => stack.push((inner, this_depth)),
            Expr::Cmp(_, left, right) => {
                stack.push((left, this_depth));
                stack.push((right, this_depth));
            }
            Expr::In(left, items) => {
                stack.push((left, this_depth));
                stack.extend(items.iter().map(|item| (item, this_depth)));
            }
            Expr::Call(name, args, span) => {
                calls.push(CallSite { name, depth: parent_depth, span: *span });
                stack.extend(args.iter().map(|arg| (arg, parent_depth)));
            }
            Expr::Exists(_, condition, _) => stack.push((condition, this_depth)),
            Expr::Path(parts, _) => {
                let segments = parts.len().saturating_sub(1);
                max_path_segments = max_path_segments.max(segments);
                nodes = nodes.saturating_add(segments);
            }
            _ => {}
        }
    }
    ExprMetrics { nodes, depth, max_path_segments, calls }
}

fn add_expr_root<'a>(roots: &mut Vec<(&'a Expr, Span)>, expr: Option<&'a Expr>, owner: Span) {
    if let Some(expr) = expr {
        roots.push((expr, owner));
    }
}

/// Predicate 인라인이 V2 lowering에서 감당할 수 있는 공통 깊이와 노드 예산을 넘지 않는지 검사한다.
/// 토폴로지 순회와 각 식의 계측은 명시적 스택을 사용하므로 정의 수에 대한 반복 reachability가 없다.
fn validate_policy_expansion(spec: &Spec) -> Vec<Diag> {
    let mut diags = vec![];
    let mut names = HashMap::<&str, usize>::with_capacity(spec.predicates.len());
    let mut predicates = vec![];
    for predicate in &spec.predicates {
        if names.contains_key(predicate.name.as_str()) {
            continue;
        }
        names.insert(predicate.name.as_str(), predicates.len());
        predicates.push(predicate);
    }

    let mut metrics = Vec::with_capacity(predicates.len());
    let mut edges = Vec::<Vec<(usize, usize, Span)>>::with_capacity(predicates.len());
    for predicate in &predicates {
        let metric = expression_metrics(&predicate.body);
        if metric.max_path_segments > MAX_POLICY_PATH_SEGMENTS {
            return vec![Diag::new(
                "DEF.POLICY_EXPANSION_LIMIT",
                format!("predicate `{}`의 경로가 필드 단계 {MAX_POLICY_PATH_SEGMENTS}개 예산을 넘음", predicate.name),
                predicate.span,
            )];
        }
        let mut outgoing = vec![];
        for call in &metric.calls {
            if let Some(&target) = names.get(call.name) {
                outgoing.push((target, call.depth, call.span));
            }
        }
        metrics.push(metric);
        edges.push(outgoing);
    }

    // Iterative three-color DFS emits callee-before-caller order and finds cycles in O(V + E).
    let mut color = vec![0u8; predicates.len()];
    let mut order = Vec::with_capacity(predicates.len());
    for start in 0..predicates.len() {
        if color[start] != 0 {
            continue;
        }
        color[start] = 1;
        let mut stack = vec![(start, 0usize)];
        while let Some((node, next_edge)) = stack.last_mut() {
            if *next_edge == edges[*node].len() {
                color[*node] = 2;
                order.push(*node);
                stack.pop();
                continue;
            }
            let (target, _, edge_span) = edges[*node][*next_edge];
            *next_edge += 1;
            match color[target] {
                0 => {
                    color[target] = 1;
                    stack.push((target, 0));
                }
                1 => {
                    diags.push(Diag::new(
                        "DEF.POLICY_CYCLE",
                        format!("predicate `{}`가 호출 경로를 따라 자기 자신을 다시 부름", predicates[target].name),
                        edge_span,
                    ));
                    return diags;
                }
                _ => {}
            }
        }
    }

    let node_ceiling = MAX_POLICY_EXPANDED_NODES + 1;
    let mut expanded_nodes = vec![0usize; predicates.len()];
    let mut expanded_depth = vec![0usize; predicates.len()];
    let mut total_nodes = 0usize;
    for index in order {
        let metric = &metrics[index];
        let mut node_count = metric.nodes.min(node_ceiling);
        let mut expression_depth = metric.depth;
        for &(target, call_depth, _) in &edges[index] {
            node_count = node_count.saturating_add(expanded_nodes[target]).min(node_ceiling);
            expression_depth = expression_depth.max(call_depth.saturating_add(expanded_depth[target]));
        }
        if node_count > MAX_POLICY_EXPANDED_NODES || expression_depth > MAX_POLICY_EXPANDED_DEPTH {
            return vec![Diag::new(
                "DEF.POLICY_EXPANSION_LIMIT",
                format!("predicate `{}`의 인라인 확장이 깊이 {expression_depth}, 노드 {node_count}로 예산을 넘음", predicates[index].name),
                predicates[index].span,
            )];
        }
        expanded_nodes[index] = node_count;
        expanded_depth[index] = expression_depth;
        total_nodes = total_nodes.saturating_add(node_count).min(node_ceiling);
        if total_nodes > MAX_POLICY_EXPANDED_NODES {
            return vec![Diag::new(
                "DEF.POLICY_EXPANSION_LIMIT",
                format!("predicate 정의 전체의 인라인 노드 수가 {MAX_POLICY_EXPANDED_NODES} 예산을 넘음"),
                predicates[index].span,
            )];
        }
    }

    let mut roots = vec![];
    for access in &spec.accesses {
        if let AccessBody::Guard(expr) = &access.body {
            roots.push((expr, access.span));
        }
    }
    for limit in &spec.limits {
        roots.push((&limit.cond, limit.span));
    }
    for resource in &spec.resources {
        add_expr_root(&mut roots, resource.row_read.as_ref(), resource.span);
        for (_, expr, span) in &resource.field_read {
            roots.push((expr, *span));
        }
        for (_, expr, span) in &resource.checks {
            roots.push((expr, *span));
        }
        if let Some(create) = &resource.expose_create {
            add_expr_root(&mut roots, create.allow.as_ref(), create.span);
        }
        if let Some(compose) = &resource.expose_compose {
            add_expr_root(&mut roots, compose.same_scope.as_ref(), compose.span);
            for (_, values, span) in &compose.creates {
                roots.extend(values.iter().map(|expr| (expr, *span)));
            }
        }
        for apply in &resource.expose_apply {
            add_expr_root(&mut roots, apply.same_scope.as_ref(), apply.span);
        }
        for aggregate in &resource.aggregates {
            add_expr_root(&mut roots, aggregate.where_.as_ref(), aggregate.span);
            if let Some(AccessRef::Call(_, args)) = &aggregate.source_access {
                roots.extend(args.iter().map(|expr| (expr, aggregate.span)));
            }
        }
        for transition in &resource.transitions {
            roots.push((&transition.from, transition.span));
            roots.push((&transition.allow, transition.span));
            roots.extend(transition.to.iter().map(|(_, expr)| (expr, transition.span)));
            for effect in &transition.effects {
                match effect {
                    Effect::Create { values, span, .. } => roots.extend(values.iter().map(|(_, expr)| (expr, *span))),
                    Effect::Update { matches, values, span, .. } => {
                        roots.extend(matches.iter().chain(values).map(|(_, expr)| (expr, *span)));
                    }
                    Effect::Delete { matches, span, .. } => {
                        roots.extend(matches.iter().map(|(_, expr)| (expr, *span)));
                    }
                    Effect::Notify { to, span, .. } => roots.push((to, *span)),
                }
            }
        }
    }

    for operation in &spec.operations {
        add_expr_root(&mut roots, operation.allow.as_ref(), operation.span);
    }
    for (expr, owner_span) in roots {
        let metric = expression_metrics(expr);
        if metric.max_path_segments > MAX_POLICY_PATH_SEGMENTS {
            return vec![Diag::new(
                "DEF.POLICY_EXPANSION_LIMIT",
                format!("정책 경로가 필드 단계 {MAX_POLICY_PATH_SEGMENTS}개 예산을 넘음"),
                owner_span,
            )];
        }
        let mut node_count = metric.nodes.min(node_ceiling);
        let mut expression_depth = metric.depth;
        for call in metric.calls {
            if let Some(&target) = names.get(call.name) {
                node_count = node_count.saturating_add(expanded_nodes[target]).min(node_ceiling);
                expression_depth = expression_depth.max(call.depth.saturating_add(expanded_depth[target]));
            }
        }
        if node_count > MAX_POLICY_EXPANDED_NODES || expression_depth > MAX_POLICY_EXPANDED_DEPTH {
            return vec![Diag::new(
                "DEF.POLICY_EXPANSION_LIMIT",
                format!("정책 식의 인라인 확장이 깊이 {expression_depth}, 노드 {node_count}로 예산을 넘음"),
                owner_span,
            )];
        }
        total_nodes = total_nodes.saturating_add(node_count).min(node_ceiling);
        if total_nodes > MAX_POLICY_EXPANDED_NODES {
            return vec![Diag::new(
                "DEF.POLICY_EXPANSION_LIMIT",
                format!("정책 전체의 인라인 노드 수가 {MAX_POLICY_EXPANDED_NODES} 예산을 넘음"),
                owner_span,
            )];
        }
    }
    diags
}

fn span_of(e: &Expr) -> Span {
    match e {
        Expr::Path(_, s) | Expr::Call(_, _, s) | Expr::Exists(_, _, s) | Expr::Arith(_, _, _, s) => *s,
        Expr::Or(v) | Expr::And(v) => v.first().map(span_of).unwrap_or_default(),
        Expr::Not(x) => span_of(x),
        Expr::Cmp(_, l, _) | Expr::In(l, _) => span_of(l),
        _ => Span::default(),
    }
}
