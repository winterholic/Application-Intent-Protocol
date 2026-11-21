//! Advice on how a policy was written. Nothing here rejects a definition.
//!
//! AIP has no automatic tenant isolation (product/SUPPORT.md 13): the condition `org = actor.org` is written by hand in
//! every resource, so a forgotten one is the one mistake the checker can still point at. The three advisories read the
//! normalized facts (predicate calls inlined, `=` operands ordered), so a policy written through a predicate and the same
//! policy written inline are judged alike. A policy that contains `exists` is not judged: membership tables express
//! tenancy in ways a structural comparison cannot follow, and a warning there would mostly be wrong.

use crate::ast::Spec;
use crate::diag::Span;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const TENANT_UNSCOPED_READ: &str = "DEF.ADVISORY_TENANT_UNSCOPED_READ";
pub const WRITE_WIDER_THAN_READ: &str = "DEF.ADVISORY_WRITE_WIDER_THAN_READ";
pub const PUBLIC_READ_TENANT_FIELD: &str = "DEF.ADVISORY_PUBLIC_READ_TENANT_FIELD";

pub type Advice = (&'static str, String, Span);

const MAX_INLINE_DEPTH: usize = 16;

/// A reference field of a resource that the actor resource also has, by the same name and target:
/// `User.org: Org` and `Task.org: Org` make `org` the tenant field of `Task`.
struct Tenant {
    /// Field of the resource under judgement.
    field: String,
    /// Field of the actor resource it is compared with.
    actor_field: String,
    /// Resource the two fields point to.
    target: String,
}

pub fn advise(spec: &Spec, facts: &Value) -> Vec<Advice> {
    let Some(actor) = facts["actor"].as_str() else { return vec![] };
    let Some(resources) = facts["resources"].as_object() else { return vec![] };
    let preds = facts["predicates"].as_object().cloned().unwrap_or_default();
    // actor가 resource가 아닌 정의(operation 전용 principal)는 견줄 필드가 없다.
    let Some(actor_resource) = resources.get(actor) else { return vec![] };
    let actor_refs = ref_fields(&actor_resource["fields"]);
    // 같은 이름·대상의 참조 쌍이 이 정의의 어느 정책에서 실제로 actor와 묶여 있을 때만 테넌트 필드로 본다.
    // `primary_group: Group`처럼 actor와 이름·대상이 같아도 아무 정책도 묶지 않는 필드는 역할이나 속성이지 테넌트가 아니다.
    let mut conventions: Vec<(String, String)> = vec![];
    for (name, rf) in resources {
        let Some(read) = policy(&rf["rowRead"], &preds) else { continue };
        if has_exists(&read) {
            continue;
        }
        for (field, target) in pairs(&rf["fields"], &actor_refs, actor) {
            let t = Tenant { field: field.clone(), actor_field: field.clone(), target: target.clone() };
            if ensures_tenant(&read, &t, true) {
                conventions.push((field, target));
            }
        }
        // 테넌트 resource 자신: `rows read when id = actor.org`.
        if let Some((g, _)) = actor_refs.iter().find(|(g, t)| **t == *name && pins(&read, g) && *name != actor) {
            conventions.push((g.clone(), name.clone()));
        }
    }
    let mut out = vec![];
    for r in &spec.resources {
        let Some(rf) = resources.get(&r.name) else { continue };
        let Some(read) = policy(&rf["rowRead"], &preds) else { continue };
        let tenants: Vec<Tenant> = pairs(&rf["fields"], &actor_refs, actor)
            .into_iter()
            .filter(|pair| conventions.contains(pair))
            .map(|(field, target)| Tenant { actor_field: field.clone(), field, target })
            .collect();
        let is_true = read == json!({"lit": true});

        // 1. 테넌트 필드가 있는데 행 정책이 그 필드를 actor와 묶지 않는다.
        if !tenants.is_empty() && !is_true && !has_exists(&read) && !tenants.iter().any(|t| ensures_tenant(&read, t, false)) {
            let t = &tenants[0];
            out.push((
                TENANT_UNSCOPED_READ,
                format!(
                    "resource `{}`의 `rows read`에 테넌트 조건이 없음: `{}.{}`가 `actor.{}`와 같은 `{}` 참조인데 정책이 이 둘을 견주지 않아, 다른 `{}`의 행도 읽힐 수 있다. 자동 테넌트 격리는 없으므로 의도한 공개 범위가 아니면 `rows read when {} = actor.{}`처럼 쓴다",
                    r.name, r.name, t.field, t.actor_field, t.target, t.target, t.field, t.actor_field
                ),
                r.span,
            ));
        }

        // 3. 모든 행이 읽히는 resource인데 행이 테넌트에 묶여 있다.
        if is_true && !tenants.is_empty() {
            let tied: Vec<&str> = tenants.iter().map(|t| t.field.as_str()).collect();
            out.push((
                PUBLIC_READ_TENANT_FIELD,
                format!(
                    "resource `{}`는 `rows read when true`로 모든 호출자에게 모든 행이 보이는데 `{}` 필드는 actor의 같은 이름 필드와 같은 대상을 가리키고, 이 정의의 다른 정책은 그 필드를 `actor`와 묶음. 행이 테넌트별 데이터라면 다른 테넌트의 행이 노출된다",
                    r.name,
                    tied.join("`, `"),
                ),
                r.span,
            ));
        }

        // 2. 쓰기가 읽기 정책보다 넓다.
        if is_true || has_exists(&read) {
            continue;
        }
        if let (Some(c), Some(allow)) = (&r.expose_create, policy(&rf["exposeCreate"]["allow"], &preds)) {
            if !has_exists(&allow) {
                let chosen: Vec<&str> = c.fields.iter().map(|(f, _)| f.as_str()).collect();
                let missing = missing_terms(&allow, &read)
                    .into_iter()
                    .filter(|m| caller_decides(m, &chosen) && !covered_by_ref_readability(m, &rf["fields"], resources, &preds))
                    .collect::<Vec<_>>();
                if !missing.is_empty() {
                    out.push((
                        WRITE_WIDER_THAN_READ,
                        format!(
                            "resource `{}`의 `expose create` allow가 `rows read`보다 넓음: allow가 읽기 조건 `{}`를 요구하지 않아 호출자가 자기에게 보이지 않는 행을 만들 수 있다",
                            r.name,
                            clip(&missing.iter().map(show).collect::<Vec<_>>().join(" and "))
                        ),
                        c.span,
                    ));
                }
            }
        }
        for t in &r.transitions {
            if !r.expose_apply.iter().any(|e| e.transition == t.name && !e.internal) {
                continue;
            }
            for tenant in &tenants {
                if t.patch.iter().any(|(f, _)| *f == tenant.field)
                    && !ref_readability_restricts(&tenant.target, resources, &preds, &tenant.actor_field)
                {
                    out.push((
                        WRITE_WIDER_THAN_READ,
                        format!(
                            "resource `{}`의 transition `{}`가 테넌트 필드 `{}`를 patch로 바꿀 수 있고 `{}`의 `rows read`가 호출자의 `actor.{}`로 한정하지 않아, 행을 다른 `{}`로 옮겨 호출자 가시성 밖으로 보낼 수 있다",
                            r.name, t.name, tenant.field, tenant.target, tenant.actor_field, tenant.target
                        ),
                        t.span,
                    ));
                }
            }
        }
    }
    out
}

/// (field, target) of the reference fields that the actor resource has by the same name and target.
fn pairs(fields: &Value, actor_refs: &BTreeMap<String, String>, actor: &str) -> Vec<(String, String)> {
    ref_fields(fields).into_iter().filter(|(f, t)| actor_refs.get(f) == Some(t) && t != actor).collect()
}

/// Field name → target resource for the `Ref<T>` / `Ref<T>?` fields of a resource.
fn ref_fields(fields: &Value) -> BTreeMap<String, String> {
    fields
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, f)| {
            let ty = f["ty"].as_str()?.trim_end_matches('?');
            Some((name.clone(), ty.strip_prefix("Ref<")?.strip_suffix('>')?.to_string()))
        })
        .collect()
}

/// The policy with predicate calls inlined and the operands of `=` / `!=` put in a fixed order.
/// `None` when the policy is not an expression (no `rows read`) or a call cannot be followed.
fn policy(v: &Value, preds: &serde_json::Map<String, Value>) -> Option<Value> {
    if v.is_null() || v.get("default").is_some() {
        return None;
    }
    inline(v, preds, 0)
}

fn inline(v: &Value, preds: &serde_json::Map<String, Value>, depth: usize) -> Option<Value> {
    if depth > MAX_INLINE_DEPTH {
        return None;
    }
    if let Some(name) = v.get("call").and_then(Value::as_str) {
        let p = preds.get(name)?;
        let params = p["params"].as_array()?;
        let args = v["args"].as_array()?;
        let mut bind = BTreeMap::new();
        for (param, arg) in params.iter().zip(args) {
            bind.insert(format!("var.{}", param[0].as_str()?), inline(arg, preds, depth + 1)?);
        }
        return inline(&substitute(&p["body"], &bind), preds, depth + 1);
    }
    let mut out = v.clone();
    match &mut out {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if k == "path" || k == "ty" {
                    continue;
                }
                *x = inline(x, preds, depth + 1)?;
            }
        }
        Value::Array(a) => {
            for x in a.iter_mut() {
                *x = inline(x, preds, depth + 1)?;
            }
        }
        _ => {}
    }
    if let Value::Object(m) = &mut out {
        order_cmp(m);
    }
    Some(out)
}

/// Replaces `var.<param>` path roots by the argument; a path argument extends its segments.
fn substitute(v: &Value, bind: &BTreeMap<String, Value>) -> Value {
    match v {
        Value::Object(m) => {
            if let (Some(path), Some(arg)) = (m.get("path"), path_root(m).and_then(|root| bind.get(root))) {
                if let Some(base) = arg.get("path") {
                    let mut segs = base["segs"].as_array().cloned().unwrap_or_default();
                    segs.extend(path["segs"].as_array().cloned().unwrap_or_default());
                    let mut out = m.clone();
                    out.insert("path".into(), json!({"root": base["root"], "segs": segs}));
                    return Value::Object(out);
                }
                return arg.clone();
            }
            Value::Object(m.iter().map(|(k, x)| (k.clone(), substitute(x, bind))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute(x, bind)).collect()),
        _ => v.clone(),
    }
}

fn path_root(m: &serde_json::Map<String, Value>) -> Option<&str> {
    m.get("path")?.get("root")?.as_str()
}

fn has_exists(v: &Value) -> bool {
    match v {
        Value::Object(m) => m.contains_key("exists") || m.values().any(has_exists),
        Value::Array(a) => a.iter().any(has_exists),
        _ => false,
    }
}

fn segs(path: &Value) -> Vec<&str> {
    path["segs"].as_array().into_iter().flatten().filter_map(Value::as_str).collect()
}

fn this_path(side: &Value) -> Option<Vec<&str>> {
    let p = side.get("path")?;
    (p["root"] == "this").then(|| segs(p))
}

fn actor_path(side: &Value) -> Option<Vec<&str>> {
    let p = side.get("path")?;
    (p["root"] == "actor").then(|| segs(p))
}

/// The condition holds for every row the policy admits: one AND term is enough, every branch of an OR is needed.
/// A term ties the row to the caller when it equals the tenant pair, or the row (or a row it points to through
/// `this.a.b.field`) to the actor itself; the second keeps `owner = actor` and `id = actor.id` from being flagged.
fn ensures_tenant(v: &Value, t: &Tenant, strict: bool) -> bool {
    if let Some(items) = v.get("and").and_then(Value::as_array) {
        return items.iter().any(|x| ensures_tenant(x, t, strict));
    }
    if let Some(items) = v.get("or").and_then(Value::as_array) {
        return !items.is_empty() && items.iter().all(|x| ensures_tenant(x, t, strict));
    }
    if v.get("cmp").and_then(Value::as_str) != Some("=") {
        return false;
    }
    for (a, b) in [(&v["l"], &v["r"]), (&v["r"], &v["l"])] {
        let (Some(row), Some(who)) = (this_path(a), actor_path(b)) else { continue };
        if who.as_slice() == [t.actor_field.as_str()] && row.last() == Some(&t.field.as_str()) {
            return true;
        }
        if !strict && (who.is_empty() || who.as_slice() == ["id"]) {
            return true;
        }
    }
    false
}

/// AND terms of `read` that `allow` does not imply (`allow` need not repeat a term it already contains).
fn missing_terms(allow: &Value, read: &Value) -> Vec<Value> {
    fn flatten<'a>(e: &'a Value, out: &mut Vec<&'a Value>) {
        match e.get("and").and_then(Value::as_array) {
            Some(items) => items.iter().for_each(|x| flatten(x, out)),
            None => out.push(e),
        }
    }
    fn covered(have: &[&Value], need: &Value) -> bool {
        if have.contains(&need) {
            return true;
        }
        if let Some(terms) = need.get("and").and_then(Value::as_array) {
            return terms.iter().all(|t| covered(have, t));
        }
        need.get("or").and_then(Value::as_array).is_some_and(|b| b.iter().any(|x| covered(have, x)))
    }
    let mut have = vec![];
    flatten(allow, &mut have);
    let mut terms = vec![];
    flatten(read, &mut terms);
    terms.into_iter().filter(|t| !covered(&have, t)).cloned().collect()
}

/// The caller can influence the term through a field it sets in `expose create`. A term that reads only fields the server
/// or the database fills, or only the caller (`actor`), is not about which row the caller chose to create.
fn caller_decides(term: &Value, chosen: &[&str]) -> bool {
    fn walk(v: &Value, chosen: &[&str]) -> bool {
        match v {
            Value::Object(m) => {
                if let Some(first) = m.get("path").and_then(this_path_of).and_then(|p| p.into_iter().next()) {
                    return chosen.contains(&first.as_str());
                }
                m.values().any(|x| walk(x, chosen))
            }
            Value::Array(a) => a.iter().any(|x| walk(x, chosen)),
            _ => false,
        }
    }
    walk(term, chosen)
}

fn this_path_of(path: &Value) -> Option<Vec<String>> {
    (path["root"] == "this").then(|| segs(path).into_iter().map(String::from).collect())
}

/// A created row has to point to rows the caller can read (`ensure_refs_readable`). A missing term that is only about
/// a row it points to is therefore still guaranteed when that row's own `rows read` contains the same term:
/// `video.is_deleted = false` for a `video` whose `rows read` has `is_deleted = false`, or `org = actor.org` for an
/// `org` whose rows are limited to `actor.org` itself.
fn covered_by_ref_readability(
    term: &Value,
    fields: &Value,
    resources: &serde_json::Map<String, Value>,
    preds: &serde_json::Map<String, Value>,
) -> bool {
    let refs = ref_fields(fields);
    // 항이 읽는 this 경로가 모두 같은 참조 필드 하나에서 시작해야 그 필드의 대상 행 정책으로 판정할 수 있다.
    let mut firsts = vec![];
    collect_this_firsts(term, &mut firsts);
    firsts.sort();
    firsts.dedup();
    let [field] = firsts.as_slice() else { return false };
    let Some(target) = refs.get(field) else { return false };
    let Some(target_read) = resources.get(target).and_then(|r| policy(&r["rowRead"], preds)) else { return false };
    if has_exists(&target_read) {
        return false;
    }
    // `this.f = actor.g`: 대상 행이 actor.g 자신뿐이다.
    if term.get("cmp").and_then(Value::as_str) == Some("=") {
        for (a, b) in [(&term["l"], &term["r"]), (&term["r"], &term["l"])] {
            if let (Some(row), Some(who)) = (this_path(a), actor_path(b)) {
                if let ([f], [g]) = (row.as_slice(), who.as_slice()) {
                    if f == field && pins(&target_read, g) {
                        return true;
                    }
                }
            }
        }
    }
    // `this.f.rest ...`: 같은 항이 대상 행 정책에도 있다.
    let stripped = strip_first(term, field);
    missing_terms(&target_read, &stripped).is_empty()
}

fn collect_this_firsts(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            if let Some(first) = m.get("path").and_then(this_path_of).and_then(|p| p.into_iter().next()) {
                out.push(first);
            } else {
                m.values().for_each(|x| collect_this_firsts(x, out));
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_this_firsts(x, out)),
        _ => {}
    }
}

/// The term as it reads from the row that `field` points to: `this.video.is_deleted` becomes `this.is_deleted`.
fn strip_first(v: &Value, field: &str) -> Value {
    match v {
        Value::Object(m) => {
            if let Some(path) = m.get("path").filter(|p| p["root"] == "this" && segs(p).first() == Some(&field)) {
                let mut out = m.clone();
                out.insert("path".into(), json!({"root": "this", "segs": segs(path)[1..]}));
                return Value::Object(out);
            }
            let mut out: serde_json::Map<String, Value> = m.iter().map(|(k, x)| (k.clone(), strip_first(x, field))).collect();
            order_cmp(&mut out);
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| strip_first(x, field)).collect()),
        _ => v.clone(),
    }
}

/// `Value` has no ordering, so the operands are ordered by their JSON text.
#[allow(clippy::cmp_owned)]
fn order_cmp(m: &mut serde_json::Map<String, Value>) {
    if matches!(m.get("cmp").and_then(Value::as_str), Some("=" | "!=")) && m["l"].to_string() > m["r"].to_string() {
        let (l, r) = (m["l"].clone(), m["r"].clone());
        m.insert("l".into(), r);
        m.insert("r".into(), l);
    }
}

/// The rows of `target` that a caller can read are exactly `actor.<actor_field>` (`rows read when id = actor.org`).
fn ref_readability_restricts(
    target: &str,
    resources: &serde_json::Map<String, Value>,
    preds: &serde_json::Map<String, Value>,
    actor_field: &str,
) -> bool {
    let Some(read) = resources.get(target).and_then(|r| policy(&r["rowRead"], preds)) else { return true };
    pins(&read, actor_field)
}

/// `rows read when id = actor.org` (alone or as an AND term): the readable rows are the caller's own `actor.<actor_field>`.
fn pins(v: &Value, actor_field: &str) -> bool {
    if let Some(items) = v.get("and").and_then(Value::as_array) {
        return items.iter().any(|x| pins(x, actor_field));
    }
    if v.get("cmp").and_then(Value::as_str) != Some("=") {
        return false;
    }
    [(&v["l"], &v["r"]), (&v["r"], &v["l"])]
        .into_iter()
        .any(|(a, b)| this_path(a).is_some_and(|p| p.is_empty() || p == ["id"]) && actor_path(b).is_some_and(|p| p == [actor_field]))
}

fn clip(text: &str) -> String {
    const MAX: usize = 160;
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    format!("{}...", text.chars().take(MAX).collect::<String>())
}

/// A condition as the author wrote it, close enough to quote in a message.
fn show(v: &Value) -> String {
    if let Some(items) = v.get("and").and_then(Value::as_array) {
        return items.iter().map(show).collect::<Vec<_>>().join(" and ");
    }
    if let Some(items) = v.get("or").and_then(Value::as_array) {
        return format!("({})", items.iter().map(show).collect::<Vec<_>>().join(" or "));
    }
    if let Some(op) = v.get("cmp").and_then(Value::as_str) {
        let (l, r) = if this_path(&v["r"]).is_some() && this_path(&v["l"]).is_none() { (&v["r"], &v["l"]) } else { (&v["l"], &v["r"]) };
        return format!("{} {op} {}", show(l), show(r));
    }
    if let Some(p) = v.get("path") {
        let mut parts = vec![p["root"].as_str().unwrap_or("").trim_start_matches("var.").to_string()];
        parts.extend(segs(p).into_iter().map(String::from));
        if parts[0] == "this" {
            parts.remove(0);
        }
        return parts.join(".");
    }
    if let Some(e) = v.get("enum").and_then(Value::as_str) {
        return e.rsplit('.').next().unwrap_or(e).to_string();
    }
    if let Some(l) = v.get("lit") {
        return l.to_string();
    }
    "...".to_string()
}
