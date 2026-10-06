//! Facts a client or a reviewer needs about an intent that the IR does not
//! spell out as a field: which entities a body writes, which extension effects
//! it runs, and which error codes a call can fail with. They are derived from
//! the IR alone, so every consumer (contract, docs, backends) agrees on them.
//!
//! Error codes are the language's own taxonomy (`AIP.NOT_FOUND`,
//! `AIP.PRECONDITION.FAILED` with a reason); mapping them to a transport or to
//! database objects is a backend concern and not done here.

use crate::*;
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ErrorFact {
    pub code: String,
    pub reason: Option<String>,
}

impl ErrorFact {
    pub fn new(code: &str, reason: Option<&str>) -> Self {
        ErrorFact { code: code.into(), reason: reason.map(String::from) }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// Entity names a body inserts into, updates or deletes from.
    pub writes: BTreeSet<String>,
    /// Extension operations the body runs (`mail.send`, `s3.put`, `timer.run`).
    pub effects: BTreeSet<String>,
    pub errors: BTreeSet<ErrorFact>,
}

impl Facts {
    pub fn code(&mut self, code: &str, reason: Option<&str>) {
        self.errors.insert(ErrorFact::new(code, reason));
    }
}

/// `ClubMember` -> `club_member`, `createdAt` -> `created_at`; the spelling error reasons use.
pub fn snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let chars: Vec<char> = s.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() {
            let prev_lower = i > 0 && (chars[i - 1].is_ascii_lowercase() || chars[i - 1].is_ascii_digit());
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            let prev_upper = i > 0 && chars[i - 1].is_ascii_uppercase();
            if i > 0 && (prev_lower || (prev_upper && next_lower)) {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(*c);
        }
    }
    out
}

/// Name of a callee, whatever its kind.
pub fn callee_name(c: &Call) -> &str {
    let (Callee::Fn { name } | Callee::Relation { name } | Callee::Builtin { name } | Callee::Ext { name } | Callee::Intent { name }) = &c.callee;
    name
}

/// The entity a value of type `t` refers to: one row or a collection of rows.
pub fn entity_of_type(t: &Type) -> Option<String> {
    match t {
        Type::Ref { entity } => Some(entity.clone()),
        Type::Set { of, .. } | Type::List { of, .. } | Type::Many { of } => match of.as_ref() {
            Type::Ref { entity } => Some(entity.clone()),
            _ => None,
        },
        _ => None,
    }
}

pub fn set_entity(se: &SetExpr) -> Option<String> {
    match &se.source {
        SetSource::Entity { entity } => Some(entity.clone()),
        SetSource::Expr { expr } => entity_of_type(&expr.ty),
    }
}

/// Text of a string literal.
pub fn lit_text(e: &Expr) -> Option<&str> {
    match &e.node {
        Node::Lit { lit: Literal::Text(s) } => Some(s),
        _ => None,
    }
}

fn param_name(e: &Expr) -> Option<&str> {
    match &e.node {
        Node::Param { name } | Node::Local { name } => Some(name),
        _ => None,
    }
}

/// Root binding of a path (`a.b.c` -> `a`).
fn root(e: &Expr) -> Option<String> {
    match &e.node {
        Node::Local { name } | Node::Param { name } => Some(name.clone()),
        Node::RowField { field, .. } => Some(field.clone()),
        Node::Field { base, .. } => root(base),
        _ => None,
    }
}

fn set_root(se: &SetExpr) -> Option<String> {
    match &se.source {
        SetSource::Expr { expr } => root(expr),
        SetSource::Entity { entity } => Some(entity.clone()),
    }
}

fn target_field(e: &Expr) -> Option<String> {
    match &e.node {
        Node::RowField { field, .. } | Node::Field { field, .. } => Some(field.clone()),
        _ => None,
    }
}

/// How a field is held, as far as the facts care: references and counters are
/// not plain values.
enum Held {
    Value,
    Ref(String),
    Other,
    Counter,
}

fn held(f: &Field) -> Held {
    match &f.kind {
        FieldKind::Ref { target, .. } => Held::Ref(target.clone()),
        FieldKind::RefUnion { .. } | FieldKind::Inverse { .. } => Held::Other,
        FieldKind::Counter { .. } => Held::Counter,
        _ => match &f.ty {
            Type::Snapshot { .. } => Held::Other,
            Type::Ref { entity } => Held::Ref(entity.clone()),
            _ => Held::Value,
        },
    }
}

fn field<'a>(core: &'a Program, entity: &str, name: &str) -> Option<&'a Field> {
    core.entities.get(entity)?.fields.iter().find(|f| f.name == name)
}

/// Rows an entity parameter must find (and may see) before the intent runs.
pub fn param_loads(params: &[Param], out: &mut Facts) {
    for p in params {
        if let Some(e) = entity_of_type(&p.ty) {
            out.code(codes::NOT_FOUND, Some(&format!("{}_NOT_FOUND", snake(&e).to_uppercase())));
        }
    }
}

fn let_code(l: &Let, out: &mut Facts) {
    if let Some(code) = &l.code {
        out.code(codes::PRECONDITION_FAILED, Some(code));
    }
}

/// Errors a transition-guarded assignment can fail with.
fn transition(core: &Program, entity: &str, field_name: &str, out: &mut Facts) {
    let Some(info) = core.entities.get(entity) else { return };
    if !info.lifecycles.iter().any(|l| l.field == field_name) {
        return;
    }
    if !field(core, entity, field_name).is_some_and(|f| matches!(held(f), Held::Value)) {
        return;
    }
    out.code(codes::PRECONDITION_FAILED, Some(&format!("{}_{}_INVALID_TRANSITION", snake(entity).to_uppercase(), snake(field_name).to_uppercase())));
}

/// Writes, effects and errors of a statement block, nested blocks included.
pub fn collect_stmts(core: &Program, stmts: &[Stmt], out: &mut Facts) {
    for s in stmts {
        collect_stmt(core, s, out);
    }
}

fn collect_stmt(core: &Program, s: &Stmt, out: &mut Facts) {
    match s {
        Stmt::Let(l) => let_code(l, out),
        Stmt::Insert { entity, .. } | Stmt::Upsert { entity, .. } | Stmt::Toggle { entity, .. } => {
            out.writes.insert(entity.clone());
        }
        Stmt::Update { target, via, assigns } => {
            let Some(entity) = set_entity(target) else { return };
            out.writes.insert(entity.clone());
            if via.is_none() {
                for a in assigns {
                    transition(core, &entity, &target_field(&a.target).unwrap_or_default(), out);
                }
            }
        }
        Stmt::Delete { target } | Stmt::Purge { target } => {
            if let SetSource::Expr { expr } = &target.source
                && let Type::RefUnion { entities } = &expr.ty
            {
                out.writes.extend(entities.iter().cloned());
            } else if let Some(e) = set_entity(target) {
                out.writes.insert(e);
            }
        }
        Stmt::Erase { target } => erase(core, target, out),
        Stmt::Set { assigns } => {
            for a in assigns {
                let Node::Field { base, field: f, .. } = &a.target.node else { continue };
                let Some(entity) = entity_of_type(&base.ty) else { continue };
                out.writes.insert(entity.clone());
                transition(core, &entity, f, out);
            }
        }
        Stmt::When { body, .. } | Stmt::Each { body, .. } => collect_stmts(core, body, out),
        Stmt::Effect { call, into, on_failure, .. } => {
            let name = callee_name(call);
            out.effects.insert(name.to_string());
            if name == "s3.put"
                && let Some(Expr { node: Node::Field { base, .. }, .. }) = into
                && let Some(e) = entity_of_type(&base.ty)
            {
                out.writes.insert(e);
            }
            if let Some(b) = on_failure {
                collect_stmts(core, b, out);
            }
        }
        Stmt::Reserve { .. } => {}
        Stmt::AtRun { .. } => {
            out.effects.insert("timer.run".into());
        }
        Stmt::Notify(n) => {
            out.effects.insert(callee_name(&n.via).to_string());
        }
        Stmt::ExportPersonalData { .. } => {
            out.effects.insert("export.personal".into());
        }
    }
}

/// `erase x`: every reference to the row follows its `on erase` policy, broken
/// cardinality groups run their repair, then the row goes.
fn erase(core: &Program, target: &Expr, out: &mut Facts) {
    let Some(pentity) = entity_of_type(&target.ty) else { return };
    for (ename, info) in &core.entities {
        if *ename == pentity {
            continue;
        }
        for f in &info.fields {
            let Held::Ref(tgt) = held(f) else { continue };
            if tgt != pentity {
                continue;
            }
            for con in &info.constraints {
                if let Constraint::Cardinality { per, repair: Some(block), .. } = con
                    && field(core, ename, per).is_some_and(|p| matches!(held(p), Held::Ref(_) | Held::Value))
                {
                    collect_stmts(core, block, out);
                }
            }
            out.writes.insert(ename.clone());
            if let FieldKind::Ref { on_erase: Some(RefPolicy::Restrict), .. } = &f.kind {
                out.code(codes::PRECONDITION_FAILED, Some(&format!("ERASE_RESTRICTED_BY_{}", snake(ename).to_uppercase())));
            }
        }
    }
    out.writes.insert(pentity);
}

/// Parameter names whose rows a body writes (the rows a command must lock for update).
pub fn written_roots(body: &[Stmt]) -> BTreeSet<String> {
    fn eq_names(e: &Expr, out: &mut BTreeSet<String>) {
        match &e.node {
            Node::Binary { l, r, .. } => {
                eq_names(l, out);
                eq_names(r, out);
            }
            Node::Local { name } | Node::Param { name } => {
                out.insert(name.clone());
            }
            Node::RowField { field, .. } => {
                out.insert(field.clone());
            }
            _ => {}
        }
    }
    fn walk(stmts: &[Stmt], out: &mut BTreeSet<String>) {
        for s in stmts {
            match s {
                Stmt::Set { assigns } => {
                    for a in assigns {
                        if let Node::Field { base, .. } = &a.target.node
                            && let Some(r) = root(base)
                        {
                            out.insert(r);
                        }
                    }
                }
                Stmt::Update { target, .. } | Stmt::Delete { target } | Stmt::Purge { target } => {
                    if let Some(r) = set_root(target) {
                        out.insert(r);
                    }
                    if let Some(f) = &target.filter {
                        eq_names(f, out);
                    }
                }
                Stmt::Effect { into: Some(t), .. } => {
                    if let Some(r) = root(t) {
                        out.insert(r);
                    }
                }
                Stmt::When { body, .. } | Stmt::Each { body, .. } => walk(body, out),
                _ => {}
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(body, &mut out);
    out
}

/// `root -> fields` assigned directly by `set root.f = ...`; `*` for delete/update of the root.
/// Relative updates (`+=`, `-=`) are left out: they commute, so they cannot lose an edit.
fn edited_fields(body: &[Stmt]) -> BTreeMap<String, BTreeSet<String>> {
    fn walk(stmts: &[Stmt], out: &mut BTreeMap<String, BTreeSet<String>>) {
        for s in stmts {
            match s {
                Stmt::Set { assigns } => {
                    for a in assigns {
                        if matches!(a.op, AssignOp::Set)
                            && let Node::Field { base, field: f, .. } = &a.target.node
                            && let Some(r) = param_name(base)
                        {
                            out.entry(r.to_string()).or_default().insert(f.clone());
                        }
                    }
                }
                Stmt::Update { assigns, .. } if assigns.iter().all(|a| !matches!(a.op, AssignOp::Set)) => {}
                Stmt::Delete { .. } | Stmt::Update { .. } => {
                    for r in written_roots(std::slice::from_ref(s)) {
                        out.entry(r).or_default().insert("*".into());
                    }
                }
                Stmt::When { body, .. } => walk(body, out),
                _ => {}
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(body, &mut out);
    out
}

/// The version a client states when it edits a row of a `versioned` entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionParam {
    /// The entity parameter being edited.
    pub of: String,
    /// The extra parameter that carries the version (`<of>Version`).
    pub name: String,
    pub reason: String,
}

/// `versioned` entities: a command that edits a row it received as a parameter
/// must say which version it edited.
pub fn version_params(core: &Program, c: &Command) -> Vec<VersionParam> {
    let edited = edited_fields(&c.body);
    let mut out = Vec::new();
    for p in &c.params {
        let Type::Ref { entity } = &p.ty else { continue };
        let Some(info) = core.entities.get(entity) else { continue };
        if !info.traits.versioned {
            continue;
        }
        let Some(fields) = edited.get(&p.name) else { continue };
        let moves_value = |f: &String| f == "*" || !field(core, entity, f).is_some_and(|x| matches!(held(x), Held::Counter));
        if !fields.iter().any(moves_value) {
            continue;
        }
        let name = format!("{}Version", p.name);
        if c.params.iter().any(|x| x.name == name) {
            continue;
        }
        out.push(VersionParam { of: p.name.clone(), name, reason: format!("{}_VERSION_STALE", snake(entity).to_uppercase()) });
    }
    out
}

/// Errors a write to `entity` can fail with because of the constraints the
/// entity declares, and the values the runtime generates for it.
pub fn violation_errors(core: &Program, entity: &str) -> Vec<ErrorFact> {
    let Some(info) = core.entities.get(entity) else { return Vec::new() };
    let mut out = Vec::new();
    for f in &info.fields {
        if let Some(Generated::Sequence { .. } | Generated::Slug { .. }) = &f.generated
            && matches!(held(f), Held::Value)
        {
            out.push(ErrorFact::new(codes::CONFLICT_UNIQUE, Some(&format!("{}_TAKEN", snake(&f.name).to_uppercase()))));
        }
    }
    for con in &info.constraints {
        match con {
            Constraint::Unique { code, .. } => out.push(ErrorFact { code: codes::CONFLICT_UNIQUE.into(), reason: code.clone() }),
            Constraint::Cardinality { kind, .. } => {
                let which = match kind {
                    crate::Cardinality::ExactlyOne => "EXACTLY_ONE",
                    crate::Cardinality::AtMostOne => "AT_MOST_ONE",
                    crate::Cardinality::AtLeastOne => "AT_LEAST_ONE",
                };
                out.push(ErrorFact::new(codes::INVARIANT_VIOLATED, Some(&format!("{}_{which}", snake(entity).to_uppercase()))));
            }
            Constraint::NoOverlap { code, .. } => out.push(ErrorFact { code: codes::CONFLICT_OVERLAP.into(), reason: code.clone() }),
            Constraint::Invariant { name, .. } => out.push(ErrorFact::new(codes::INVARIANT_VIOLATED, Some(&name.to_uppercase()))),
            Constraint::Capacity { code, .. } => out.push(ErrorFact::new(codes::CONFLICT_CAPACITY, Some(code))),
        }
    }
    out
}

/// What every command can fail with besides its own checks: bad input, no
/// caller, a replayed key, the constraints of what it writes, a lost race.
pub fn finish_command(core: &Program, idempotent: bool, f: &mut Facts) {
    f.code(codes::INPUT_INVALID, None);
    f.code(codes::AUTH_UNAUTHENTICATED, None);
    if idempotent {
        f.code(codes::IDEMPOTENCY_KEY_REUSED, None);
    }
    let extra: Vec<ErrorFact> = f.writes.iter().flat_map(|w| violation_errors(core, w)).collect();
    f.errors.extend(extra);
    f.code(codes::CONCURRENCY_CONFLICT, None);
}

/// Facts of a declared command.
pub fn command_facts(core: &Program, name: &str, c: &Command) -> Facts {
    let mut f = Facts::default();
    param_loads(&c.params, &mut f);
    for l in &c.lets {
        let_code(l, &mut f);
    }
    if let Some(al) = &c.allow {
        f.code(codes::AUTH_FORBIDDEN, al.code.as_deref());
    }
    for v in version_params(core, c) {
        f.code(codes::CONFLICT_STALE_VERSION, Some(&v.reason));
    }
    for r in &c.requires {
        f.code(codes::PRECONDITION_FAILED, Some(&r.code));
    }
    collect_stmts(core, &c.body, &mut f);
    finish_command(core, !matches!(c.idempotency, Idempotency::None), &mut f);
    tenant_errors(core, &c.params, c.cross_tenant, Some(&f.writes.clone()), &mut f);
    consent_errors(core, name, &mut f);
    if crate::form_intents::touches_personal_data(&c.body) {
        crate::form_intents::refuse_impersonated(core, &mut f);
    }
    f
}

/// `AIP.CONSENT.REQUIRED` once per consent that lists the intent; the reason is the consent's name.
pub fn consent_errors(core: &Program, intent: &str, out: &mut Facts) {
    for c in crate::form_intents::consents_of(core, intent) {
        out.code(codes::CONSENT_REQUIRED, Some(&c.name));
    }
}

/// `AIP.TENANT.MISMATCH`: parameters that can disagree on a tenant, or a write into a tenant-scoped entity (a reference may cross tenants).
pub fn tenant_errors(core: &Program, params: &[Param], cross: bool, writes: Option<&BTreeSet<String>>, out: &mut Facts) {
    if cross {
        return;
    }
    let checked = tenant::checked_params(core, params);
    let writes_scoped = writes.is_some_and(|w| w.iter().any(|e| tenant::scoped(core, e)));
    if tenant::needs_check(&checked) || writes_scoped {
        out.code(codes::TENANT_MISMATCH, None);
    }
}

/// Errors of a declared query.
pub fn query_errors(core: &Program, name: &str, q: &Query) -> BTreeSet<ErrorFact> {
    let mut f = Facts::default();
    param_loads(&q.params, &mut f);
    for l in &q.lets {
        let_code(l, &mut f);
    }
    if let Some(al) = &q.allow {
        f.code(codes::AUTH_FORBIDDEN, al.code.as_deref());
    }
    f.code(codes::INPUT_INVALID, None);
    f.code(codes::AUTH_FORBIDDEN, None);
    tenant_errors(core, &q.params, q.cross_tenant, None, &mut f);
    consent_errors(core, name, &mut f);
    f.errors
}
