use crate::ast::*;
use crate::diag::{Diag, Span};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Id(String),
    Text,
    Url,
    Time,
    Int,
    Bool,
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
            Ty::Url => "Url".into(),
            Ty::Time => "Time".into(),
            Ty::Int => "Int".into(),
            Ty::Bool => "Bool".into(),
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
}

#[derive(Clone, Default)]
struct Scope {
    this: Option<String>,
    in_exists: bool,
    vars: Vec<(String, TT)>,
    input: Vec<(String, TT)>,
}

const UNRESOLVED: &str = "UNRESOLVED_NAME";
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
    };
    cx.collect();
    let execution = cx.build();
    if !cx.diags.is_empty() {
        return Err(cx.diags);
    }
    let (metadata, spans) = metadata_of(spec);
    Ok(Output { execution, metadata, spans })
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

    fn collect(&mut self) {
        let mut names: BTreeSet<String> = BTreeSet::new();
        let spec = self.spec;
        let mut seen = |n: &str, sp: Span, diags: &mut Vec<Diag>| {
            if !names.insert(n.to_string()) {
                diags.push(Diag::new("DUPLICATE", format!("이름 `{n}` 중복 선언"), sp));
            }
        };
        for e in &spec.enums {
            seen(&e.name, e.span, &mut self.diags);
            self.enums.insert(&e.name, e);
        }
        for r in &spec.resources {
            seen(&r.name, r.span, &mut self.diags);
            self.res.insert(&r.name, r);
        }
        for p in &spec.predicates {
            seen(&p.name, p.span, &mut self.diags);
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
            Some((a, sp)) if !self.res.contains_key(a.as_str()) => self.d("UNKNOWN_SYMBOL", format!("actor 대상 resource `{a}` 없음"), *sp),
            Some((a, _)) => self.actor = Some(a.clone()),
            None => self.d("MISSING_ITEM", "actor 선언 필요", Span::default()),
        }
    }

    fn type_of(&mut self, owner: Option<&str>, t: &TypeRef) -> Option<TT> {
        let base = if t.id_of {
            if self.res.contains_key(t.name.as_str()) {
                Ty::Id(t.name.clone())
            } else {
                self.d("UNKNOWN_TYPE", format!("`{}.Id`의 resource 없음", t.name), t.span);
                return None;
            }
        } else {
            match t.name.as_str() {
                "Id" => match owner {
                    Some(o) => Ty::Id(o.to_string()),
                    None => {
                        self.d("UNKNOWN_TYPE", "resource 밖에서 맨 `Id`는 대상이 없음. `X.Id`를 쓴다", t.span);
                        return None;
                    }
                },
                "Text" => Ty::Text,
                "Url" => Ty::Url,
                "Time" => Ty::Time,
                "Int" => Ty::Int,
                "Bool" => Ty::Bool,
                n if self.enums.contains_key(n) => Ty::Enum(n.to_string()),
                n if self.res.contains_key(n) => Ty::Ref(n.to_string()),
                n => {
                    self.d("UNKNOWN_TYPE", format!("알 수 없는 타입 `{n}`"), t.span);
                    return None;
                }
            }
        };
        if t.range.is_some() && !matches!(base, Ty::Text | Ty::Int) {
            self.d("TYPE_MISMATCH", "범위 제약은 Text/Int에만", t.span);
        }
        if let Some((lo, hi)) = t.range {
            if lo > hi || (base == Ty::Text && lo < 0) {
                self.d("BAD_RANGE", format!("만족할 수 없는 범위 {lo}..{hi}"), t.span);
            }
        }
        Some(TT { ty: base, nullable: t.nullable })
    }

    fn field_tt(&self, r: &str, f: &str) -> Option<TT> {
        let res = self.res.get(r)?;
        let fd = res.fields.iter().find(|x| x.name == f)?;
        // 여기서는 진단을 내지 않는다. 필드 타입 오류는 resource 검사에서 한 번만 낸다.
        let t = &fd.ty;
        let base = if t.id_of {
            Ty::Id(t.name.clone())
        } else {
            match t.name.as_str() {
                "Id" => Ty::Id(r.to_string()),
                "Text" => Ty::Text,
                "Url" => Ty::Url,
                "Time" => Ty::Time,
                "Int" => Ty::Int,
                "Bool" => Ty::Bool,
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
                self.d("DUPLICATE", format!("매개변수·입출력 이름 `{n}` 중복"), t.span);
            }
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

    fn path(&self, segs: &[String], sp: Span, sc: &Scope) -> Result<(Value, TT), Diag> {
        let first = segs[0].as_str();
        let this_field = sc.this.as_ref().and_then(|t| self.field_tt(t, first));
        let var = sc.vars.iter().find(|(n, _)| n == first).map(|x| x.1.clone());
        let (root, mut tt, rest) = match first {
            "this" if sc.in_exists => {
                return Err(Diag::new(
                    "THIS_IN_EXISTS",
                    "exists 조건 안의 `this`는 안쪽 행으로 다시 묶여 `team = this.team`이 항상 참이 된다. 안쪽 필드는 맨 이름으로 쓰고, 바깥 행 값은 predicate 인자로 넘겨라",
                    sp,
                ))
            }
            "this" => match &sc.this {
                Some(t) => ("this".to_string(), TT::new(Ty::Ref(t.clone())), &segs[1..]),
                None => return Err(Diag::new(UNRESOLVED, "이 위치에는 `this`가 없음", sp)),
            },
            "input" => {
                let n = segs.get(1).ok_or_else(|| Diag::new("PARSE_EXPECTED", "`input.<이름>` 필요", sp))?;
                match sc.input.iter().find(|(x, _)| x == n) {
                    Some((_, t)) => (format!("input.{n}"), t.clone(), &segs[2..]),
                    None => return Err(Diag::new("UNKNOWN_FIELD", format!("input에 `{n}` 없음"), sp)),
                }
            }
            _ => match (this_field, var) {
                (Some(_), Some(_)) => return Err(Diag::new("AMBIGUOUS_NAME", format!("`{first}`가 필드와 변수 둘 다로 해석됨"), sp)),
                (Some(t), None) => ("this".to_string(), t, &segs[1..]),
                (None, Some(t)) => (if first == "actor" { "actor".to_string() } else { format!("var.{first}") }, t, &segs[1..]),
                (None, None) => return Err(Diag::new(UNRESOLVED, format!("`{}`를 해석할 수 없음", segs.join(".")), sp)),
            },
        };
        let mut walked: Vec<String> = if root == "this" && first != "this" { vec![first.to_string()] } else { vec![] };
        for s in rest {
            let target = match &tt.ty {
                Ty::Ref(r) => r.clone(),
                _ => return Err(Diag::new("UNKNOWN_FIELD", format!("`{}` 타입에는 필드 `{s}`가 없음", tt.show()), sp)),
            };
            match self.field_tt(&target, s) {
                Some(f) => {
                    let nullable = tt.nullable || f.nullable;
                    tt = TT { ty: f.ty, nullable };
                }
                None => {
                    return Err(Diag::new("UNKNOWN_FIELD", format!("`{target}`에 필드 `{s}` 없음"), sp));
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
        if let (Expr::Path(segs, sp), Some(TT { ty: Ty::Enum(en), .. })) = (e, want) {
            if segs.len() == 1 {
                match self.path(segs, *sp, sc) {
                    Err(d) if d.code == UNRESOLVED => {
                        let decl = self.enums[en.as_str()];
                        if decl.variants.contains(&segs[0]) {
                            return Ok((json!({ "enum": format!("{en}.{}", segs[0]) }), TT::new(Ty::Enum(en.clone()))));
                        }
                        return Err(Diag::new("UNKNOWN_SYMBOL", format!("`{en}`에 값 `{}` 없음", segs[0]), *sp));
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
            return Err(Diag::new("TYPE_MISMATCH", format!("증감 대입은 null이 아닌 Int 필드만, `{f}`는 `{}`", ft.show()), sp));
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
            return Err(Diag::new("ARITH_NOT_ALLOWED", format!("증감 대입은 `{f} = {f} + 정수` 또는 `{f} = {f} - 정수`(0 제외)만"), sp));
        }
        let delta = if op == "-" { n.checked_neg().ok_or_else(|| Diag::new("BAD_RANGE", "증감 값 범위", sp))? } else { n };
        Ok(json!({ "increment": delta }))
    }

    fn bool_of(&self, e: &Expr, sc: &Scope) -> Result<Value, Diag> {
        let (v, t) = self.ex(e, sc)?;
        if t.ty != Ty::Bool {
            return Err(Diag::new("TYPE_MISMATCH", format!("조건식은 Bool이어야 함, `{}`", t.show()), span_of(e)));
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
            Expr::Arith(_, _, _, sp) => Err(Diag::new("ARITH_NOT_ALLOWED", "산술식은 transition `to`의 `필드 = 필드 ± 정수`에서만 쓸 수 있음", *sp)),
            Expr::Cmp(op, l, r) => {
                let (lv, lt, rv, rt) = match self.ex(l, sc) {
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
                };
                let sp = span_of(e);
                if lt.ty == Ty::Null || rt.ty == Ty::Null {
                    let other = if lt.ty == Ty::Null { &rt } else { &lt };
                    if !matches!(*op, "=" | "!=") {
                        return Err(Diag::new("TYPE_MISMATCH", "null은 = 또는 != 로만 비교", sp));
                    }
                    // 익명 요청에서 actor는 실제로 NULL이다. `actor != null`이 "로그인한 사용자"를 표현하는 유일한 방법이다.
                    let bare_actor = [l, r].iter().any(|x| matches!(x.as_ref(), Expr::Path(p, _) if p.len() == 1 && p[0] == "actor"));
                    if !other.nullable && !bare_actor {
                        return Err(Diag::new("NULL_COMPARE_NON_NULLABLE", format!("null이 될 수 없는 `{}`를 null과 비교", other.show()), sp));
                    }
                } else if !same_target(&lt.ty, &rt.ty) {
                    return Err(Diag::new("TYPE_MISMATCH", format!("`{}` {op} `{}`", lt.show(), rt.show()), sp));
                } else if !matches!(*op, "=" | "!=") && !matches!(lt.ty, Ty::Time | Ty::Int | Ty::Text) {
                    return Err(Diag::new("TYPE_MISMATCH", format!("`{}`는 크기 비교 불가", lt.show()), sp));
                }
                Ok((json!({ "cmp": op, "l": lv, "r": rv }), b))
            }
            Expr::In(l, items) => {
                let (lv, lt) = self.ex(l, sc)?;
                let mut iv = vec![];
                for it in items {
                    let (v, t) = self.expect(it, sc, Some(&lt))?;
                    if !same_target(&t.ty, &lt.ty) {
                        return Err(Diag::new("TYPE_MISMATCH", format!("in 목록 `{}` vs `{}`", lt.show(), t.show()), span_of(it)));
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
                    None => return Err(Diag::new("UNKNOWN_SYMBOL", format!("predicate `{n}` 없음"), *sp)),
                };
                self.args_against(n, &p.params, args, sc, *sp).map(|av| (json!({ "call": n, "args": av }), b))
            }
            Expr::Exists(r, cond, sp) => {
                if !self.res.contains_key(r.as_str()) {
                    return Err(Diag::new("UNKNOWN_SYMBOL", format!("resource `{r}` 없음"), *sp));
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
            return Err(Diag::new("ARITY_MISMATCH", format!("`{n}` 인자 {}개 필요, {}개", params.len(), args.len()), sp));
        }
        let mut av = vec![];
        for ((pn, pt), a) in params.iter().zip(args) {
            let want = self.plain_type(pt).ok_or_else(|| Diag::new("UNKNOWN_TYPE", format!("`{n}.{pn}` 타입"), sp))?;
            let (v, t) = self.expect(a, sc, Some(&want))?;
            if !same_target(&t.ty, &want.ty) || (t.nullable && !want.nullable) {
                return Err(Diag::new("TYPE_MISMATCH", format!("`{n}`의 `{pn}`는 `{}`, 받은 값 `{}`", want.show(), t.show()), span_of(a)));
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
                "Url" => Ty::Url,
                "Time" => Ty::Time,
                "Int" => Ty::Int,
                "Bool" => Ty::Bool,
                n if self.enums.contains_key(n) => Ty::Enum(n.into()),
                n if self.res.contains_key(n) => Ty::Ref(n.into()),
                _ => return None,
            }
        };
        Some(TT { ty, nullable: t.nullable })
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
        out.insert("enums".into(), Value::Object(spec.enums.iter().map(|e| (e.name.clone(), json!(e.variants))).collect()));

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
                        self.d("TYPE_MISMATCH", "totalOfVisible access는 매개변수를 받지 않음", a.span);
                    }
                    if !self.res.contains_key(r.as_str()) {
                        self.d("UNKNOWN_SYMBOL", format!("resource `{r}` 없음"), *sp);
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
                self.d("UNKNOWN_SYMBOL", format!("limit 대상 resource `{}` 없음", l.on), l.span);
                continue;
            }
            if l.at_most < 1 {
                self.d("TYPE_MISMATCH", "atMost는 1 이상", l.span);
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
        Value::Object(out)
    }

    fn resource(&mut self, r: &Resource, limits: &Map<String, Value>) -> Value {
        let mut o = Map::new();
        // 같은 resource 안 이름 충돌은 마지막 선언이 앞을 덮어쓰지 않게 거부한다(F09).
        let mut names: BTreeSet<String> = r.fields.iter().map(|f| f.name.clone()).collect();
        for (n, sp) in r.aggregates.iter().map(|a| (&a.name, a.span)) {
            if !names.insert(n.clone()) {
                self.d("DUPLICATE", format!("`{n}`이 필드/다른 집계와 이름 충돌"), sp);
            }
        }
        let mut seen = BTreeSet::new();
        for (n, sp) in r.transitions.iter().map(|t| (&t.name, t.span)).chain(r.extensions.iter().map(|x| (&x.name, x.span))) {
            if !seen.insert(n.clone()) {
                self.d("DUPLICATE", format!("동작 이름 `{n}` 중복"), sp);
            }
        }
        let mut seen = BTreeSet::new();
        for (f, _, sp) in &r.field_read {
            if !seen.insert(f.clone()) {
                self.d("DUPLICATE", format!("필드 `{f}`의 read 정책 중복"), *sp);
            }
        }
        let mut seen = BTreeSet::new();
        for (n, _, sp) in &r.invariants {
            if !seen.insert(n.clone()) {
                self.d("DUPLICATE", format!("invariant `{n}` 중복"), *sp);
            }
        }
        let mut seen = BTreeSet::new();
        for (n, sp) in &r.expose_aggregates {
            if !seen.insert(n.clone()) {
                self.d("DUPLICATE", format!("expose aggregate `{n}` 중복"), *sp);
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
                .chain(e.traverse.iter().map(|(n, _, sp)| (format!("traverse {n}"), *sp)));
            for (k, sp) in items {
                if !seen.insert(k.clone()) {
                    self.d("DUPLICATE", format!("expose read `{k}` 중복"), sp);
                }
            }
        }
        let mut fields = Map::new();
        let mut seen = BTreeSet::new();
        for f in &r.fields {
            if !seen.insert(&f.name) {
                self.d("DUPLICATE", format!("필드 `{}` 중복", f.name), f.span);
            }
            if let Some(tt) = self.type_of(Some(&r.name), &f.ty) {
                fields.insert(f.name.clone(), json!({ "ty": tt.show(), "range": f.ty.range.map(|(a, b)| json!([a, b])) }));
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
                self.d("UNKNOWN_FIELD", format!("field 정책 대상 `{f}` 없음"), *sp);
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
                self.d("UNKNOWN_SYMBOL", format!("노출할 aggregate `{n}` 없음"), *sp);
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
            let allow = self.bool_of(&t.allow, &sc);
            let allow = self.run(allow);
            let mut to = Map::new();
            let mut counters = false;
            for (f, val) in &t.to {
                if to.contains_key(f) {
                    self.d("DUPLICATE", format!("transition `{}`의 `{f}` 대입 중복", t.name), t.span);
                    continue;
                }
                let Some(ft) = self.field_tt(&r.name, f) else {
                    self.d("UNKNOWN_FIELD", format!("transition 대상 필드 `{f}` 없음"), t.span);
                    continue;
                };
                if let Expr::Arith(op, l, r, sp) = val {
                    if let Some(v) = self.run(self.counter(f, &ft, op, l, r, *sp)) {
                        counters = true;
                        to.insert(f.clone(), v);
                    }
                    continue;
                }
                let res = self.expect(val, &sc, Some(&ft)).and_then(|(v, vt)| {
                    if (same_target(&vt.ty, &ft.ty) && (!vt.nullable || ft.nullable)) || (vt.ty == Ty::Null && ft.nullable) {
                        Ok(v)
                    } else {
                        Err(Diag::new("TYPE_MISMATCH", format!("`{f}`에 `{}` 대입 불가", vt.show()), t.span))
                    }
                });
                if let Some(v) = self.run(res) {
                    to.insert(f.clone(), v);
                }
            }
            if counters && t.repeat.as_ref().is_some_and(|(r, _)| r == "unchanged") {
                // 증감은 매번 값이 달라져 "이미 목표 상태"가 없다. unchanged는 재시도를 조용히 성공시키는 것으로 오해된다.
                self.d("UNSUPPORTED", format!("transition `{}`: 증감 대입에는 repeat unchanged를 쓸 수 없음", t.name), t.span);
            }
            let repeat = match &t.repeat {
                None => "reject".to_string(),
                Some((r, _)) if r == "unchanged" || r == "reject" => r.clone(),
                Some((r, sp)) => {
                    self.d("UNSUPPORTED", format!("repeat `{r}`는 unchanged/reject만"), *sp);
                    r.clone()
                }
            };
            let mut effects = vec![];
            for e in &t.effects {
                if let Some(v) = self.effect(r, e, &sc) {
                    effects.push(v);
                }
            }
            tr.insert(t.name.clone(), json!({ "from": from, "to": to, "allow": allow, "repeat": repeat, "effects": effects }));
        }
        o.insert("transitions".into(), Value::Object(tr));

        let mut ap = Map::new();
        for ea in &r.expose_apply {
            if !r.transitions.iter().any(|t| t.name == ea.transition) {
                self.d("UNKNOWN_SYMBOL", format!("공개할 전이 `{}` 없음", ea.transition), ea.span);
                continue;
            }
            if ap.contains_key(&ea.transition) {
                self.d("DUPLICATE", format!("expose apply `{}` 중복", ea.transition), ea.span);
                continue;
            }
            let mut targets = BTreeSet::new();
            for (t, sp) in &ea.targets {
                if t != "id" && t != "where" {
                    self.d("UNSUPPORTED", format!("target `{t}`는 id/where만"), *sp);
                }
                if !targets.insert(t.clone()) {
                    self.d("DUPLICATE", format!("target `{t}` 중복"), *sp);
                }
            }
            if targets.is_empty() {
                self.d("MISSING_ITEM", "expose apply에 target 필요", ea.span);
            }
            // where 대상은 읽기 filter 허용 목록 안에서만 쓴다. 쓰기 조건으로 숨은 값을 추론하지 못하게.
            if targets.contains("where") && r.expose_read.as_ref().is_none_or(|e| e.filter.is_empty()) {
                self.d("MISSING_ITEM", "where 대상에는 expose read filter 허용 목록이 필요", ea.span);
            }
            match ea.bulk {
                Some(n) if n >= 1 => {}
                Some(_) => self.d("BAD_BUDGET", "bulk maxRows는 1 이상", ea.span),
                None => self.d("MISSING_ITEM", "expose apply에 bulk maxRows 필요(무제한 기본값 없음)", ea.span),
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
                        self.d("MISSING_ITEM", "expose create에 allow 필요", c.span);
                        Value::Null
                    }
                };
                let mut fs = BTreeSet::new();
                for (f, sp) in &c.fields {
                    if !r.fields.iter().any(|x| &x.name == f) {
                        self.d("UNKNOWN_FIELD", format!("생성 필드 `{f}` 없음"), *sp);
                    }
                    fs.insert(f.clone());
                }
                for f in required_fields(r) {
                    if !fs.contains(&f) {
                        self.d("MISSING_ITEM", format!("필수 필드 `{f}`가 expose create fields에 없음"), c.span);
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
                        self.d("UNKNOWN_SYMBOL", format!("compose 전이 `{t}` 없음"), *sp);
                    }
                    trs.insert(t.clone());
                }
                let mut creates = Map::new();
                for (res, srcs, sp) in &c.creates {
                    match self.res.get(res.as_str()) {
                        Some(t) if t.expose_create.is_some() => {}
                        Some(_) => self.d("MISSING_ITEM", format!("compose create `{res}`에는 그 resource의 expose create(allow)가 필요"), *sp),
                        None => self.d("UNKNOWN_SYMBOL", format!("resource `{res}` 없음"), *sp),
                    }
                    let mut sv = vec![];
                    for e in srcs {
                        // 출처는 대상 행의 경로만 받는다. 상수·호출은 실행기가 해석하지 않는다(R3-06).
                        if !matches!(e, Expr::Path(..)) {
                            self.d("UNSUPPORTED", format!("compose create `{res}`의 출처는 대상 행 경로여야 함"), *sp);
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
                    self.d("MISSING_ITEM", "expose compose에 bulk maxRows(1 이상) 필요", c.span);
                }
                let self_row = match &c.self_row {
                    None => Value::Null,
                    Some((a, by, sp)) => {
                        let actor_ok = matches!((self.field_tt(&r.name, a), &self.actor), (Some(TT { ty: Ty::Ref(x), .. }), Some(ac)) if &x == ac);
                        let by_ok = matches!(self.field_tt(&r.name, by), Some(TT { ty: Ty::Ref(_), .. }));
                        if !actor_ok || !by_ok {
                            self.d("TYPE_MISMATCH", "selfRow는 `<actor 참조 필드> by <참조 필드>`", *sp);
                        }
                        // 자기 행은 대상과 같은 범위에서 찾아야 하므로 sameScope는 `by` 필드 그 자체여야 한다(r6 F01).
                        match &c.same_scope {
                            Some(Expr::Path(p, _)) if p.len() == 1 && &p[0] == by => {}
                            _ => self.d("TYPE_MISMATCH", format!("selfRow `by {by}`와 sameScope가 같은 필드여야 함"), *sp),
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
                    self.d("UNKNOWN_FIELD", format!("unique 필드 `{c}` 없음"), *csp);
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
                self.d("DUPLICATE", format!("check `{n}` 중복"), *sp);
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
                self.d("UNKNOWN_SYMBOL", format!("invariant `{n}` 정의 없음. 실행식 없는 불변식은 허용하지 않음"), *sp);
                continue;
            };
            if l["on"] != json!(r.name) {
                self.d("TYPE_MISMATCH", format!("`{n}`은 `{}` 대상 정의", l["on"]), *sp);
                continue;
            }
            match self.field_tt(&r.name, per) {
                Some(TT { ty: Ty::Ref(_), .. }) => {}
                _ => {
                    self.d("UNKNOWN_FIELD", format!("`per {per}`는 이 resource의 참조 필드여야 함"), *sp);
                    continue;
                }
            }
            let deferred = r.deferred_invariants.contains(n);
            // lockedCountCheck는 V3(spike-v3-write)도 집행하지 않고 DDL 생성(sqlgen)도 거부한다.
            // check 통과 후 init 실패가 없도록 여기서 같은 판정을 낸다.
            if !(l["atMost"] == json!(1) && index_safe(&l["where"])) {
                self.d(
                    "UNSUPPORTED_INVARIANT",
                    format!("`{n}`: atMost 1이고 조건이 이 행의 필드와 리터럴만 쓰는 형태만 집행할 수 있음(시간·actor·하위조회 의존이나 atMost 2 이상은 미지원). atMost 1 형태로 바꿔라"),
                    *sp,
                );
                continue;
            }
            let enforcement = json!({ "kind": "partialUniqueIndex", "columns": [per], "where": l["where"], "deferred": deferred });
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
                    self.d("UNKNOWN_SYMBOL", format!("생성 대상 `{resource}` 없음"), *span);
                    return None;
                };
                let mut vals = Map::new();
                for (f, v) in values {
                    if vals.contains_key(f) {
                        self.d("DUPLICATE", format!("`{resource}.{f}` 값 중복"), *span);
                        continue;
                    }
                    let Some(ft) = self.field_tt(resource, f) else {
                        self.d("UNKNOWN_FIELD", format!("`{resource}`에 필드 `{f}` 없음"), *span);
                        continue;
                    };
                    let res = self.expect(v, sc, Some(&ft)).and_then(|(val, vt)| {
                        if same_target(&vt.ty, &ft.ty) && (!vt.nullable || ft.nullable) {
                            Ok(val)
                        } else {
                            Err(Diag::new("TYPE_MISMATCH", format!("`{resource}.{f}`({})에 `{}` 불가", ft.show(), vt.show()), *span))
                        }
                    });
                    if let Some(val) = self.run(res) {
                        vals.insert(f.clone(), val);
                    }
                }
                for f in required_fields(target) {
                    if !vals.contains_key(&f) {
                        self.d("MISSING_ITEM", format!("`{resource}` 생성에 필수 필드 `{f}` 값 필요"), *span);
                    }
                }
                let _ = r;
                Some(json!({ "create": resource, "values": vals }))
            }
            Effect::Update { resource, matches, values, span } => {
                if !self.res.contains_key(resource.as_str()) {
                    self.d("UNKNOWN_SYMBOL", format!("갱신 대상 `{resource}` 없음"), *span);
                    return None;
                }
                let typed = |pairs: &Vec<(String, Expr)>, what: &str, cx: &mut Self| -> Map<String, Value> {
                    let mut out = Map::new();
                    for (f, v) in pairs {
                        if out.contains_key(f) {
                            cx.d("DUPLICATE", format!("`{resource}.{f}` {what} 중복"), *span);
                            continue;
                        }
                        let Some(ft) = cx.field_tt(resource, f) else {
                            cx.d("UNKNOWN_FIELD", format!("`{resource}`에 필드 `{f}` 없음"), *span);
                            continue;
                        };
                        let res = cx.expect(v, sc, Some(&ft)).and_then(|(val, vt)| {
                            if same_target(&vt.ty, &ft.ty) && (!vt.nullable || ft.nullable) {
                                Ok(val)
                            } else {
                                Err(Diag::new("TYPE_MISMATCH", format!("`{resource}.{f}`({})에 `{}` 불가", ft.show(), vt.show()), *span))
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
                    self.d("MISSING_ITEM", "update 효과에는 찾을 조건과 바꿀 값이 필요", *span);
                }
                Some(json!({ "update": resource, "match": m, "values": v }))
            }
            Effect::Notify { to, topic, span } => {
                let res = self.ex(to, sc).and_then(|(v, t)| match (&t.ty, &self.actor) {
                    (Ty::Ref(x), Some(a)) if x == a && !t.nullable => Ok(v),
                    _ => Err(Diag::new("TYPE_MISMATCH", format!("notify 대상은 actor 타입 참조여야 함, `{}`", t.show()), *span)),
                });
                let v = self.run(res)?;
                Some(json!({ "notify": { "to": v, "topic": topic } }))
            }
        }
    }

    fn aggregate(&mut self, r: &Resource, a: &Aggregate) -> Option<Value> {
        let tt = self.type_of(Some(&r.name), &a.ty)?;
        for (k, v, allowed) in [("callerFilter", &a.caller_filter, "none"), ("rowOutput", &a.row_output, "none"), ("release", &a.release, "count")] {
            match v.as_deref() {
                None => self.d("MISSING_ITEM", format!("aggregate `{}`에 `{k}` 필요", a.name), a.span),
                Some(x) if x != allowed => self.d("UNSUPPORTED", format!("`{k} {x}`는 spike에서 지원하지 않음(허용: {allowed})"), a.span),
                _ => {}
            }
        }
        if tt.ty != Ty::Int {
            self.d("TYPE_MISMATCH", "count 집계 타입은 Int", a.span);
        }
        let source = a.source.clone().unwrap_or_else(|| r.name.clone());
        if !self.res.contains_key(source.as_str()) {
            self.d("UNKNOWN_SYMBOL", format!("집계 source `{source}` 없음"), a.span);
            return None;
        }
        if let Some(g) = &a.group_key {
            match self.field_tt(&source, g) {
                Some(TT { ty: Ty::Ref(t), .. }) if t == r.name => {}
                _ => self.d("GROUP_KEY_MISMATCH", format!("groupKey `{g}`는 `{source}`에서 `{}`를 가리키는 필드여야 함", r.name), a.span),
            }
        }
        let (input, ij) = self.params(&a.input);
        let access = match &a.source_access {
            None => {
                self.d("MISSING_ITEM", format!("aggregate `{}`에 sourceAccess 필요. 원본 행 정책을 자동으로 물려받지 않음", a.name), a.span);
                return None;
            }
            Some(AccessRef::Name(n)) => match self.accesses.get(n.as_str()).map(|x| &x.body) {
                Some(AccessBody::TotalOfVisible(p, _)) => {
                    if p != &r.name || a.group_key.is_none() {
                        self.d("ACCESS_MISMATCH", format!("`{n}`은 `{p}` 기준 groupKey 집계에만 사용"), a.span);
                    }
                    json!({ "ref": n })
                }
                Some(AccessBody::Guard(_)) => {
                    self.d("ARITY_MISMATCH", format!("guard access `{n}`에는 인자가 필요"), a.span);
                    return None;
                }
                None => {
                    self.d("UNKNOWN_SYMBOL", format!("access `{n}` 없음"), a.span);
                    return None;
                }
            },
            Some(AccessRef::Call(n, args)) => {
                let acc = match self.accesses.get(n.as_str()) {
                    Some(x) if matches!(x.body, AccessBody::Guard(_)) => *x,
                    Some(_) => {
                        self.d("ARITY_MISMATCH", format!("`{n}`은 인자를 받지 않음"), a.span);
                        return None;
                    }
                    None => {
                        self.d("UNKNOWN_SYMBOL", format!("access `{n}` 없음"), a.span);
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
        Some(json!({
            "ty": tt.show(), "tyRange": a.ty.range.map(|(x, y)| json!([x, y])), "input": ij, "source": source, "sourceAccess": access, "groupKey": a.group_key,
            "where": where_, "callerFilter": a.caller_filter, "rowOutput": a.row_output, "release": a.release,
        }))
    }

    fn expose(&mut self, r: &Resource, e: &ExposeRead) -> Value {
        let mut kinds = Map::new();
        for (s, sp) in &e.select {
            if r.fields.iter().any(|f| &f.name == s) {
                let k = if r.field_read.iter().any(|x| &x.0 == s) { "fieldWithPolicy" } else { "field" };
                kinds.insert(s.clone(), json!(k));
            } else if let Some(a) = r.aggregates.iter().find(|a| &a.name == s) {
                if a.group_key.is_none() || !a.input.is_empty() {
                    self.d("AGG_NOT_SELECTABLE", format!("`{s}`는 행별 집계가 아니어서 select 불가"), *sp);
                }
                kinds.insert(s.clone(), json!("aggregate"));
            } else {
                self.d("UNKNOWN_FIELD", format!("select `{s}`: `{}`에 필드/집계 없음", r.name), *sp);
            }
        }
        let mut filters = BTreeSet::new();
        for (f, op, sp) in &e.filter {
            let Some(tt) = self.field_tt(&r.name, f) else {
                self.d("UNKNOWN_FIELD", format!("filter 대상 `{f}` 없음"), *sp);
                continue;
            };
            if r.field_read.iter().any(|x| &x.0 == f) {
                self.d(
                    "POLICY_FIELD_NOT_FILTERABLE",
                    format!("`{f}`에는 field read 정책이 있어 filter에 올릴 수 없음(가려진 값이 결과 유무로 추론됨)"),
                    *sp,
                );
            }
            let ok = match op.as_str() {
                // Ref는 대상 행의 Id로 비교한다(값은 Id wire를 따른다). 같은 검사를 필드 단위로 거치므로 filter 대상 규칙은 연산자와 무관하게 적용된다.
                "eq" => true,
                // in: 값 배열. 범위 비교 대상(Time/Int)과 Bool은 제외한다. 배열 길이 상한은 호출 시점(plan.rs MAX_IN_VALUES)에서 검사한다.
                "in" => matches!(tt.ty, Ty::Text | Ty::Enum(_) | Ty::Id(_) | Ty::Ref(_)),
                // isNull: bool 값. 항상 값이 있는 필드에는 의미가 없으므로 nullable 필드에만 연다.
                "isNull" => tt.nullable,
                "gte" | "lte" | "gt" | "lt" => matches!(tt.ty, Ty::Time | Ty::Int),
                "prefix" => tt.ty == Ty::Text,
                // contains: 리터럴 부분일치(Text만). 대소문자 무시(icontains)는 DB collation 의존이라 보류.
                "contains" => tt.ty == Ty::Text,
                _ => {
                    self.d("UNKNOWN_OPERATOR", format!("filter 연산 `{op}` 없음"), *sp);
                    continue;
                }
            };
            if !ok {
                self.d("OP_TYPE_MISMATCH", format!("`{f}`({})에 `{op}` 불가", tt.show()), *sp);
            }
            filters.insert(format!("{f}.{op}"));
        }
        let mut sorts = BTreeSet::new();
        for (s, sp) in &e.sort {
            if r.field_read.iter().any(|x| &x.0 == s) {
                self.d(
                    "POLICY_FIELD_NOT_FILTERABLE",
                    format!("`{s}`에는 field read 정책이 있어 sort에 올릴 수 없음(가려진 값이 정렬 순서로 추론됨)"),
                    *sp,
                );
            }
            match self.field_tt(&r.name, s) {
                Some(TT { ty: Ty::Ref(_), .. }) | Some(TT { ty: Ty::Bool, .. }) => self.d("OP_TYPE_MISMATCH", format!("`{s}`는 정렬 불가 타입"), *sp),
                Some(_) => {}
                None => self.d("UNKNOWN_FIELD", format!("sort 대상 `{s}` 없음"), *sp),
            }
            sorts.insert(s.clone());
        }
        let mut trav = Map::new();
        for (rel, sel, sp) in &e.traverse {
            let target = match self.field_tt(&r.name, rel) {
                Some(TT { ty: Ty::Ref(t), .. }) => t,
                _ => {
                    self.d("UNKNOWN_FIELD", format!("traverse `{rel}`는 참조 필드가 아님"), *sp);
                    continue;
                }
            };
            let tres = self.res[target.as_str()];
            let Some(te) = &tres.expose_read else {
                self.d("TRAVERSE_NOT_EXPOSED", format!("`{target}`에 expose read가 없어 traverse 불가"), *sp);
                continue;
            };
            let mut ss = BTreeSet::new();
            for (s, ssp) in sel {
                if !te.select.iter().any(|x| &x.0 == s) {
                    self.d("TRAVERSE_NOT_EXPOSED", format!("`{target}.{s}`는 `{target}`의 expose select에 없음"), *ssp);
                }
                ss.insert(s.clone());
            }
            trav.insert(rel.clone(), json!({ "target": target, "select": ss, "reapply": ["rowRead", "fieldRead"] }));
        }
        // budget이 없으면 무제한 기본값을 두지 않고, 다른 resource의 traverse 대상으로만 쓴다(루트 조회 불가).
        let budget = match &e.budget {
            None => Value::Null,
            Some(b) => {
                if b.rows.is_none() || b.depth.is_none() || b.deadline_ms.is_none() || b.cost.is_none() {
                    self.d("MISSING_ITEM", "budget에는 rows/depth/deadline/cost가 모두 필요", b.span);
                }
                for (k, v) in
                    [("rows", b.rows), ("depth", b.depth), ("cost", b.cost), ("deadline", b.deadline_ms.map(|x| x as i64)), ("offset", b.offset)]
                {
                    if v.is_some_and(|x| x < 1) {
                        self.d("BAD_BUDGET", format!("budget {k}는 1 이상"), b.span);
                    }
                }
                if !e.traverse.is_empty() && b.depth.unwrap_or(0) < 2 {
                    self.d("BUDGET_DEPTH_TOO_SMALL", "traverse가 있으면 depth는 2 이상", b.span);
                }
                let mut bj = json!({ "rows": b.rows, "depth": b.depth, "deadlineMs": b.deadline_ms, "cost": b.cost });
                // offset은 opt-in이라 선언이 있을 때만 키를 넣는다(기존 facts 호환).
                if let Some(o) = b.offset {
                    bj["maxOffset"] = json!(o);
                }
                bj
            }
        };
        let root = !budget.is_null();
        json!({ "select": kinds, "filter": filters, "sort": sorts, "traverse": trav, "budget": budget, "rootQueryable": root })
    }

    fn extension(&mut self, x: &Extension) -> Option<Value> {
        if x.kind == "write" {
            // 쓰기 확장: 서버 트랜잭션 안에서 공개 전이만 부른다. 효과는 DB 쓰기(db)로 명시한다.
            if x.effect != "db" {
                self.d("EFFECT_MISMATCH", format!("write extension의 effect는 db여야 함, `{}`", x.effect), x.span);
            }
            if x.deadline_ms.is_none() || x.implementation.is_empty() {
                self.d("MISSING_ITEM", "extension에 deadline·implementation 필요", x.span);
            }
            let (_, ij) = self.params(&x.input);
            let (_, oj) = self.params(&x.output);
            let mut acc = Map::new();
            for (rn, tn) in &x.access {
                let exposed = self.res.get(rn.as_str()).is_some_and(|r| r.expose_apply.iter().any(|e| &e.transition == tn));
                if !exposed {
                    self.d("UNKNOWN_SYMBOL", format!("쓰기 access `{rn}.{tn}`는 공개된 전이(expose apply)여야 함"), x.span);
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
            self.d("UNSUPPORTED", format!("extension 종류 `{}`는 spike 범위 밖", x.kind), x.span);
            return None;
        }
        if x.effect != "none" {
            self.d("EFFECT_MISMATCH", format!("read extension의 effect는 none이어야 함, `{}`", x.effect), x.span);
        }
        if x.deadline_ms.is_none() {
            self.d("MISSING_ITEM", "extension에 deadline 필요", x.span);
        }
        if x.implementation.is_empty() {
            self.d("MISSING_ITEM", "extension에 implementation 필요", x.span);
        }
        let (input, ij) = self.params(&x.input);
        let (_, oj) = self.params(&x.output);
        let mut binding = Map::new();
        for (rn, an) in &x.access {
            let Some(agg) = self.res.get(rn.as_str()).and_then(|r| r.aggregates.iter().find(|a| &a.name == an)) else {
                self.d("UNKNOWN_SYMBOL", format!("access 대상 `{rn}.{an}` 없음"), x.span);
                continue;
            };
            let mut b = Map::new();
            for (pn, pt) in &agg.input {
                let want = self.plain_type(pt);
                match input.iter().find(|(n, _)| n == pn) {
                    Some((_, t)) if Some(t) == want.as_ref() => {
                        b.insert(pn.clone(), json!(format!("input.{pn}")));
                    }
                    _ => self.d("EXT_INPUT_UNBOUND", format!("`{rn}.{an}`의 입력 `{pn}`을 extension 입력에서 같은 타입으로 받지 않음"), x.span),
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
    r.fields.iter().filter(|f| !f.ty.nullable && f.name != "id").map(|f| f.name.clone()).collect()
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
                "POLICY_EXPANSION_LIMIT",
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
                        "POLICY_CYCLE",
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
                "POLICY_EXPANSION_LIMIT",
                format!("predicate `{}`의 인라인 확장이 깊이 {expression_depth}, 노드 {node_count}로 예산을 넘음", predicates[index].name),
                predicates[index].span,
            )];
        }
        expanded_nodes[index] = node_count;
        expanded_depth[index] = expression_depth;
        total_nodes = total_nodes.saturating_add(node_count).min(node_ceiling);
        if total_nodes > MAX_POLICY_EXPANDED_NODES {
            return vec![Diag::new(
                "POLICY_EXPANSION_LIMIT",
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
                    Effect::Notify { to, span, .. } => roots.push((to, *span)),
                }
            }
        }
    }

    for (expr, owner_span) in roots {
        let metric = expression_metrics(expr);
        if metric.max_path_segments > MAX_POLICY_PATH_SEGMENTS {
            return vec![Diag::new("POLICY_EXPANSION_LIMIT", format!("정책 경로가 필드 단계 {MAX_POLICY_PATH_SEGMENTS}개 예산을 넘음"), owner_span)];
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
                "POLICY_EXPANSION_LIMIT",
                format!("정책 식의 인라인 확장이 깊이 {expression_depth}, 노드 {node_count}로 예산을 넘음"),
                owner_span,
            )];
        }
        total_nodes = total_nodes.saturating_add(node_count).min(node_ceiling);
        if total_nodes > MAX_POLICY_EXPANDED_NODES {
            return vec![Diag::new(
                "POLICY_EXPANSION_LIMIT",
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
