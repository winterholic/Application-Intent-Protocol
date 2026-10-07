//! H 리터럴을 A와 같은 AST로 옮긴다. 정책 식·타입 문자열은 A의 parser를 그대로 쓴다.
use crate::ast::*;
use crate::diag::{Diag, Span};
use crate::hlit::Lit;
use crate::parser::{parse_assignments_str, parse_duration_str, parse_expr_str, parse_type_str};

type R<T> = Result<T, Diag>;
type Obj<'a> = &'a [(String, Lit, Span)];

fn obj<'a>(l: &'a Lit, sp: Span, what: &str) -> R<Obj<'a>> {
    match l {
        Lit::Obj(v) => Ok(v),
        _ => Err(Diag::new("H_SHAPE", format!("{what}는 객체여야 함"), sp)),
    }
}
fn s<'a>(l: &'a Lit, sp: Span, what: &str) -> R<&'a str> {
    match l {
        Lit::Str(v) => Ok(v),
        _ => Err(Diag::new("H_SHAPE", format!("{what}는 문자열이어야 함"), sp)),
    }
}
fn int(l: &Lit, sp: Span, what: &str) -> R<i64> {
    match l {
        Lit::Int(v) => Ok(*v),
        _ => Err(Diag::new("H_SHAPE", format!("{what}는 정수여야 함"), sp)),
    }
}
fn boolean(l: &Lit, sp: Span, what: &str) -> R<bool> {
    match l {
        Lit::Bool(v) => Ok(*v),
        _ => Err(Diag::new("H_SHAPE", format!("{what}는 bool이어야 함"), sp)),
    }
}
fn strs(l: &Lit, sp: Span, what: &str) -> R<Vec<(String, Span)>> {
    match l {
        Lit::Arr(v) => v.iter().map(|(x, xs)| s(x, *xs, what).map(|t| (t.to_string(), *xs))).collect(),
        _ => Err(Diag::new("H_SHAPE", format!("{what}는 문자열 배열이어야 함"), sp)),
    }
}
/// 모르는 키를 조용히 무시하지 않는다(SY-6).
fn keys(o: Obj, allowed: &[&str], what: &str) -> R<()> {
    for (k, _, sp) in o {
        if !allowed.contains(&k.as_str()) {
            return Err(Diag::new("UNKNOWN_KEY", format!("{what}에 알 수 없는 키 `{k}`"), *sp));
        }
    }
    Ok(())
}
fn get<'a>(o: Obj<'a>, k: &str) -> Option<(&'a Lit, Span)> {
    o.iter().find(|x| x.0 == k).map(|x| (&x.1, x.2))
}
fn req<'a>(o: Obj<'a>, k: &str, sp: Span, what: &str) -> R<(&'a Lit, Span)> {
    get(o, k).ok_or_else(|| Diag::new("MISSING_ITEM", format!("{what}에 `{k}` 필요"), sp))
}
fn params(l: &Lit, sp: Span) -> R<Params> {
    obj(l, sp, "params")?.iter().map(|(k, v, vs)| Ok((k.clone(), parse_type_str(s(v, *vs, "타입")?, inner(*vs))?))).collect()
}
/// 문자열 값의 span은 여는 따옴표 위치다. 식 오류 위치는 따옴표 다음 글자부터 센다.
fn inner(sp: Span) -> Span {
    Span { line: sp.line, col: sp.col + 1 }
}
fn expr(l: &Lit, sp: Span, what: &str) -> R<Expr> {
    parse_expr_str(s(l, sp, what)?, inner(sp))
}

pub fn to_spec(root: &Lit, sp: Span) -> R<Spec> {
    let o = obj(root, sp, "define 인자")?;
    keys(o, &["enums", "actor", "predicates", "access", "limits", "resources"], "define")?;
    let mut spec = Spec::default();
    if let Some((e, es)) = get(o, "enums") {
        for (n, v, vs) in obj(e, es, "enums")? {
            let variants = strs(v, *vs, "enum 값")?.into_iter().map(|x| x.0).collect();
            spec.enums.push(EnumDecl { name: n.clone(), variants, span: *vs });
        }
    }
    if let Some((a, asp)) = get(o, "actor") {
        spec.actor = Some((s(a, asp, "actor")?.to_string(), asp));
    }
    if let Some((p, ps)) = get(o, "predicates") {
        for (n, v, vs) in obj(p, ps, "predicates")? {
            let po = obj(v, *vs, "predicate")?;
            keys(po, &["params", "body"], "predicate")?;
            let (pl, pls) = req(po, "params", *vs, "predicate")?;
            let (b, bs) = req(po, "body", *vs, "predicate")?;
            spec.predicates.push(Predicate { name: n.clone(), params: params(pl, pls)?, body: expr(b, bs, "body")?, span: *vs });
        }
    }
    if let Some((a, asp)) = get(o, "access") {
        for (n, v, vs) in obj(a, asp, "access")? {
            let ao = obj(v, *vs, "access")?;
            keys(ao, &["params", "guard", "totalOfVisible"], "access")?;
            let (ps, body) = match (get(ao, "totalOfVisible"), get(ao, "guard")) {
                (Some((t, ts)), None) => {
                    if let Some((_, ps)) = get(ao, "params") {
                        return Err(Diag::new("H_SHAPE", "totalOfVisible access에는 params를 쓸 수 없음", ps));
                    }
                    (vec![], AccessBody::TotalOfVisible(s(t, ts, "totalOfVisible")?.to_string(), ts))
                }
                (None, Some((g, gs))) => {
                    let ps = match get(ao, "params") {
                        Some((pl, pls)) => params(pl, pls)?,
                        None => vec![],
                    };
                    (ps, AccessBody::Guard(expr(g, gs, "guard")?))
                }
                _ => return Err(Diag::new("H_SHAPE", "access는 totalOfVisible 또는 guard 중 하나", *vs)),
            };
            spec.accesses.push(Access { name: n.clone(), params: ps, body, span: *vs });
        }
    }
    if let Some((l, ls)) = get(o, "limits") {
        for (n, v, vs) in obj(l, ls, "limits")? {
            let lo = obj(v, *vs, "limit")?;
            keys(lo, &["on", "atMost", "where"], "limit")?;
            let (on, ons) = req(lo, "on", *vs, "limit")?;
            let (am, ams) = req(lo, "atMost", *vs, "limit")?;
            let (w, ws) = req(lo, "where", *vs, "limit")?;
            spec.limits.push(Limit {
                name: n.clone(),
                on: s(on, ons, "on")?.to_string(),
                at_most: int(am, ams, "atMost")?,
                cond: expr(w, ws, "where")?,
                span: *vs,
            });
        }
    }
    let (rs, rss) = req(o, "resources", sp, "define")?;
    for (n, v, vs) in obj(rs, rss, "resources")? {
        spec.resources.push(resource(n, v, *vs)?);
    }
    Ok(spec)
}

fn resource(name: &str, l: &Lit, span: Span) -> R<Resource> {
    let o = obj(l, span, "resource")?;
    keys(
        o,
        &[
            "fields",
            "rows",
            "fieldRead",
            "read",
            "aggregates",
            "transitions",
            "unique",
            "checks",
            "invariants",
            "extensions",
            "docs",
            "exposeAggregate",
            "exposeApply",
            "exposeCreate",
            "exposeCompose",
        ],
        "resource",
    )?;
    let mut r = Resource { name: name.to_string(), span, ..Default::default() };
    if let Some((f, fs)) = get(o, "fields") {
        for (n, v, vs) in obj(f, fs, "fields")? {
            r.fields.push(Field { name: n.clone(), ty: parse_type_str(s(v, *vs, "필드 타입")?, inner(*vs))?, span: *vs });
        }
    }
    if let Some((e, es)) = get(o, "rows") {
        r.row_read = Some(expr(e, es, "rows")?);
    }
    if let Some((f, fs)) = get(o, "fieldRead") {
        for (n, v, vs) in obj(f, fs, "fieldRead")? {
            r.field_read.push((n.clone(), expr(v, *vs, "fieldRead")?, *vs));
        }
    }
    if let Some((rd, rds)) = get(o, "read") {
        let ro = obj(rd, rds, "read")?;
        keys(ro, &["select", "filter", "sort", "traverse", "budget"], "read")?;
        let mut e = ExposeRead { span: rds, ..Default::default() };
        if let Some((x, xs)) = get(ro, "select") {
            e.select = strs(x, xs, "select")?;
        }
        if let Some((x, xs)) = get(ro, "filter") {
            for (f, fsp) in strs(x, xs, "filter")? {
                let (a, b) = f.split_once('.').ok_or_else(|| Diag::new("H_SHAPE", "filter는 `필드.연산`", fsp))?;
                e.filter.push((a.to_string(), b.to_string(), fsp));
            }
        }
        if let Some((x, xs)) = get(ro, "sort") {
            e.sort = strs(x, xs, "sort")?;
        }
        if let Some((x, xs)) = get(ro, "traverse") {
            for (rel, tv, ts) in obj(x, xs, "traverse")? {
                let to = obj(tv, *ts, "traverse")?;
                if let Some((v, vs)) = get(to, "via") {
                    keys(to, &["via", "select", "sort", "limit"], "traverse")?;
                    let path = parse_expr_str(s(v, vs, "via")?, inner(vs))?;
                    let (child, via) = match path {
                        Expr::Path(p, _) if p.len() == 2 => (p[0].clone(), p[1].clone()),
                        _ => return Err(Diag::new("H_SHAPE", "via는 `Child.field`", vs)),
                    };
                    let select = get(to, "select").map(|(v, vs)| strs(v, vs, "select")).transpose()?.unwrap_or_default();
                    let sort = get(to, "sort").map(|(v, vs)| traverse_sort(v, vs)).transpose()?;
                    let limit = get(to, "limit").map(|(v, vs)| int(v, vs, "limit")).transpose()?;
                    e.traverse_many.push(TraverseMany { name: rel.clone(), child, via, select, sort, limit, span: *ts });
                } else {
                    keys(to, &["select"], "traverse")?;
                    let (sel, sels) = req(to, "select", *ts, "traverse")?;
                    e.traverse.push((rel.clone(), strs(sel, sels, "select")?, *ts));
                }
            }
        }
        if let Some((x, xs)) = get(ro, "budget") {
            let bo = obj(x, xs, "budget")?;
            keys(bo, &["rows", "depth", "deadline", "cost", "offset", "cursor"], "budget")?;
            let mut b = Budget { span: xs, ..Default::default() };
            if let Some((v, vs)) = get(bo, "rows") {
                b.rows = Some(int(v, vs, "rows")?);
            }
            if let Some((v, vs)) = get(bo, "depth") {
                b.depth = Some(int(v, vs, "depth")?);
            }
            if let Some((v, vs)) = get(bo, "deadline") {
                b.deadline_ms = Some(parse_duration_str(s(v, vs, "deadline")?, inner(vs))?);
            }
            if let Some((v, vs)) = get(bo, "cost") {
                b.cost = Some(int(v, vs, "cost")?);
            }
            if let Some((v, vs)) = get(bo, "offset") {
                b.offset = Some(int(v, vs, "offset")?);
            }
            if let Some((v, vs)) = get(bo, "cursor") {
                b.cursor = boolean(v, vs, "cursor")?;
            }
            e.budget = Some(b);
        }
        r.expose_read = Some(e);
    }
    if let Some((x, xs)) = get(o, "exposeApply") {
        for (n, v, vs) in obj(x, xs, "exposeApply")? {
            let eo = obj(v, *vs, "exposeApply")?;
            keys(eo, &["target", "bulkMaxRows", "sameScope"], "exposeApply")?;
            r.expose_apply.push(ExposeApply {
                transition: n.clone(),
                targets: match get(eo, "target") {
                    Some((t, ts)) => strs(t, ts, "target")?,
                    None => vec![],
                },
                bulk: get(eo, "bulkMaxRows").map(|(b, bs)| int(b, bs, "bulkMaxRows")).transpose()?,
                same_scope: get(eo, "sameScope").map(|(v, vs)| expr(v, vs, "sameScope")).transpose()?,
                span: *vs,
            });
        }
    }
    if let Some((x, xs)) = get(o, "exposeCreate") {
        let eo = obj(x, xs, "exposeCreate")?;
        keys(eo, &["allow", "fields"], "exposeCreate")?;
        r.expose_create = Some(ExposeCreate {
            allow: get(eo, "allow").map(|(v, vs)| expr(v, vs, "allow")).transpose()?,
            fields: get(eo, "fields").map(|(v, vs)| strs(v, vs, "fields")).transpose()?.unwrap_or_default(),
            span: xs,
        });
    }
    if let Some((x, xs)) = get(o, "exposeCompose") {
        let eo = obj(x, xs, "exposeCompose")?;
        keys(eo, &["bulkMaxRows", "sameScope", "transitions", "creates", "selfRow"], "exposeCompose")?;
        let mut creates = vec![];
        if let Some((x, xs)) = get(eo, "creates") {
            for (name, sources, span) in obj(x, xs, "creates")? {
                let sources = strs(sources, *span, "create sources")?;
                if sources.is_empty() {
                    return Err(Diag::new("H_SHAPE", "create sources에는 출처 하나 이상 필요", *span));
                }
                let sources = sources.into_iter().map(|(source, sp)| parse_expr_str(&source, inner(sp))).collect::<R<Vec<_>>>()?;
                creates.push((name.clone(), sources, *span));
            }
        }
        let self_row = get(eo, "selfRow")
            .map(|(v, vs)| {
                let raw = s(v, vs, "selfRow")?;
                match raw.split_whitespace().collect::<Vec<_>>().as_slice() {
                    [field, "by", scope] => Ok((field.to_string(), scope.to_string(), vs)),
                    _ => Err(Diag::new("H_SHAPE", "selfRow는 `<actor 참조 필드> by <범위 필드>`", vs)),
                }
            })
            .transpose()?;
        r.expose_compose = Some(ExposeCompose {
            bulk: get(eo, "bulkMaxRows").map(|(v, vs)| int(v, vs, "bulkMaxRows")).transpose()?,
            same_scope: get(eo, "sameScope").map(|(v, vs)| expr(v, vs, "sameScope")).transpose()?,
            transitions: get(eo, "transitions").map(|(v, vs)| strs(v, vs, "transitions")).transpose()?.unwrap_or_default(),
            creates,
            self_row,
            span: xs,
        });
    }
    if let Some((x, xs)) = get(o, "exposeAggregate") {
        r.expose_aggregates = strs(x, xs, "exposeAggregate")?;
    }
    if let Some((x, xs)) = get(o, "aggregates") {
        for (n, v, vs) in obj(x, xs, "aggregates")? {
            r.aggregates.push(aggregate(n, v, *vs)?);
        }
    }
    if let Some((x, xs)) = get(o, "transitions") {
        for (n, v, vs) in obj(x, xs, "transitions")? {
            let to = obj(v, *vs, "transition")?;
            keys(to, &["from", "to", "allow", "repeat", "effects"], "transition")?;
            let (f, fs) = req(to, "from", *vs, "transition")?;
            let (t, tsp) = req(to, "to", *vs, "transition")?;
            let (a, asp) = req(to, "allow", *vs, "transition")?;
            r.transitions.push(Transition {
                name: n.clone(),
                from: expr(f, fs, "from")?,
                to: parse_assignments_str(s(t, tsp, "to")?, inner(tsp))?,
                allow: expr(a, asp, "allow")?,
                repeat: get(to, "repeat").map(|(v, vs)| s(v, vs, "repeat").map(|x| (x.to_string(), vs))).transpose()?,
                effects: match get(to, "effects") {
                    Some((Lit::Arr(effects), _)) => effects.iter().map(|(effect, span)| transition_effect(effect, *span)).collect::<R<_>>()?,
                    Some((_, span)) => return Err(Diag::new("H_SHAPE", "effects는 배열", span)),
                    None => vec![],
                },
                span: *vs,
            });
        }
    }
    if let Some((x, xs)) = get(o, "unique") {
        match x {
            Lit::Arr(groups) => {
                for (group, span) in groups {
                    let columns = strs(group, *span, "unique 필드")?;
                    if columns.is_empty() {
                        return Err(Diag::new("H_SHAPE", "unique에는 필드 하나 이상 필요", *span));
                    }
                    r.uniques.push((columns, *span));
                }
            }
            _ => return Err(Diag::new("H_SHAPE", "unique는 필드 배열의 배열", xs)),
        }
    }
    if let Some((x, xs)) = get(o, "checks") {
        for (name, condition, span) in obj(x, xs, "checks")? {
            r.checks.push((name.clone(), expr(condition, *span, "check")?, *span));
        }
    }
    if let Some((x, xs)) = get(o, "invariants") {
        for (inv, isp) in strs(x, xs, "invariants")? {
            match inv.split_whitespace().collect::<Vec<_>>()[..] {
                [n, "per", f] => r.invariants.push((n.to_string(), f.to_string(), isp)),
                [n, "per", f, "deferred"] => {
                    r.invariants.push((n.to_string(), f.to_string(), isp));
                    r.deferred_invariants.push(n.to_string());
                }
                _ => return Err(Diag::new("H_SHAPE", "invariant 문자열은 `<이름> per <필드> [deferred]`", isp)),
            }
        }
    }
    if let Some((x, xs)) = get(o, "extensions") {
        for (n, v, vs) in obj(x, xs, "extensions")? {
            let eo = obj(v, *vs, "extension")?;
            keys(eo, &["kind", "input", "output", "access", "effect", "deadline", "implementation"], "extension")?;
            let mut ext = Extension {
                kind: s(req(eo, "kind", *vs, "extension")?.0, *vs, "kind")?.to_string(),
                name: n.clone(),
                input: vec![],
                output: vec![],
                access: vec![],
                effect: String::new(),
                deadline_ms: None,
                implementation: String::new(),
                span: *vs,
            };
            if let Some((p, ps)) = get(eo, "input") {
                ext.input = params(p, ps)?;
            }
            if let Some((p, ps)) = get(eo, "output") {
                ext.output = params(p, ps)?;
            }
            if let Some((p, ps)) = get(eo, "access") {
                for (a, asp) in strs(p, ps, "access")? {
                    let (rn, an) = a.split_once('.').ok_or_else(|| Diag::new("H_SHAPE", "access는 `Resource.aggregate`", asp))?;
                    ext.access.push((rn.to_string(), an.to_string()));
                }
            }
            if let Some((p, ps)) = get(eo, "effect") {
                ext.effect = s(p, ps, "effect")?.to_string();
            }
            if let Some((p, ps)) = get(eo, "deadline") {
                ext.deadline_ms = Some(parse_duration_str(s(p, ps, "deadline")?, inner(ps))?);
            }
            if let Some((p, ps)) = get(eo, "implementation") {
                ext.implementation = s(p, ps, "implementation")?.to_string();
            }
            r.extensions.push(ext);
        }
    }
    if let Some((x, xs)) = get(o, "docs") {
        let d = obj(x, xs, "docs")?;
        keys(d, &["summary", "visibility"], "docs")?;
        let mut docs = Docs { summary: None, visibility: None, span: xs };
        if let Some((v, vs)) = get(d, "summary") {
            docs.summary = Some(s(v, vs, "summary")?.to_string());
        }
        if let Some((v, vs)) = get(d, "visibility") {
            docs.visibility = Some(s(v, vs, "visibility")?.to_string());
        }
        r.docs = Some(docs);
    }
    Ok(r)
}

fn traverse_sort(value: &Lit, span: Span) -> R<(String, bool, Span)> {
    let raw = s(value, span, "sort")?;
    let parts: Vec<_> = raw.split_whitespace().collect();
    let (field, desc) = match parts.as_slice() {
        [field] | [field, "asc"] => (*field, false),
        [field, "desc"] => (*field, true),
        _ => return Err(Diag::new("H_SHAPE", "sort는 `field [asc|desc]`", span)),
    };
    match parse_expr_str(field, inner(span))? {
        Expr::Path(p, _) if p.len() == 1 => Ok((p[0].clone(), desc, span)),
        _ => Err(Diag::new("H_SHAPE", "sort에는 필드 하나만 허용", span)),
    }
}

fn aggregate(name: &str, l: &Lit, span: Span) -> R<Aggregate> {
    let o = obj(l, span, "aggregate")?;
    keys(o, &["type", "input", "source", "sourceAccess", "groupKey", "where", "callerFilter", "rowOutput", "release"], "aggregate")?;
    let (t, ts) = req(o, "type", span, "aggregate")?;
    let opt = |k: &str| -> R<Option<String>> { get(o, k).map(|(v, vs)| s(v, vs, k).map(str::to_string)).transpose() };
    let source_access = match get(o, "sourceAccess") {
        None => None,
        Some((v, vs)) => Some(match parse_expr_str(s(v, vs, "sourceAccess")?, inner(vs))? {
            Expr::Path(p, _) if p.len() == 1 => AccessRef::Name(p[0].clone()),
            Expr::Call(n, args, _) => AccessRef::Call(n, args),
            _ => return Err(Diag::new("H_SHAPE", "sourceAccess는 이름 또는 호출", vs)),
        }),
    };
    Ok(Aggregate {
        name: name.to_string(),
        ty: parse_type_str(s(t, ts, "type")?, inner(ts))?,
        input: match get(o, "input") {
            Some((p, ps)) => params(p, ps)?,
            None => vec![],
        },
        source: opt("source")?,
        source_access,
        group_key: opt("groupKey")?,
        where_: get(o, "where").map(|(v, vs)| expr(v, vs, "where")).transpose()?,
        caller_filter: opt("callerFilter")?,
        row_output: opt("rowOutput")?,
        release: opt("release")?,
        span,
    })
}

fn effect_values(value: &Lit, span: Span, what: &str) -> R<Vec<(String, Expr)>> {
    obj(value, span, what)?.iter().map(|(name, value, span)| Ok((name.clone(), expr(value, *span, what)?))).collect()
}

fn transition_effect(value: &Lit, span: Span) -> R<Effect> {
    let eo = obj(value, span, "effect")?;
    if ["create", "update", "notify"].iter().filter(|key| get(eo, key).is_some()).count() != 1 {
        return Err(Diag::new("H_SHAPE", "effect는 create/update/notify 중 하나", span));
    }
    if let Some((target, ts)) = get(eo, "create") {
        keys(eo, &["create", "values"], "create effect")?;
        let (values, vs) = req(eo, "values", span, "create effect")?;
        Ok(Effect::Create { resource: s(target, ts, "create")?.to_string(), values: effect_values(values, vs, "values")?, span })
    } else if let Some((target, ts)) = get(eo, "update") {
        keys(eo, &["update", "where", "values"], "update effect")?;
        let (matches, ms) = req(eo, "where", span, "update effect")?;
        let (values, vs) = req(eo, "values", span, "update effect")?;
        Ok(Effect::Update {
            resource: s(target, ts, "update")?.to_string(),
            matches: effect_values(matches, ms, "where")?,
            values: effect_values(values, vs, "values")?,
            span,
        })
    } else {
        keys(eo, &["notify", "topic"], "notify effect")?;
        let (target, ts) = req(eo, "notify", span, "notify effect")?;
        let (topic, ps) = req(eo, "topic", span, "notify effect")?;
        Ok(Effect::Notify { to: expr(target, ts, "notify")?, topic: s(topic, ps, "topic")?.to_string(), span })
    }
}
