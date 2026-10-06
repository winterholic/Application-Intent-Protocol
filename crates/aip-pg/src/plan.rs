//! Intents -> execution plans. Each command becomes an ordered list of steps
//! run inside one transaction; each query becomes one JSON-producing statement.

use crate::names::{lit, q, snake};
use crate::schema::{self, Col, Schema};
use crate::select;
use crate::sqlexpr::{ACTOR, Compiler, SetElem, Val, duration_sql, finalize, lone_marker, marker, typed_marker};
use crate::tenant::{self as tn, TenantScope};
use crate::ty::{Ty, type_spec};
use aip_ir::codes;
use aip_ir::facts;
use aip_ir::form_intents::{self, FormIntent};
use aip_ir::tenant;
use aip_ir::{self as ir, AssignOp, Expr, Literal, Node, SetSource, Stmt, Type};
use aip_plan::*;
use std::collections::{BTreeMap, BTreeSet};

mod approval;
mod consent;
mod impersonate;
mod job;
mod outbound;
mod publish;
mod rule;
pub mod subscribe;
mod webhook;

pub struct Planner<'x> {
    pub core: &'x ir::Program,
    pub s: &'x Schema,
    /// What clients read: `s` with every publishable entity pointing at its published table. `None` when there is none.
    published: Option<&'x Schema>,
    pub errors: Vec<Diagnostic>,
    /// IR path of the declaration being planned; diagnostics point at it.
    pub at: String,
    /// (bind, field) -> (upload param, bucket) for `s3.put(...) into bind.field` after `insert ... as bind`.
    pending_uploads: BTreeMap<(String, String), (String, String)>,
    /// Validation steps produced while compiling column values; flushed before the write.
    validations: Vec<Step>,
    /// `Encrypt` steps produced while compiling column values; flushed before the write, like `validations`.
    encrypts: Vec<Step>,
    /// Numbers the bound names of encrypted writes (`__enc.N`, `__id.N`).
    enc_seq: usize,
    emits: BTreeSet<String>,
}

fn sql(text: String) -> Sql {
    finalize(&text)
}

pub(crate) use aip_ir::facts::{callee_name, lit_text};

fn error_spec(e: &facts::ErrorFact) -> ErrorSpec {
    ErrorSpec { code: e.code.clone(), reason: e.reason.clone() }
}

fn error_specs(f: &facts::Facts) -> Vec<ErrorSpec> {
    f.errors.iter().map(error_spec).collect()
}

/// `n.via("template", ...)`: the template is the first string argument, else the callee's name.
fn template_of(via: &ir::Call) -> String {
    match via.args.first().and_then(|a| lit_text(&a.value)) {
        Some(s) => s.to_string(),
        None => callee_name(via).to_string(),
    }
}

/// The places of a result that hold encrypted values, each once, in the order the selections met them.
fn decrypt_paths(found: Vec<DecryptPath>) -> Vec<DecryptPath> {
    let mut out: Vec<DecryptPath> = Vec::new();
    for p in found {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

fn param_name(e: &Expr) -> Option<&str> {
    match &e.node {
        Node::Param { name } | Node::Local { name } => Some(name),
        _ => None,
    }
}

/// An expression that reads the binding `name` (what a shorthand `{ name }` lowers to).
fn bound(name: &str, ty: &Type) -> Expr {
    Expr { ty: ty.clone(), node: Node::Local { name: name.to_string() } }
}

impl<'x> Planner<'x> {
    pub fn new(core: &'x ir::Program, s: &'x Schema) -> Self {
        Planner {
            core,
            s,
            published: None,
            errors: Vec::new(),
            at: String::new(),
            pending_uploads: BTreeMap::new(),
            validations: Vec::new(),
            encrypts: Vec::new(),
            enc_seq: 0,
            emits: BTreeSet::new(),
        }
    }

    /// Queries are compiled against `published` unless they are `drafts` queries.
    pub fn with_published(mut self, published: &'x Schema) -> Self {
        self.published = self.s.tables.values().any(|t| t.published).then_some(published);
        self
    }

    fn fail(&mut self, msg: impl Into<String>) {
        self.fail_code(codes::E601, msg);
    }

    fn fail_code(&mut self, code: &str, msg: impl Into<String>) {
        self.errors.push(Diagnostic {
            severity: Severity::Error,
            code: code.into(),
            message: msg.into(),
            path: self.at.clone(),
            line: 0,
            col: 0,
        });
    }

    /// Reports at the intent's name rather than at its keyword.
    fn fail_at_name(&mut self, msg: &str) {
        let named = format!("{}.name", self.at);
        let decl = std::mem::replace(&mut self.at, named);
        self.fail(msg);
        self.at = decl;
    }

    fn compiler(&self) -> Compiler<'x> {
        Compiler::new(self.core, self.s, &self.at)
    }

    fn absorb(&mut self, c: &mut Compiler<'_>) {
        self.errors.append(&mut c.errors);
    }

    fn actor_entity(&self) -> Option<&str> {
        self.core.actor.as_ref().map(|a| a.entity.as_str())
    }

    // ---------- parameters ----------

    fn param_val(&self, name: &str, ty: &Ty) -> Val {
        match ty {
            Ty::Entity(e) => Val::Id { entity: e.clone(), sql: typed_marker(name, &Ty::Uuid) },
            Ty::Coll(inner) => match inner.as_ref() {
                Ty::Entity(e) => {
                    Val::IdSet { entity: e.clone(), sql: format!("SELECT (jsonb_array_elements_text(({}::text)::jsonb))::uuid", marker(name)) }
                }
                other => Val::JsonArray { sql: format!("({}::text)::jsonb", marker(name)), elem: other.clone() },
            },
            Ty::Record(r) => Val::Json { sql: format!("({}::text)::jsonb", marker(name)), record: Some(r.clone()) },
            Ty::Json | Ty::Union(_) => Val::Json { sql: format!("({}::text)::jsonb", marker(name)), record: None },
            Ty::Upload | Ty::Object => Val::Scalar { sql: format!("({}::text)", marker(name)), ty: Ty::Object },
            other => Val::Scalar { sql: typed_marker(name, other), ty: other.clone() },
        }
    }

    fn params(&mut self, c: &mut Compiler<'_>, params: &[ir::Param]) -> Vec<ParamSpec> {
        let mut out = Vec::new();
        for p in params {
            let ty = Ty::from_ir(&p.ty);
            c.bind(&p.name, self.param_val(&p.name, &ty));
            let default = p.default.as_ref().and_then(|d| match &d.node {
                Node::Lit { lit: Literal::Int(n) } => Some(serde_json::json!(n)),
                Node::Lit { lit: Literal::Text(s) } => Some(serde_json::json!(s)),
                Node::Lit { lit: Literal::Bool(b) } => Some(serde_json::json!(b)),
                Node::EnumValue { value, .. } => Some(serde_json::json!(value)),
                _ => None,
            });
            out.push(ParamSpec { name: p.name.clone(), ty: type_spec(&p.ty), optional: p.optional || p.default.is_some(), default });
        }
        out
    }

    /// Loads entity parameters in global table order: visibility applies (so an
    /// invisible row is NOT_FOUND) and rows are locked for the transaction.
    fn loads(&mut self, c: &mut Compiler<'_>, params: &[ir::Param], written: &BTreeSet<String>, lock: bool) -> Vec<Step> {
        let mut loads: Vec<(usize, Step)> = Vec::new();
        for p in params {
            let ty = Ty::from_ir(&p.ty);
            let (entity, many) = match &ty {
                Ty::Entity(e) => (e.clone(), false),
                Ty::Coll(inner) => match inner.as_ref() {
                    Ty::Entity(e) => (e.clone(), true),
                    _ => continue,
                },
                _ => continue,
            };
            let t = self.s.table(&entity).clone();
            let alias = c.fresh_alias();
            let row = Val::Row { entity: entity.clone(), alias: alias.clone() };
            let mut conds = vec![if many {
                format!("{alias}.\"id\" IN (SELECT (jsonb_array_elements_text(({}::text)::jsonb))::uuid)", marker(&p.name))
            } else {
                format!("{alias}.\"id\" = {}", typed_marker(&p.name, &Ty::Uuid))
            }];
            if t.soft_delete {
                conds.push(format!("{alias}.\"deleted_at\" IS NULL"));
            }
            if let Some(v) = c.visibility_sql(&entity, &row) {
                conds.push(v);
            }
            let lock_clause = if !lock {
                String::new()
            } else if written.contains(&p.name) {
                format!(" FOR UPDATE OF {alias}")
            } else {
                format!(" FOR SHARE OF {alias}")
            };
            let text =
                format!("SELECT {alias}.\"id\" FROM {} {alias} WHERE {} ORDER BY {alias}.\"id\"{lock_clause}", q(&t.table), conds.join(" AND "));
            loads.push((t.rank, Step::Load { param: p.name.clone(), entity, many, lock: lock && written.contains(&p.name), sql: sql(text) }));
        }
        loads.sort_by_key(|(r, _)| *r);
        loads.into_iter().map(|(_, s)| s).collect()
    }

    fn allow_step(&mut self, c: &mut Compiler<'_>, allow: &ir::Policy) -> Step {
        let cond = c.pred(&allow.cond);
        let full = match c.superuser_sql() {
            Some(su) => format!("({cond} OR {su})"),
            None => cond,
        };
        Step::Check { kind: CheckKind::Allow, sql: sql(format!("SELECT {full}")), code: allow.code.clone() }
    }

    fn let_step(&mut self, c: &mut Compiler<'_>, l: &ir::Let) -> Step {
        let v = c.expr(&l.value);
        let ty = Ty::from_ir(&l.value.ty);
        let value_sql = c.scalar(&v);
        let bound = match (&v, &ty) {
            (Val::Id { entity, .. } | Val::Row { entity, .. }, _) => Val::Id { entity: entity.clone(), sql: typed_marker(&l.name, &Ty::Uuid) },
            (Val::Json { record, .. }, _) => Val::Json { sql: format!("({}::text)::jsonb", marker(&l.name)), record: record.clone() },
            (_, t) => Val::Scalar { sql: typed_marker(&l.name, t), ty: t.clone() },
        };
        c.bind(&l.name, bound);
        Step::Let { name: l.name.clone(), sql: sql(format!("SELECT ({value_sql})::text")), code: l.code.clone() }
    }

    // ---------- tenants ----------

    /// Sets the tenant a call's reads are filtered to and returns the check that its entity
    /// parameters share one tenant. The tenant is the first required parameter's; the others are
    /// compared with it, so filtering by one is filtering by all.
    ///
    /// With `pin` (a call that writes), the steps end by fixing the transaction to that tenant for
    /// the database's write trigger, or, for `cross tenant`, by switching the trigger off.
    fn tenant_steps(&mut self, c: &mut Compiler<'_>, params: &[ir::Param], cross: bool, pin: bool) -> Vec<Step> {
        c.tenant = None;
        let active = tenant::active(self.core);
        if cross {
            return if pin && active { vec![tn::pin_step(None, true)] } else { Vec::new() };
        }
        let checked = tenant::checked_params(self.core, params);
        let ids = |p: &ir::Param| -> (String, String, bool) {
            let entity = ir::facts::entity_of_type(&p.ty).unwrap_or_default();
            if matches!(p.ty, Type::Ref { .. }) {
                (entity, typed_marker(&p.name, &Ty::Uuid), false)
            } else {
                (entity, format!("SELECT (jsonb_array_elements_text(({}::text)::jsonb))::uuid", marker(&p.name)), true)
            }
        };
        if let Some(a) = tenant::anchor(&checked) {
            let (entity, id, many) = ids(a);
            if let Some(sel) = tn::rows_select(self.core, self.s, &entity, &id, many) {
                c.tenant = Some(TenantScope { sql: format!("({sel} LIMIT 1)"), checked: checked.iter().map(|p| (*p).clone()).collect() });
            }
        }
        let mut steps = Vec::new();
        if tenant::needs_check(&checked) {
            let branches: Vec<String> = checked
                .iter()
                .filter_map(|p| {
                    let (entity, id, many) = ids(p);
                    tn::rows_select(self.core, self.s, &entity, &id, many)
                })
                .collect();
            let text = format!("SELECT (SELECT count(DISTINCT x.t) FROM ({}) x) <= 1", branches.join(" UNION ALL "));
            steps.push(Step::Check { kind: CheckKind::Tenant, sql: sql(text), code: None });
        }
        if pin
            && active
            && let Some(ts) = &c.tenant
        {
            steps.push(tn::pin_step(Some(&ts.sql), false));
        }
        steps
    }

    /// The step that starts an execution context sharing a transaction with others (a handler, a
    /// schedule item, a rule row, a job item): it forgets what the previous one pinned and takes
    /// the tenant the context's anchor names, if any. `None` when the program has no tenants.
    fn context_pin(&self, c: &Compiler<'_>, cross: bool) -> Option<Step> {
        tenant::active(self.core).then(|| tn::pin_step(c.tenant.as_ref().map(|t| t.sql.as_str()), cross))
    }

    /// A write that sets the reference a tenant path starts with must land in the call's own tenant;
    /// the database only checks the references of the row against each other.
    fn tenant_write_guard(&mut self, c: &Compiler<'_>, entity: &str, field: &str, value_sql: &str) -> Option<Step> {
        let ts = c.tenant.as_ref()?;
        let hops = tenant::path(self.core, entity).ok()?;
        let first = hops.first()?;
        if first.field != field {
            return None;
        }
        let new_tenant = tn::id_sql(self.core, self.s, &first.target, value_sql)?;
        Some(Step::Check { kind: CheckKind::Tenant, sql: sql(format!("SELECT {new_tenant} IS NOT DISTINCT FROM {}", ts.sql)), code: None })
    }

    // ---------- commands ----------

    pub fn command(&mut self, name: &str, cd: &ir::Command) -> CommandPlan {
        self.emits.clear();
        let mut c = self.compiler();
        c.push();
        let params = self.params(&mut c, &cd.params);
        let written = facts::written_roots(&cd.body);
        // reading out or removing a person's data is not for someone who only acts as that person
        let mut steps: Vec<Step> =
            if form_intents::touches_personal_data(&cd.body) { self.not_impersonating().into_iter().collect() } else { Vec::new() };
        steps.extend(self.loads(&mut c, &cd.params, &written, true));
        steps.extend(self.tenant_steps(&mut c, &cd.params, cd.cross_tenant, true));
        for l in &cd.lets {
            let s = self.let_step(&mut c, l);
            steps.push(s);
        }
        match &cd.allow {
            Some(al) => {
                let s = self.allow_step(&mut c, al);
                steps.push(s);
            }
            None => self.fail_at_name("command without allow"),
        }
        steps.extend(self.consent_steps(name));
        let (version_params, version_checks) = self.version_checks(cd);
        let mut params = params;
        params.extend(version_params);
        steps.extend(version_checks);
        for r in &cd.requires {
            let cond = c.pred(&r.cond);
            let text = match &r.when {
                Some(w) => {
                    let wc = c.pred(w);
                    format!("SELECT (NOT {wc}) OR {cond}")
                }
                None => format!("SELECT {cond}"),
            };
            steps.push(Step::Check { kind: CheckKind::Require, sql: sql(text), code: Some(r.code.clone()) });
        }
        self.stmts(&mut c, &cd.body, &mut steps);
        for e in &cd.emits {
            let s = self.emit(&mut c, e);
            steps.push(s);
        }
        let returns = cd.returns.as_ref().map(|r| {
            let v = c.expr(&r.value);
            let text = match (&r.select, &v) {
                (Some(sel), Val::Id { entity, sql }) => format!("SELECT {}", select::nested(&mut c, entity, sql, sel)),
                (_, Val::Scalar { sql: s, ty: ty @ (Ty::Decimal | Ty::Money(_)) }) => format!("SELECT {}", select::scalar_json(s, ty)),
                (_, other) => {
                    let s = c.scalar(other);
                    format!("SELECT to_jsonb({s})")
                }
            };
            sql(text)
        });
        let idempotency_key = match &cd.idempotency {
            ir::Idempotency::Derived { key } => {
                let s = c.sql(key);
                Some(sql(format!("SELECT ({s})::text")))
            }
            _ => None,
        };
        c.pop();
        let decrypt = decrypt_paths(std::mem::take(&mut c.enc_paths));
        self.absorb(&mut c);
        let idempotent = !matches!(cd.idempotency, ir::Idempotency::None);
        let facts = facts::command_facts(self.core, name, cd);
        CommandPlan {
            name: name.to_string(),
            params,
            internal: cd.internal,
            idempotent,
            idempotency_key,
            audited: cd.audited,
            steps,
            returns,
            rate_limits: rates(&cd.rate_limits),
            errors: error_specs(&facts),
            effects: facts.effects.iter().cloned().collect(),
            writes: facts.writes.iter().cloned().collect(),
            emits: self.emits.iter().cloned().collect(),
            output: crate::shape::to_json(&cd.output),
            decrypt,
        }
    }

    fn emit(&mut self, c: &mut Compiler<'_>, e: &ir::Emit) -> Step {
        let mut pairs = Vec::new();
        for (name, value) in &e.fields {
            let v = c.expr(value);
            let s = match &v {
                Val::Scalar { sql, ty: Ty::Decimal | Ty::Money(_) } => format!("(({sql})::text)"),
                _ => c.scalar(&v),
            };
            pairs.push(format!("{}, {s}", lit(name)));
        }
        self.emits.insert(e.event.clone());
        let obj = if pairs.is_empty() { "'{}'::jsonb".into() } else { format!("jsonb_build_object({})", pairs.join(", ")) };
        Step::Emit {
            event: e.event.clone(),
            payload: sql(format!("SELECT {obj}")),
            broker: e.to.as_ref().map(|t| (t.broker.clone(), t.topic.clone())),
        }
    }

    pub fn stmts(&mut self, c: &mut Compiler<'_>, stmts: &[Stmt], out: &mut Vec<Step>) {
        // `insert X {...} as b` followed by `s3.put(u, ...) into b.f`: stage first, insert with the key
        for s in stmts {
            if let Stmt::Effect { call, into: Some(Expr { node: Node::Field { base, field: f, .. }, .. }), .. } = s
                && let (Some(b), "s3.put") = (param_name(base), callee_name(call))
            {
                let upload = call.args.first().and_then(|a| param_name(&a.value)).map(String::from);
                let bucket = call
                    .args
                    .iter()
                    .find(|a| a.name.as_deref() == Some("bucket"))
                    .and_then(|a| lit_text(&a.value))
                    .map(String::from)
                    .unwrap_or_else(|| "default".into());
                let is_insert_bind = stmts.iter().any(|x| matches!(x, Stmt::Insert { bind: Some(ib), .. } if ib == b));
                if let (Some(u), true) = (upload, is_insert_bind) {
                    self.pending_uploads.insert((b.to_string(), f.clone()), (u, bucket));
                }
            }
        }
        for s in stmts {
            self.stmt(c, s, out);
        }
    }

    fn block(&mut self, c: &mut Compiler<'_>, b: &[Stmt]) -> Vec<Step> {
        let mut out = Vec::new();
        c.push();
        self.stmts(c, b, &mut out);
        c.pop();
        out
    }

    fn column_values(&mut self, c: &mut Compiler<'_>, entity: &str, fields: &[(String, Expr)]) -> Vec<(String, String)> {
        let t = self.s.table(entity).clone();
        let mut cols: Vec<(String, String)> = Vec::new();
        let set_col = |cols: &mut Vec<(String, String)>, col: String, v: String| {
            cols.retain(|(c0, _)| *c0 != col);
            cols.push((col, v));
        };
        for (name, value) in fields {
            // a member of a record value (`...input` spells out one of these per record field) is read straight from its JSON
            let col = t.col(name).cloned();
            let mut member: Option<(String, String)> = None;
            let v = match &value.node {
                Node::Field { base, entity: None, field } if matches!(Ty::from_ir(&base.ty), Ty::Record(ref r) if !r.starts_with("event:")) => {
                    match c.expr(base) {
                        Val::Json { sql: js, record: Some(r) } if matches!(col, Some(Col::Scalar { .. } | Col::Ref { .. })) => {
                            member = Some((js.clone(), field.clone()));
                            Val::Json { sql: js, record: Some(r) }
                        }
                        other => c.field(other, field),
                    }
                }
                _ => c.expr(value),
            };
            if let Some(Col::Scalar { col: ccol, .. }) = &col
                && c.is_encrypted(entity, name)
            {
                // the value is encrypted by the runtime, for the row's id, just before the statement runs
                let source = match (&member, &v) {
                    (Some((js, f)), _) => lone_marker(js).map(|n| (n, Some(f.clone()))),
                    (_, Val::Scalar { sql, .. }) => lone_marker(sql).map(|n| (n, None)),
                    _ => None,
                };
                match source {
                    Some((source, member)) => {
                        self.enc_seq += 1;
                        let bind = format!("__enc.{}", self.enc_seq);
                        let value = format!("({}::text)", marker(&bind));
                        self.encrypts.push(Step::Encrypt {
                            field: format!("{entity}.{name}"),
                            source,
                            member,
                            bind,
                            row: EncryptRow::New { id: String::new() },
                        });
                        set_col(&mut cols, ccol.clone(), value);
                    }
                    None => self.fail_code(codes::E320, format!("{entity}.{name} is encrypted: it can only be written from a parameter")),
                }
                continue;
            }
            match col {
                Some(Col::Scalar { col, ty }) => {
                    let s = match (&member, &v, &ty) {
                        (Some((js, f)), _, _) => match &ty {
                            Ty::Coll(_) | Ty::Json | Ty::Record(_) => format!("({js} -> {})", lit(f)),
                            Ty::Range(_) => {
                                format!("tstzrange((({js} -> {}) ->> 'start')::timestamptz, (({js} -> {}) ->> 'end')::timestamptz)", lit(f), lit(f))
                            }
                            other => format!("(({js} ->> {}))::{}", lit(f), schema::cast(other)),
                        },
                        (_, Val::JsonArray { sql, .. }, _) => sql.clone(),
                        _ => {
                            let s = c.scalar(&v);
                            format!("({s})::{}", schema::cast(&ty))
                        }
                    };
                    set_col(&mut cols, col, s);
                }
                Some(Col::Ref { col, .. }) => {
                    let s = match &member {
                        Some((js, f)) => format!("(({js} ->> {}))::uuid", lit(f)),
                        None => {
                            let s = c.scalar(&v);
                            format!("({s})::uuid")
                        }
                    };
                    if let Some(g) = self.tenant_write_guard(c, entity, name, &s) {
                        self.validations.push(g);
                    }
                    set_col(&mut cols, col, s);
                }
                Some(Col::Union { type_col, id_col, .. }) => match &v {
                    Val::Id { entity: e, sql } => {
                        set_col(&mut cols, type_col, lit(e));
                        set_col(&mut cols, id_col, sql.clone());
                    }
                    Val::Json { sql, .. } => {
                        set_col(&mut cols, type_col, format!("({sql} ->> 'type')"));
                        set_col(&mut cols, id_col, format!("(({sql} ->> 'id'))::uuid"));
                    }
                    _ => self.fail("union reference needs {type, id} input or a typed row"),
                },
                Some(Col::Snapshot { id_col, ver_col, .. }) => match &v {
                    Val::Snap { id, version, .. } => {
                        set_col(&mut cols, id_col, id.clone());
                        set_col(&mut cols, ver_col, version.clone());
                    }
                    _ => self.fail("snapshot field needs snapshot(...) or a snapshot value"),
                },
                _ => self.fail(format!("cannot write {entity}.{name}")),
            }
        }
        self.dynamic_validations(c, entity, fields);
        let info = &self.core.entities[entity];
        if info.traits.track_by_actor
            && let Some(Col::Ref { col, .. }) = t.col("createdBy")
        {
            set_col(&mut cols, col.clone(), typed_marker(ACTOR, &Ty::Uuid));
        }
        cols
    }

    fn lifecycle_guard(
        &mut self,
        c: &mut Compiler<'_>,
        entity: &str,
        field: &str,
        new_value: &str,
        rows_from: &str,
        rows_where: &str,
        alias: &str,
    ) -> Option<Step> {
        let info = self.core.entities.get(entity)?;
        let lc = info.lifecycles.iter().find(|l| l.field == field)?;
        let col = match self.s.table(entity).col(field) {
            Some(Col::Scalar { col, .. }) => col.clone(),
            _ => return None,
        };
        let mut sources: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for t in &lc.transitions {
            for to in &t.to {
                sources.entry(to.clone()).or_default().extend(t.from.iter().cloned());
            }
        }
        let cases: Vec<String> = sources
            .iter()
            .map(|(to, from)| {
                format!("WHEN {} THEN {alias}.{} IN ({})", lit(to), q(&col), from.iter().map(|f| lit(f)).collect::<Vec<_>>().join(", "))
            })
            .collect();
        // only declared transitions are allowed; staying in the same state must be declared too
        let ok = format!("(CASE ({new_value})::text {} ELSE false END)", cases.join(" "));
        let code = format!("{}_{}_INVALID_TRANSITION", snake(entity).to_uppercase(), snake(field).to_uppercase());
        let _ = c;
        let w = if rows_where.is_empty() { format!(" WHERE NOT {ok}") } else { format!("{rows_where} AND NOT {ok}") };
        Some(Step::Check { kind: CheckKind::Transition, sql: sql(format!("SELECT NOT EXISTS (SELECT 1 FROM {rows_from}{w})")), code: Some(code) })
    }

    fn stmt(&mut self, c: &mut Compiler<'_>, s: &Stmt, out: &mut Vec<Step>) {
        match s {
            Stmt::Let(l) => {
                let st = self.let_step(c, l);
                out.push(st);
            }
            Stmt::Insert { entity, from, values, bind } => {
                let t = self.s.table(entity).clone();
                let mut cols;
                let text = match from {
                    None => {
                        cols = self.column_values(c, entity, values);
                        if let Some(b) = bind {
                            let pend: Vec<(String, (String, String))> =
                                self.pending_uploads.iter().filter(|((pb, _), _)| pb == b).map(|((_, f), v)| (f.clone(), v.clone())).collect();
                            for (field, (upload, bucket)) in pend {
                                let key = format!("__obj.{b}.{field}");
                                out.push(Step::Upload { param: upload, bucket, bind: Some(key.clone()), target: None });
                                if let Some(Col::Scalar { col, .. }) = t.col(&field) {
                                    cols.push((col.clone(), format!("({}::text)", marker(&key))));
                                }
                            }
                        }
                        out.append(&mut self.validations);
                        // the ciphertext is bound to the id of the row it is in, so the row gets its id here and not from the default
                        let mut encrypts = std::mem::take(&mut self.encrypts);
                        if !encrypts.is_empty() {
                            self.enc_seq += 1;
                            let id = format!("__id.{}", self.enc_seq);
                            for st in &mut encrypts {
                                if let Step::Encrypt { row, .. } = st {
                                    *row = EncryptRow::New { id: id.clone() };
                                }
                            }
                            cols.push(("id".to_string(), typed_marker(&id, &Ty::Uuid)));
                            out.push(Step::NewId { name: id });
                            out.extend(encrypts);
                        }
                        let (names, vals): (Vec<String>, Vec<String>) = cols.iter().map(|(a, b)| (q(a), b.clone())).unzip();
                        if names.is_empty() {
                            format!("INSERT INTO {} DEFAULT VALUES RETURNING \"id\"", q(&t.table))
                        } else {
                            format!("INSERT INTO {} ({}) VALUES ({}) RETURNING \"id\"", q(&t.table), names.join(", "), vals.join(", "))
                        }
                    }
                    Some(se) => {
                        let parts = c.begin_set(se);
                        cols = self.column_values(c, entity, values);
                        c.end_set();
                        // per-row dynamic validation is not available for set inserts
                        self.validations.clear();
                        if !self.encrypts.is_empty() {
                            self.encrypts.clear();
                            self.fail_code(codes::E320, format!("insert {entity} from a set writes many rows, and an encrypted value is encrypted for one row"));
                        }
                        let (names, vals): (Vec<String>, Vec<String>) = cols.iter().map(|(a, b)| (q(a), b.clone())).unzip();
                        format!(
                            "INSERT INTO {} ({}) SELECT {} FROM {}{}",
                            q(&t.table),
                            names.join(", "),
                            vals.join(", "),
                            parts.from,
                            Compiler::where_sql(&parts.conds)
                        )
                    }
                };
                out.push(Step::Exec { sql: sql(text), bind: bind.clone(), label: format!("insert {entity}") });
                if let Some(b) = bind {
                    c.bind(b, Val::Id { entity: entity.clone(), sql: typed_marker(b, &Ty::Uuid) });
                }
            }
            Stmt::Upsert { entity, keys, values, bind } => {
                let t = self.s.table(entity).clone();
                let cols = self.column_values(c, entity, values);
                if !self.encrypts.is_empty() {
                    // on a conflict the existing row keeps its id, which the new ciphertext would not be bound to
                    self.encrypts.clear();
                    self.fail_code(codes::E320, format!("upsert {entity} cannot write an encrypted field"));
                }
                out.append(&mut self.validations);
                let key_cols: Vec<String> = keys
                    .iter()
                    .filter_map(|k| match t.col(k) {
                        Some(Col::Scalar { col, .. }) | Some(Col::Ref { col, .. }) => Some(col.clone()),
                        _ => None,
                    })
                    .collect();
                let updates: Vec<String> =
                    cols.iter().filter(|(c0, _)| !key_cols.contains(c0)).map(|(c0, _)| format!("{} = EXCLUDED.{}", q(c0), q(c0))).collect();
                let (names, vals): (Vec<String>, Vec<String>) = cols.iter().map(|(a, b)| (q(a), b.clone())).unzip();
                let conflict = if updates.is_empty() {
                    "DO UPDATE SET \"id\" = EXCLUDED.\"id\"".to_string()
                } else {
                    format!("DO UPDATE SET {}", updates.join(", "))
                };
                let text = format!(
                    "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) {conflict} RETURNING \"id\"",
                    q(&t.table),
                    names.join(", "),
                    vals.join(", "),
                    key_cols.iter().map(|k| q(k)).collect::<Vec<_>>().join(", ")
                );
                out.push(Step::Exec { sql: sql(text), bind: bind.clone(), label: format!("upsert {entity}") });
                if let Some(b) = bind {
                    c.bind(b, Val::Id { entity: entity.clone(), sql: typed_marker(b, &Ty::Uuid) });
                }
            }
            Stmt::Update { target, via, assigns } => {
                let parts = c.begin_set(target);
                let SetElem::Entity(entity) = parts.elem.clone() else {
                    c.end_set();
                    return self.fail("update target must be entity rows");
                };
                let mut sets = Vec::new();
                let mut guards = Vec::new();
                let where_sql = Compiler::where_sql(&parts.conds);
                if let Some(ir::UpdateVia { path, alias }) = via {
                    // aggregate joined rows per target row
                    let vv = c.expr(path);
                    let (child, via_col, parent) = match vv {
                        Val::Inverse { entity: ch, via_col, parent } => (ch, via_col, parent),
                        _ => {
                            c.end_set();
                            return self.fail("'via' needs a one-to-many relation path");
                        }
                    };
                    let key_col = self.s.table(&child).cols.values().find_map(|col| match col {
                        Col::Ref { col, target } if *target == entity => Some(col.clone()),
                        _ => None,
                    });
                    let Some(key_col) = key_col else {
                        c.end_set();
                        return self.fail(format!("{child} has no reference to {entity}"));
                    };
                    let ia = c.fresh_alias();
                    c.push();
                    c.bind(alias, Val::Row { entity: child.clone(), alias: ia.clone() });
                    c.via_alias = Some(alias.clone());
                    let mut aggs = Vec::new();
                    for (i, asg) in assigns.iter().enumerate() {
                        let col = self.assign_col(&entity, &asg.target);
                        if let Some(f) = target_field(&asg.target).filter(|f| c.is_encrypted(&entity, f)) {
                            self.fail_code(codes::E320, format!("{entity}.{f} is encrypted: an update over many rows cannot encrypt for each row"));
                        }
                        let v = c.sql(&asg.value);
                        aggs.push(format!("{v} AS v{i}"));
                        let cur = format!("{}.{}", parts.alias, q(&col));
                        sets.push(match asg.op {
                            AssignOp::Set => format!("{} = agg.v{i}", q(&col)),
                            AssignOp::Add => format!("{} = {cur} + agg.v{i}", q(&col)),
                            AssignOp::Sub => format!("{} = {cur} - agg.v{i}", q(&col)),
                        });
                    }
                    c.via_alias = None;
                    c.pop();
                    let ct = q(&self.s.table(&child).table);
                    let text = format!(
                        "UPDATE {} SET {} FROM (SELECT {ia}.{} AS __k, {} FROM {ct} {ia} WHERE {ia}.{} = {parent} GROUP BY {ia}.{}) agg{} {} {}.\"id\" = agg.__k",
                        parts.from,
                        sets.join(", "),
                        q(&key_col),
                        aggs.join(", "),
                        q(&via_col),
                        q(&key_col),
                        where_sql,
                        if where_sql.is_empty() { "WHERE" } else { "AND" },
                        parts.alias
                    );
                    c.end_set();
                    out.push(Step::Exec { sql: sql(text), bind: None, label: format!("update {entity} via") });
                    return;
                }
                for asg in assigns {
                    let col = self.assign_col(&entity, &asg.target);
                    if let Some(f) = target_field(&asg.target).filter(|f| c.is_encrypted(&entity, f)) {
                        self.fail_code(codes::E320, format!("{entity}.{f} is encrypted: an update over many rows cannot encrypt for each row"));
                    }
                    let v = c.sql(&asg.value);
                    let cur = format!("{}.{}", parts.alias, q(&col));
                    let field = target_field(&asg.target).unwrap_or_default();
                    if let Some(g) = self.lifecycle_guard(c, &entity, &field, &v, &parts.from, &where_sql, &parts.alias) {
                        guards.push(g);
                    }
                    if let Some(g) = self.tenant_write_guard(c, &entity, &field, &format!("({v})::uuid")) {
                        guards.push(g);
                    }
                    sets.push(match asg.op {
                        AssignOp::Set => format!("{} = {v}", q(&col)),
                        AssignOp::Add => format!("{} = {cur} + {v}", q(&col)),
                        AssignOp::Sub => format!("{} = {cur} - {v}", q(&col)),
                    });
                }
                c.end_set();
                out.extend(guards);
                out.push(Step::Exec {
                    sql: sql(format!("UPDATE {} SET {}{where_sql}", parts.from, sets.join(", "))),
                    bind: None,
                    label: format!("update {entity}"),
                });
            }
            Stmt::Delete { target } | Stmt::Purge { target } => {
                let purge = matches!(s, Stmt::Purge { .. });
                if let SetSource::Expr { expr } = &target.source
                    && let Ty::Union(alts) = Ty::from_ir(&expr.ty)
                {
                    // `delete report.target`: one statement per alternative, guarded by the type column
                    let (Node::Field { base, field: f, .. }, None, None) = (&expr.node, &target.alias, &target.filter) else {
                        return self.fail("union delete needs 'delete row.field'");
                    };
                    let bv = c.expr(base);
                    let (bent, bid) = match bv {
                        Val::Id { entity, sql } => (entity, sql),
                        Val::Row { entity, alias } => (entity, format!("{alias}.\"id\"")),
                        _ => return self.fail("union delete base must be a row"),
                    };
                    let Some(Col::Union { type_col, id_col, .. }) = self.s.table(&bent).col(f).cloned() else {
                        return self.fail("not a union field");
                    };
                    let bt = q(&self.s.table(&bent).table);
                    for alt in alts {
                        let at = self.s.table(&alt).clone();
                        let pick = format!("(SELECT b.{} FROM {bt} b WHERE b.\"id\" = {bid} AND b.{} = {})", q(&id_col), q(&type_col), lit(&alt));
                        let text = if at.soft_delete && !purge {
                            format!("UPDATE {} SET \"deleted_at\" = now() WHERE \"id\" = {pick}", q(&at.table))
                        } else {
                            format!("DELETE FROM {} WHERE \"id\" = {pick}", q(&at.table))
                        };
                        out.push(Step::Exec { sql: sql(text), bind: None, label: format!("delete {alt}") });
                    }
                    return;
                }
                let parts = c.begin_set(target);
                c.end_set();
                let SetElem::Entity(entity) = parts.elem.clone() else {
                    return self.fail("delete target must be entity rows");
                };
                let soft = self.s.table(&entity).soft_delete && !purge;
                let w = Compiler::where_sql(&parts.conds);
                let text =
                    if soft { format!("UPDATE {} SET \"deleted_at\" = now(){w}", parts.from) } else { format!("DELETE FROM {}{w}", parts.from) };
                out.push(Step::Exec { sql: sql(text), bind: None, label: format!("delete {entity}") });
            }
            Stmt::Erase { target } => self.erase(c, target, out),
            Stmt::Toggle { entity, values } => {
                let t = self.s.table(entity).clone();
                let cols = self.column_values(c, entity, values);
                if !self.encrypts.is_empty() {
                    self.encrypts.clear();
                    self.fail_code(codes::E320, format!("toggle {entity} cannot write an encrypted field"));
                }
                out.append(&mut self.validations);
                let conds: Vec<String> = cols.iter().map(|(col, v)| format!("x.{} = {v}", q(col))).collect();
                let (names, vals): (Vec<String>, Vec<String>) = cols.iter().map(|(a, b)| (q(a), b.clone())).unzip();
                let text = format!(
                    "WITH d AS (DELETE FROM {t} x WHERE {} RETURNING 1) INSERT INTO {t} ({}) SELECT {} WHERE NOT EXISTS (SELECT 1 FROM d) ON CONFLICT DO NOTHING",
                    conds.join(" AND "),
                    names.join(", "),
                    vals.join(", "),
                    t = q(&t.table)
                );
                out.push(Step::Exec { sql: sql(text), bind: None, label: format!("toggle {entity}") });
            }
            Stmt::Set { assigns } => {
                // group consecutive assignments to the same row into one UPDATE
                let mut i = 0;
                while i < assigns.len() {
                    let Node::Field { base, .. } = &assigns[i].target.node else {
                        self.fail("set target must be row.field");
                        i += 1;
                        continue;
                    };
                    let bv = c.expr(base);
                    let (entity, id) = match &bv {
                        Val::Id { entity, sql } => (entity.clone(), sql.clone()),
                        Val::Row { entity, alias } => (entity.clone(), format!("{alias}.\"id\"")),
                        _ => {
                            self.fail("set target must be a row");
                            i += 1;
                            continue;
                        }
                    };
                    let mut j = i;
                    let mut sets = Vec::new();
                    let t = self.s.table(&entity).clone();
                    while j < assigns.len() {
                        let Node::Field { base: b2, field: f, .. } = &assigns[j].target.node else { break };
                        if b2 != base {
                            break;
                        }
                        let col = self.assign_col(&entity, &assigns[j].target);
                        let v = c.sql(&assigns[j].value);
                        if c.is_encrypted(&entity, f) {
                            // the ciphertext is bound to this row's id, so it is made here, for the id the statement updates
                            let source = lone_marker(&v).filter(|_| matches!(assigns[j].op, AssignOp::Set) && !matches!(bv, Val::Row { .. }));
                            match source {
                                Some(source) => {
                                    self.enc_seq += 1;
                                    let bind = format!("__enc.{}", self.enc_seq);
                                    out.push(Step::Encrypt {
                                        field: format!("{entity}.{f}"),
                                        source,
                                        member: None,
                                        bind: bind.clone(),
                                        row: EncryptRow::Existing { id: sql(format!("SELECT ({id})::text")) },
                                    });
                                    sets.push(format!("{} = ({}::text)", q(&col), marker(&bind)));
                                }
                                None => self.fail_code(codes::E320, format!("{entity}.{f} is encrypted: write it with `set row.{f} = <parameter>`")),
                            }
                            j += 1;
                            continue;
                        }
                        if let Some(g) =
                            self.lifecycle_guard(c, &entity, f, &v, &format!("{} x", q(&t.table)), &format!(" WHERE x.\"id\" = {id}"), "x")
                        {
                            out.push(g);
                        }
                        if let Some(g) = self.tenant_write_guard(c, &entity, f, &format!("({v})::uuid")) {
                            out.push(g);
                        }
                        let cur = q(&col).to_string();
                        let val = match assigns[j].op {
                            AssignOp::Set => {
                                if matches!(Ty::from_ir(&assigns[j].value.ty), Ty::Coll(_)) {
                                    v
                                } else {
                                    let cty = match t.col(f) {
                                        Some(Col::Scalar { ty, .. }) => schema::cast(ty).to_string(),
                                        _ => "uuid".into(),
                                    };
                                    format!("({v})::{cty}")
                                }
                            }
                            AssignOp::Add => format!("{cur} + {v}"),
                            AssignOp::Sub => format!("{cur} - {v}"),
                        };
                        sets.push(format!("{} = {val}", q(&col)));
                        j += 1;
                    }
                    out.push(Step::Exec {
                        sql: sql(format!("UPDATE {} SET {} WHERE \"id\" = {id}", q(&t.table), sets.join(", "))),
                        bind: None,
                        label: format!("set {entity}"),
                    });
                    i = j.max(i + 1);
                }
            }
            Stmt::When { cond, body } => {
                let cs = if matches!(Ty::from_ir(&cond.ty), Ty::Bool) {
                    c.pred(cond)
                } else {
                    let v = c.sql(cond);
                    format!("({v}) IS NOT NULL")
                };
                let steps = self.block(c, body);
                out.push(Step::When { cond: sql(format!("SELECT {cs}")), steps });
            }
            Stmt::Each { source, body } => {
                let (SetSource::Expr { expr }, Some(item)) = (&source.source, &source.alias) else {
                    return self.fail("each needs 'each param item partial'");
                };
                let Some(src) = param_name(expr) else {
                    return self.fail("each needs 'each param item partial'");
                };
                let elem = match Ty::from_ir(&expr.ty) {
                    Ty::Coll(t) => *t,
                    other => other,
                };
                c.push();
                c.bind(item, self.param_val(item, &elem));
                let steps = self.block(c, body);
                c.pop();
                out.push(Step::EachPartial { source: src.to_string(), item: item.clone(), steps });
            }
            Stmt::Effect { call, bind, into, on_failure } => self.effect(c, call, bind.as_deref(), into.as_ref(), on_failure.as_deref(), out),
            Stmt::Reserve { .. } => self.fail("the inventory extension is not available yet"),
            Stmt::AtRun { at, intent, args } => {
                let when = c.sql(at);
                let mut pairs = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let v = c.sql(&a.value);
                    let key = a.name.clone().unwrap_or_else(|| format!("_{i}"));
                    pairs.push(format!("{}, {v}", lit(&key)));
                }
                let payload = format!(
                    "jsonb_build_object('intent', {}, 'at', {when}, 'actor', {}, 'args', jsonb_build_object({}))",
                    lit(intent),
                    typed_marker(ACTOR, &Ty::Uuid),
                    pairs.join(", ")
                );
                out.push(Step::Deferred { effect: "timer.run".into(), args: sql(format!("SELECT {payload}")), key: None, on_failure: Vec::new() });
            }
            Stmt::Notify(n) => {
                let st = self.notify(c, n);
                out.push(st);
            }
            Stmt::ExportPersonalData { of, .. } => {
                let v = c.sql(of);
                out.push(Step::Deferred {
                    effect: "export.personal".into(),
                    args: sql(format!("SELECT jsonb_build_object('subject', {v})")),
                    key: None,
                    on_failure: Vec::new(),
                });
            }
        }
    }

    fn assign_col(&mut self, entity: &str, target: &Expr) -> String {
        let f = target_field(target).unwrap_or_default();
        match self.s.table(entity).col(&f) {
            Some(Col::Scalar { col, .. }) | Some(Col::Ref { col, .. }) | Some(Col::Counter { col }) => col.clone(),
            _ => {
                self.fail(format!("cannot assign {entity}.{f}"));
                snake(&f)
            }
        }
    }

    fn effect(
        &mut self,
        c: &mut Compiler<'_>,
        call: &ir::Call,
        bind: Option<&str>,
        into: Option<&Expr>,
        on_failure: Option<&[Stmt]>,
        out: &mut Vec<Step>,
    ) {
        let name = callee_name(call).to_string();
        if name == "s3.put" {
            let Some(Expr { node: Node::Field { base, field: f, .. }, .. }) = into else {
                return self.fail("s3.put needs 'into row.field'");
            };
            if let Some(b) = param_name(base)
                && self.pending_uploads.remove(&(b.to_string(), f.clone())).is_some()
            {
                return; // staged together with the insert
            }
            let upload = match call.args.first().and_then(|a| param_name(&a.value)) {
                Some(n) => n.to_string(),
                None => return self.fail("s3.put(upload, bucket: \"...\")"),
            };
            let bucket = call
                .args
                .iter()
                .find(|a| a.name.as_deref() == Some("bucket"))
                .and_then(|a| lit_text(&a.value))
                .map(String::from)
                .unwrap_or_else(|| "default".into());
            let bv = c.expr(base);
            let (entity, id) = match bv {
                Val::Id { entity, sql } => (entity, sql),
                Val::Row { entity, alias } => (entity, format!("{alias}.\"id\"")),
                _ => return self.fail("into target must be a row field"),
            };
            let t = self.s.table(&entity).clone();
            let Some(Col::Scalar { col, .. }) = t.col(f).cloned() else {
                return self.fail("into target must be an s3.Object field");
            };
            let key = format!("__obj.{upload}");
            let old = format!("__old.{upload}");
            out.push(Step::Upload { param: upload.clone(), bucket, bind: Some(key.clone()), target: None });
            // the replaced object is released only after commit
            out.push(Step::Let { name: old.clone(), sql: sql(format!("SELECT {} FROM {} WHERE \"id\" = {id}", q(&col), q(&t.table))), code: None });
            out.push(Step::Exec {
                sql: sql(format!("UPDATE {} SET {} = ({}::text) WHERE \"id\" = {id}", q(&t.table), q(&col), marker(&key))),
                bind: None,
                label: format!("attach {entity}.{f}"),
            });
            out.push(Step::When {
                cond: sql(format!("SELECT ({}::text) IS NOT NULL", marker(&old))),
                steps: vec![Step::Deferred {
                    effect: "s3.delete".into(),
                    args: sql(format!("SELECT jsonb_build_object('key', ({}::text))", marker(&old))),
                    key: None,
                    on_failure: Vec::new(),
                }],
            });
            return;
        }
        let mut pairs = Vec::new();
        for (i, a) in call.args.iter().enumerate() {
            let v = c.json_sql(&a.value);
            let k = a.name.clone().unwrap_or_else(|| format!("_{i}"));
            pairs.push(format!("{}, {v}", lit(&k)));
        }
        let key = call.args.iter().find(|a| a.name.as_deref() == Some("key")).map(|a| {
            let v = c.sql(&a.value);
            sql(format!("SELECT ({v})::text"))
        });
        let fail_steps = on_failure.map(|b| self.block(c, b)).unwrap_or_default();
        let _ = bind;
        let args = if pairs.is_empty() { "'{}'::jsonb".into() } else { format!("jsonb_build_object({})", pairs.join(", ")) };
        out.push(Step::Deferred { effect: name, args: sql(format!("SELECT {args}")), key, on_failure: fail_steps });
    }

    fn notify(&mut self, c: &mut Compiler<'_>, n: &ir::Notify) -> Step {
        let parts = c.begin_set(&n.to);
        let actor = self.actor_entity().unwrap_or_default().to_string();
        let recip_col = match &parts.elem {
            SetElem::Entity(e) if *e == actor => "\"id\"".to_string(),
            SetElem::Entity(e) => self
                .s
                .table(e)
                .cols
                .values()
                .find_map(|col| match col {
                    Col::Ref { col, target } if *target == actor => Some(q(col)),
                    _ => None,
                })
                .unwrap_or_else(|| "\"id\"".into()),
            _ => "\"id\"".into(),
        };
        let mut pairs = Vec::new();
        for (name, value) in &n.fields {
            let v = c.json_sql(value);
            pairs.push(format!("{}, {v}", lit(name)));
        }
        c.end_set();
        let mut conds = parts.conds.clone();
        conds.push(format!("{}.{recip_col} IS NOT NULL", parts.alias));
        let recipients = format!(
            "SELECT coalesce(jsonb_agg(DISTINCT {}.{recip_col}), '[]'::jsonb) FROM {}{}",
            parts.alias,
            parts.from,
            Compiler::where_sql(&conds)
        );
        let template = template_of(&n.via);
        Step::Notify {
            recipients: sql(recipients),
            template,
            payload: sql(format!(
                "SELECT {}",
                if pairs.is_empty() { "'{}'::jsonb".into() } else { format!("jsonb_build_object({})", pairs.join(", ")) }
            )),
            category: n.category.clone(),
            digest_seconds: n.digest_seconds.map(|d| d.max(0) as u64),
        }
    }

    /// `erase x`: every reference to the personal row follows its `on erase`
    /// policy, broken cardinality groups run their repair, then the row goes.
    fn erase(&mut self, c: &mut Compiler<'_>, target: &Expr, out: &mut Vec<Step>) {
        let v = c.expr(target);
        let (pentity, id) = match v {
            Val::Id { entity, sql } => (entity, sql),
            _ => return self.fail("erase needs a row"),
        };
        let mut repairs: Vec<Step> = Vec::new();
        let core = self.core;
        for (ename, info) in &core.entities {
            if *ename == pentity {
                continue;
            }
            let t = self.s.table(ename).clone();
            for f in &info.fields {
                let Some(Col::Ref { col, target: tgt }) = t.col(&f.name).cloned() else { continue };
                if tgt != pentity {
                    continue;
                }
                let policy = match &f.kind {
                    ir::FieldKind::Ref { on_erase, .. } => on_erase.as_ref(),
                    _ => None,
                };
                // groups of cardinality constraints this row participates in, before we change anything
                for con in &info.constraints {
                    if let ir::Constraint::Cardinality { ordinal: i, filter, per, repair: Some(block), .. } = con {
                        let per_col = match t.col(per) {
                            Some(Col::Ref { col, .. }) | Some(Col::Scalar { col, .. }) => col.clone(),
                            _ => continue,
                        };
                        let mut cc = self.compiler();
                        cc.push_row(Val::Row { entity: ename.clone(), alias: "x".into() });
                        let fsql = cc.pred(filter);
                        let groups = format!("__groups.{ename}.{i}");
                        out.push(Step::Let {
                            name: groups.clone(),
                            sql: sql(format!(
                                "SELECT coalesce(jsonb_agg(DISTINCT x.{})::text, '[]') FROM {} x WHERE x.{} = {id} AND {fsql}",
                                q(&per_col),
                                q(&t.table),
                                q(&col)
                            )),
                            code: None,
                        });
                        let per_ty = match t.col(per) {
                            Some(Col::Ref { target, .. }) => Ty::Entity(target.clone()),
                            _ => Ty::Text,
                        };
                        c.push();
                        c.bind(per, self.param_val(per, &per_ty));
                        let mut body = Vec::new();
                        self.stmts(c, block, &mut body);
                        c.pop();
                        let still_ok = {
                            let mut cc = self.compiler();
                            cc.push_row(Val::Row { entity: ename.clone(), alias: "y".into() });
                            let p = cc.pred(filter);
                            format!(
                                "SELECT NOT EXISTS (SELECT 1 FROM {} y WHERE y.{} = {} AND {p})",
                                q(&t.table),
                                q(&per_col),
                                typed_marker(per, &per_ty)
                            )
                        };
                        repairs.push(Step::ForEach {
                            source: sql(format!("SELECT ({}::text)::jsonb", marker(&groups))),
                            item: per.clone(),
                            steps: vec![Step::When { cond: sql(still_ok), steps: body }],
                        });
                    }
                }
                let step = match policy {
                    Some(ir::RefPolicy::Cascade) => Step::Exec {
                        sql: sql(format!("DELETE FROM {} WHERE {} = {id}", q(&t.table), q(&col))),
                        bind: None,
                        label: format!("erase: delete {ename}"),
                    },
                    Some(ir::RefPolicy::Anonymize) | Some(ir::RefPolicy::SetNull) => Step::Exec {
                        sql: sql(format!("UPDATE {} SET {} = NULL WHERE {} = {id}", q(&t.table), q(&col), q(&col))),
                        bind: None,
                        label: format!("erase: anonymize {ename}.{}", f.name),
                    },
                    Some(ir::RefPolicy::Restrict) => {
                        let code = format!("ERASE_RESTRICTED_BY_{}", snake(ename).to_uppercase());
                        Step::Check {
                            kind: CheckKind::Require,
                            sql: sql(format!("SELECT NOT EXISTS (SELECT 1 FROM {} WHERE {} = {id})", q(&t.table), q(&col))),
                            code: Some(code),
                        }
                    }
                    Some(ir::RefPolicy::Reassign { to }) => {
                        let v = c.sql(to);
                        Step::Exec {
                            sql: sql(format!("UPDATE {} SET {} = {v} WHERE {} = {id}", q(&t.table), q(&col), q(&col))),
                            bind: None,
                            label: format!("erase: reassign {ename}.{}", f.name),
                        }
                    }
                    None => {
                        // implicit createdBy/updatedBy
                        Step::Exec {
                            sql: sql(format!("UPDATE {} SET {} = NULL WHERE {} = {id}", q(&t.table), q(&col), q(&col))),
                            bind: None,
                            label: format!("erase: clear {ename}.{}", f.name),
                        }
                    }
                };
                out.push(step);
            }
        }
        out.extend(repairs);
        out.push(Step::Exec {
            sql: sql(format!("DELETE FROM {} WHERE \"id\" = {id}", q(&self.s.table(&pentity).table))),
            bind: None,
            label: format!("erase: delete {pentity}"),
        });
    }

    // ---------- queries ----------

    pub fn query(&mut self, name: &str, qd: &ir::Query) -> QueryPlan {
        // a client reads the published version; only a `drafts` query reads the working one
        let working = self.s;
        if !qd.drafts
            && let Some(p) = self.published
        {
            self.s = p;
        }
        let plan = self.query_plan(name, qd);
        self.s = working;
        plan
    }

    fn query_plan(&mut self, name: &str, qd: &ir::Query) -> QueryPlan {
        let mut c = self.compiler();
        c.push();
        let params = self.params(&mut c, &qd.params);
        let mut prelude = self.loads(&mut c, &qd.params, &BTreeSet::new(), false);
        prelude.extend(self.tenant_steps(&mut c, &qd.params, qd.cross_tenant, false));
        for l in &qd.lets {
            let s = self.let_step(&mut c, l);
            prelude.push(s);
        }
        if !qd.fetches.is_empty() {
            self.fail_at_name("'fetch' (external reads in queries) is not available yet");
        }
        if let Some(al) = &qd.allow {
            let s = self.allow_step(&mut c, al);
            prelude.push(s);
        }
        prelude.extend(self.consent_steps(name));
        let mut variants = Vec::new();
        let mut touches = Vec::new();
        let single = matches!(qd.source, Some(ir::QuerySource::Param { .. }));
        let page = qd.page.as_ref().map(|p| PageSpec {
            size: p.size as u64,
            keyset: p.offset_max_page.is_none(),
            max_page: p.offset_max_page.map(|x| x as u64),
        });
        let mut sort_param = None;
        match &qd.source {
            Some(ir::QuerySource::Param { param, .. }) => {
                let Some(Val::Id { entity, .. }) = self.lookup_param(qd, param) else {
                    self.fail("from param must be an entity parameter");
                    return self.empty_query(name, qd, params);
                };
                let alias = c.fresh_alias();
                c.apply_visibility = true;
                let row = Val::Row { entity: entity.clone(), alias: alias.clone() };
                c.bind(param, row.clone());
                let obj = select::object(&mut c, &row, &qd.select);
                c.apply_visibility = false;
                let text = format!(
                    "SELECT {obj} FROM {} {alias} WHERE {alias}.\"id\" = {}",
                    q(&self.s.table(&entity).table),
                    typed_marker(param, &Ty::Uuid)
                );
                variants.push(QueryVariant { when: None, main: sql(text), key_count: 0 });
                for t in &qd.touches {
                    if let Node::Field { field: f, .. } = &t.node {
                        let dedupe = self.counter_dedupe(&entity, f);
                        touches.push(Touch {
                            entity: entity.clone(),
                            field: f.clone(),
                            id: sql(format!("SELECT {}", typed_marker(param, &Ty::Uuid))),
                            dedupe_seconds: dedupe,
                        });
                    }
                }
            }
            Some(src @ (ir::QuerySource::Entity { .. } | ir::QuerySource::Search { .. })) => {
                let search = match src {
                    ir::QuerySource::Search { search, query, alias } => match self.s.searches.get(search) {
                        Some(cfg) => Some((cfg.clone(), query.as_ref(), alias.as_str())),
                        None => {
                            self.fail(format!("search '{search}' has no index"));
                            return self.empty_query(name, qd, params);
                        }
                    },
                    _ => None,
                };
                let (entity, alias) = match (src, &search) {
                    (ir::QuerySource::Entity { entity, alias }, _) => (entity.clone(), alias.clone()),
                    (_, Some((cfg, _, alias))) => (cfg.entity.clone(), alias.to_string()),
                    _ => return self.empty_query(name, qd, params),
                };
                let (entity, alias) = (&entity, &alias);
                let cases: Vec<(Option<String>, Vec<ir::SortKey>)> = match &qd.sort {
                    Some(ir::Sort::Keys { keys }) => vec![(None, keys.clone())],
                    Some(ir::Sort::ByParam { param, cases }) => {
                        sort_param = Some(param.clone());
                        cases.iter().map(|(k, keys)| (Some(k.clone()), keys.clone())).collect()
                    }
                    None => vec![(None, Vec::new())],
                };
                for (when, keys) in cases {
                    let text = self.list_sql(&mut c, qd, entity, alias, &keys, page.as_ref(), search.as_ref().map(|(cfg, q, _)| (cfg, *q)));
                    variants.push(QueryVariant { when, main: sql(text), key_count: keys.len() + 1 });
                }
            }
            Some(ir::QuerySource::Call { .. }) => {
                self.fail("a query from an extension call has no plan");
            }
            None => {}
        }
        c.pop();
        let decrypt = decrypt_paths(std::mem::take(&mut c.enc_paths));
        self.absorb(&mut c);
        QueryPlan {
            name: name.to_string(),
            params,
            internal: qd.internal,
            prelude,
            variants,
            sort_param,
            single,
            page,
            cache_seconds: qd.cache.as_ref().map(|c| c.seconds.max(0) as u64),
            touches,
            rate_limits: rates(&qd.rate_limits),
            errors: facts::query_errors(self.core, name, qd).iter().map(error_spec).collect(),
            output: self.query_output(qd),
            decrypt,
        }
    }

    fn counter_dedupe(&self, entity: &str, field: &str) -> Option<u64> {
        let f = self.core.entities.get(entity)?.fields.iter().find(|f| f.name == field)?;
        match &f.kind {
            ir::FieldKind::Counter { dedupe: Some(d), .. } => Some(d.within_seconds.max(0) as u64),
            _ => None,
        }
    }

    fn lookup_param(&self, qd: &ir::Query, name: &str) -> Option<Val> {
        // parameters are bound in the compiler scope; recompute from the declaration type
        let p = qd.params.iter().find(|p| p.name == name)?;
        Some(self.param_val(name, &Ty::from_ir(&p.ty)))
    }

    fn empty_query(&self, name: &str, qd: &ir::Query, params: Vec<ParamSpec>) -> QueryPlan {
        QueryPlan {
            name: name.to_string(),
            params,
            internal: qd.internal,
            prelude: Vec::new(),
            variants: Vec::new(),
            sort_param: None,
            single: true,
            page: None,
            cache_seconds: None,
            touches: Vec::new(),
            rate_limits: Vec::new(),
            errors: Vec::new(),
            output: serde_json::Value::Null,
            decrypt: Vec::new(),
        }
    }

    fn list_sql(
        &mut self,
        c: &mut Compiler<'_>,
        qd: &ir::Query,
        entity: &str,
        alias_name: &str,
        keys: &[ir::SortKey],
        page: Option<&PageSpec>,
        search: Option<(&schema::SearchCfg, &Expr)>,
    ) -> String {
        let t = self.s.table(entity).clone();
        let a = c.fresh_alias();
        let row = Val::Row { entity: entity.to_string(), alias: a.clone() };
        c.push();
        c.bind(alias_name, row.clone());
        let mut conds = Vec::new();
        let mut keys = keys.to_vec();
        let mut joined = String::new();
        if let Some((cfg, text)) = search {
            // the analyzed query is computed once per statement, not per row
            let qa = format!("{a}_q");
            let words = c.sql(text);
            let tsquery = if cfg.prefix {
                // every word of the query matches the beginning of a document word: 'a' & 'b' becomes 'a':* & 'b':*
                format!(
                    "regexp_replace(websearch_to_tsquery('{}'::regconfig, {words})::text, '''((?:[^'']|'''')*)''', '''\\1'':*', 'g')::tsquery",
                    cfg.config
                )
            } else {
                format!("websearch_to_tsquery('{}'::regconfig, {words})", cfg.config)
            };
            joined = format!(" CROSS JOIN (SELECT {tsquery} AS tq) {qa}");
            conds.push(format!("{a}.{} @@ {qa}.tq", q(&cfg.column)));
            c.search_ranks.insert(alias_name.to_string(), format!("round(ts_rank({a}.{}, {qa}.tq)::numeric, 6)", q(&cfg.column)));
            if keys.is_empty() {
                let ty = ir::Type::Decimal { precision: None, scale: None };
                keys.push(ir::SortKey { expr: Expr { ty, node: Node::SearchRank { alias: alias_name.to_string() } }, desc: true });
            }
            if !qd.group_by.is_empty() {
                self.fail("a search query cannot be grouped");
            }
        }
        let keys = keys.as_slice();
        if let Some(f) = &qd.filter {
            conds.push(c.pred(f));
        }
        if t.soft_delete {
            conds.push(format!("{a}.\"deleted_at\" IS NULL"));
        }
        if let Some(ts) = &c.tenant
            && tenant::scoped(self.core, entity)
            && let Some(mine) = tn::row_sql(self.core, self.s, entity, &a)
        {
            conds.push(format!("{mine} = {}", ts.sql));
        }
        c.apply_visibility = true;
        if let Some(v) = c.visibility_sql(entity, &row) {
            conds.push(v);
        }
        if !qd.group_by.is_empty() {
            let text = self.group_sql(c, qd, &t.table, &a, &conds);
            c.apply_visibility = false;
            c.pop();
            return text;
        }
        let mut key_sql: Vec<(String, bool)> = keys
            .iter()
            .map(|k| {
                let s = c.sql(&k.expr);
                let s = match Ty::from_ir(&k.expr.ty) {
                    Ty::Enum(en) => c.enum_rank(&en, &s).unwrap_or(s),
                    _ => s,
                };
                (s, k.desc)
            })
            .collect();
        key_sql.push((format!("{a}.\"id\""), keys.last().is_some_and(|k| k.desc)));
        let order: Vec<String> = key_sql.iter().map(|(s, d)| format!("{s} {} NULLS LAST", if *d { "DESC" } else { "ASC" })).collect();
        let mut limit = String::new();
        if let Some(p) = page {
            if p.keyset {
                // cursor: JSON array of the previous page's last key values
                let cur = marker("__cursor");
                let mut ors = Vec::new();
                for i in 0..key_sql.len() {
                    let mut ands = Vec::new();
                    for (j, (ks, _)) in key_sql.iter().enumerate().take(i) {
                        // a score travels through the cursor as a JSON number, whose trailing zeros are not kept: compare it as a number
                        if keys.get(j).is_some_and(|k| matches!(k.expr.node, Node::SearchRank { .. })) {
                            ands.push(format!("({ks}) IS NOT DISTINCT FROM (((({cur})::text)::jsonb ->> {j})::numeric)"));
                        } else {
                            ands.push(format!("({ks})::text IS NOT DISTINCT FROM ((({cur})::text)::jsonb ->> {j})"));
                        }
                    }
                    let (ks, desc) = &key_sql[i];
                    let op = if *desc { "<" } else { ">" };
                    ands.push(format!(
                        "({ks}) {op} ((((({cur})::text)::jsonb ->> {i})))::{}",
                        key_cast(self.core, keys.get(i).map(|k| &k.expr), i == key_sql.len() - 1)
                    ));
                    ors.push(format!("({})", ands.join(" AND ")));
                }
                conds.push(format!("(({cur})::text IS NULL OR {})", ors.join(" OR ")));
                limit = format!(" LIMIT {}", p.size + 1);
            } else {
                limit = format!(" LIMIT {} OFFSET coalesce((({}::text)::bigint), 0)", p.size + 1, marker("__offset"));
            }
        }
        // each element of the result array is one object
        c.path_stack.push("[]".into());
        let obj = select::object(c, &row, &qd.select);
        c.path_stack.pop();
        c.apply_visibility = false;
        c.pop();
        let key_array = format!("jsonb_build_array({})", key_sql.iter().map(|(s, _)| format!("({s})")).collect::<Vec<_>>().join(", "));
        let w = Compiler::where_sql(&conds);
        format!(
            "SELECT coalesce(jsonb_agg(x.j || jsonb_build_object('__k', x.k) ORDER BY x.n), '[]'::jsonb) FROM (SELECT {obj} AS j, {key_array} AS k, row_number() OVER (ORDER BY {}) AS n FROM {} {a}{joined}{w} ORDER BY {}{limit}) x",
            order.join(", "),
            q(&t.table),
            order.join(", ")
        )
    }

    fn group_sql(&mut self, c: &mut Compiler<'_>, qd: &ir::Query, table: &str, a: &str, conds: &[String]) -> String {
        let groups: Vec<(String, Val)> = qd
            .group_by
            .iter()
            .map(|g| {
                let name = match &g.node {
                    Node::Field { field, .. } | Node::RowField { field, .. } => field.clone(),
                    Node::Local { name } | Node::Param { name } => name.clone(),
                    _ => String::new(),
                };
                (name, c.expr(g))
            })
            .collect();
        let mut pairs = Vec::new();
        let mut group_cols = Vec::new();
        for (name, v) in &groups {
            let s = c.scalar(v);
            group_cols.push(s.clone());
            let item = qd.select.items.iter().find(|it| it.name == *name && it.value.is_none());
            c.path_stack.extend(["[]".to_string(), name.clone()]);
            let json = match (v, item.and_then(|i| i.sub.as_ref())) {
                (Val::Id { entity, .. } | Val::Row { entity, .. }, Some(sel)) => select::nested(c, entity, &s, sel),
                (Val::Scalar { ty: ty @ (Ty::Decimal | Ty::Money(_)), .. }, _) => select::scalar_json(&s, ty),
                _ => format!("to_jsonb({s})"),
            };
            c.path_stack.truncate(c.path_stack.len() - 2);
            if item.is_some() {
                pairs.push(format!("{}, {json}", lit(name)));
            }
        }
        c.group_context = true;
        for it in &qd.select.items {
            if let Some(e) = &it.value {
                let v = c.json_sql(e);
                pairs.push(format!("{}, to_jsonb({v})", lit(&it.name)));
            }
        }
        c.group_context = false;
        format!(
            "SELECT coalesce(jsonb_agg(g.j), '[]'::jsonb) FROM (SELECT jsonb_build_object({}) AS j FROM {} {a}{} GROUP BY {}) g",
            pairs.join(", "),
            q(table),
            Compiler::where_sql(conds),
            group_cols.join(", ")
        )
    }

    // ---------- handlers and schedules ----------

    pub fn handler(&mut self, event: &str, binding: &str, when: Option<&Expr>, body: &[Stmt], idx: usize, cross: bool) -> Handler {
        let mut c = self.compiler();
        c.push();
        c.bind(binding, Val::Json { sql: format!("({}::text)::jsonb", marker("__event")), record: Some(format!("event:{event}")) });
        // the row the event names fixes the tenant of the handler
        if let Some((field, entity)) = tenant::event_anchor(self.core, event).filter(|_| !cross) {
            let id = format!("((({}::text)::jsonb ->> {})::uuid)", marker("__event"), lit(&field));
            if let Some(sel) = tn::rows_select(self.core, self.s, &entity, &id, false) {
                c.tenant = Some(TenantScope { sql: format!("({sel} LIMIT 1)"), checked: Vec::new() });
            }
        }
        let when = when.map(|w| {
            let p = c.pred(w);
            sql(format!("SELECT {p}"))
        });
        // handlers of one outbox row share a transaction; each starts from its own tenant
        let mut steps: Vec<Step> = self.context_pin(&c, cross).into_iter().collect();
        self.stmts(&mut c, body, &mut steps);
        c.pop();
        self.absorb(&mut c);
        Handler { event: event.to_string(), name: format!("on_{event}_{idx}"), when, steps }
    }

    pub fn schedule(&mut self, s: &ir::Schedule) -> Schedule {
        let mut c = self.compiler();
        c.push();
        let mut source = None;
        let mut item = None;
        let mut steps = Vec::new();
        if let Some(se) = &s.for_each {
            let parts = c.begin_set(se);
            let SetElem::Entity(e) = parts.elem.clone() else {
                c.end_set();
                self.fail("schedule 'for' needs entity rows");
                return Schedule { name: s.name.clone(), cron: String::new(), tz: s.tz.clone(), source: None, item: None, steps: Vec::new() };
            };
            c.end_set();
            source = Some(sql(format!(
                "SELECT coalesce(jsonb_agg({}.\"id\"), '[]'::jsonb) FROM {}{}",
                parts.alias,
                parts.from,
                Compiler::where_sql(&parts.conds)
            )));
            let name = se.alias.clone().unwrap_or_else(|| "it".into());
            // the sweep above spans tenants; each item fixes the tenant of its own steps
            if let Some(sel) = tn::rows_select(self.core, self.s, &e, &typed_marker(&name, &Ty::Uuid), false).filter(|_| !s.cross_tenant) {
                c.tenant = Some(TenantScope { sql: format!("({sel} LIMIT 1)"), checked: Vec::new() });
            }
            c.bind(&name, Val::Id { entity: e, sql: typed_marker(&name, &Ty::Uuid) });
            item = Some(name);
        }
        // the steps run once per item in one transaction: each item starts from its own tenant
        steps.extend(self.context_pin(&c, s.cross_tenant));
        self.stmts(&mut c, &s.body, &mut steps);
        c.pop();
        self.absorb(&mut c);
        Schedule { name: s.name.clone(), cron: cron(s), tz: s.tz.clone(), source, item, steps }
    }

    pub fn retain(&mut self, entity: &str, keep: &ir::Dur, after: &[String], anonymize: bool, notify: Option<&ir::RetainNotify>) -> Schedule {
        let t = self.s.table(entity).clone();
        // after-path: resolve through references to the time column
        let mut from = format!("{} x", q(&t.table));
        let mut cur_entity = entity.to_string();
        let mut cur_alias = "x".to_string();
        let mut time_sql = String::new();
        for (i, part) in after.iter().enumerate() {
            match self.s.table(&cur_entity).col(part).cloned() {
                Some(Col::Ref { col, target }) => {
                    let na = format!("j{i}");
                    from.push_str(&format!(" JOIN {} {na} ON {na}.\"id\" = {cur_alias}.{}", q(&self.s.table(&target).table), q(&col)));
                    cur_entity = target;
                    cur_alias = na;
                }
                Some(Col::Scalar { col, ty: Ty::Range(_) }) => {
                    let end = after.get(i + 1).map(|p| p.as_str()) == Some("end");
                    time_sql = format!("{}({cur_alias}.{})", if end { "upper" } else { "lower" }, q(&col));
                    break;
                }
                Some(Col::Scalar { col, .. }) => {
                    time_sql = format!("{cur_alias}.{}", q(&col));
                    break;
                }
                _ => {
                    self.fail(format!("cannot follow '{part}' in retain"));
                    break;
                }
            }
        }
        let keep = duration_sql(keep);
        // one statement expires rows of every tenant; the framework defines it, row by row, so it is exempt from the pin
        let mut steps: Vec<Step> = tenant::active(self.core).then(|| tn::pin_step(None, true)).into_iter().collect();
        steps.push(Step::Exec {
            sql: sql(if anonymize {
                format!("UPDATE {t} SET \"id\" = \"id\" WHERE false /* anonymize retention: TODO */", t = q(&t.table))
            } else {
                format!("DELETE FROM {} WHERE \"id\" IN (SELECT x.\"id\" FROM {from} WHERE {time_sql} + {keep} < now())", q(&t.table))
            }),
            bind: None,
            label: format!("retain {entity}"),
        });
        if let Some(ir::RetainNotify { path, before, via }) = notify {
            let actor = self.actor_entity().unwrap_or_default().to_string();
            let recip = match path.first().and_then(|p| self.s.table(entity).col(p)) {
                Some(Col::Ref { col, target }) if *target == actor => format!("x.{}", q(col)),
                _ => "NULL".into(),
            };
            let b = duration_sql(before);
            let template = template_of(via);
            steps.insert(
                0,
                Step::Notify {
                    recipients: sql(format!(
                        "SELECT coalesce(jsonb_agg(DISTINCT {recip}), '[]'::jsonb) FROM {from} WHERE {time_sql} + {keep} - {b} < now() AND {time_sql} + {keep} - {b} >= now() - interval '1 day'"
                    )),
                    template,
                    payload: sql("SELECT '{}'::jsonb".into()),
                    category: None,
                    digest_seconds: None,
                },
            );
        }
        Schedule { name: format!("retain_{entity}"), cron: "0 3 * * *".into(), tz: None, source: None, item: None, steps }
    }
}

/// Field a `set` / `update` assignment writes: `row.f` or the bare `f` of the row being updated.
fn target_field(e: &Expr) -> Option<String> {
    match &e.node {
        Node::RowField { field, .. } | Node::Field { field, .. } => Some(field.clone()),
        _ => None,
    }
}

fn key_cast(core: &ir::Program, e: Option<&Expr>, is_id: bool) -> &'static str {
    if is_id {
        return "uuid";
    }
    match e.map(|e| Ty::from_ir(&e.ty)) {
        Some(Ty::Enum(en)) if core.enums.get(&en).is_some_and(|d| d.ordered) => "int",
        Some(t) => schema::cast(&t),
        None => "text",
    }
}

fn cron(s: &ir::Schedule) -> String {
    let (h, m) = s.at.unwrap_or((0, 0));
    match &s.every {
        ir::Every::Day => format!("{m} {h} * * *"),
        ir::Every::Week { day } => format!("{m} {h} * * {day}"),
        ir::Every::MonthDay { day } => format!("{m} {h} {day} * *"),
        ir::Every::Interval { seconds } => format!("@every {}s", (*seconds).max(0)),
    }
}

fn rates(rs: &[ir::RateLimit]) -> Vec<RateLimit> {
    rs.iter()
        .map(|r| RateLimit {
            n: r.n as u64,
            per_seconds: r.per_seconds.max(0) as u64,
            key: match &r.key {
                ir::RateKey::Actor => "actor".into(),
                ir::RateKey::Client => "client".into(),
                ir::RateKey::Path { path } => path.join("."),
            },
        })
        .collect()
}

impl Planner<'_> {
    /// `versioned` entities: a command that edits a row it received as a
    /// parameter must say which version it edited (`<param>Version`).
    fn version_checks(&mut self, cd: &ir::Command) -> (Vec<ParamSpec>, Vec<Step>) {
        let mut params = Vec::new();
        let mut steps = Vec::new();
        for v in facts::version_params(self.core, cd) {
            let (name, param, reason) = (v.name, v.of, v.reason);
            let Some(Ty::Entity(e)) = cd.params.iter().find(|p| p.name == param).map(|p| Ty::from_ir(&p.ty)) else { continue };
            let t = self.s.table(&e);
            steps.push(Step::Check {
                kind: CheckKind::Version,
                sql: sql(format!(
                    "SELECT \"version\" = {} FROM {} WHERE \"id\" = {}",
                    typed_marker(&name, &Ty::Int),
                    q(&t.table),
                    typed_marker(&param, &Ty::Uuid)
                )),
                code: Some(reason),
            });
            params.push(ParamSpec { name, ty: TypeSpec::Int { min: Some(1), max: None }, optional: false, default: None });
        }
        (params, steps)
    }
}

// ---------- L3 forms compiled directly into plans ----------

impl<'x> Planner<'x> {
    fn begin(&mut self) {
        self.emits.clear();
    }

    fn finish(&mut self, intent: &FormIntent, params: Vec<ParamSpec>, steps: Vec<Step>, returns: Option<Sql>) -> CommandPlan {
        CommandPlan {
            name: intent.name.clone(),
            params,
            internal: false,
            idempotent: false,
            idempotency_key: None,
            audited: false,
            steps,
            returns,
            rate_limits: Vec::new(),
            errors: error_specs(&intent.facts),
            effects: intent.facts.effects.iter().cloned().collect(),
            writes: intent.facts.writes.iter().cloned().collect(),
            emits: self.emits.iter().cloned().collect(),
            output: serde_json::Value::Null,
            decrypt: Vec::new(),
        }
    }

    fn actor_val(&self) -> Option<Val> {
        self.actor_entity().map(|e| Val::Id { entity: e.to_string(), sql: typed_marker(ACTOR, &Ty::Uuid) })
    }

    /// `grant link X { ... }` → `IssueX(scope...)` returning a one-time token, and
    /// `RedeemX(token, redeem params...)` that grants the relation / runs the block.
    pub fn grant_link(&mut self, g: &ir::GrantLink) -> Vec<CommandPlan> {
        let intents = form_intents::grant_link(self.core, g);
        let link = lit(&g.name);
        let secs = g.expires_seconds.max(0) as u64;
        // ---- issue
        self.begin();
        let mut c = self.compiler();
        c.push();
        let params = self.params(&mut c, &intents.issue.params);
        let mut steps = self.loads(&mut c, &g.scope, &BTreeSet::new(), false);
        steps.extend(self.tenant_steps(&mut c, &g.scope, false, false));
        steps.push(self.allow_step(&mut c, &ir::Policy { cond: g.issued_by.clone(), code: None }));
        steps.push(Step::Let { name: "__token".into(), sql: sql("SELECT encode(gen_random_bytes(24), 'hex')".into()), code: None });
        let scope_json: Vec<String> = g.scope.iter().map(|p| format!("{}, {}", lit(&p.name), typed_marker(&p.name, &Ty::Text))).collect();
        let to = match &g.to {
            Some(t) => c.sql(t),
            None => "NULL".into(),
        };
        steps.push(Step::Exec {
            sql: sql(format!(
                "INSERT INTO \"_aip_grant\" (\"link\", \"token_hash\", \"scope\", \"to_member\", \"issued_by\", \"expires_at\", \"uses_left\") VALUES ({link}, encode(digest({tok}, 'sha256'), 'hex'), jsonb_build_object({scope}), {to}, {actor}, now() + make_interval(secs => {secs}), {uses})",
                tok = typed_marker("__token", &Ty::Text),
                scope = scope_json.join(", "),
                actor = typed_marker(ACTOR, &Ty::Uuid),
                uses = g.uses.unwrap_or(1_000_000),
            )),
            bind: None,
            label: format!("issue {}", g.name),
        });
        let returns = Some(sql(format!(
            "SELECT jsonb_build_object('token', ({}::text), 'expires_at', now() + make_interval(secs => {secs}))",
            marker("__token")
        )));
        c.pop();
        self.absorb(&mut c);
        let issue = self.finish(&intents.issue, params, steps, returns);

        // ---- redeem
        self.begin();
        let mut c = self.compiler();
        c.push();
        let params = self.params(&mut c, &intents.redeem.params);
        let mut steps = Vec::new();
        steps.push(Step::Let {
            name: "__grant".into(),
            sql: sql(format!(
                "SELECT g.\"id\"::text FROM \"_aip_grant\" g WHERE g.\"link\" = {link} AND g.\"token_hash\" = encode(digest({tok}, 'sha256'), 'hex') AND g.\"expires_at\" > now() AND g.\"uses_left\" > 0 AND (g.\"to_member\" IS NULL OR g.\"to_member\" = {actor}) FOR UPDATE",
                tok = typed_marker("token", &Ty::Text),
                actor = typed_marker(ACTOR, &Ty::Uuid),
            )),
            code: Some("INVALID_GRANT_LINK".into()),
        });
        steps.push(Step::Check { kind: CheckKind::Allow, sql: sql(format!("SELECT {} IS NOT NULL", typed_marker(ACTOR, &Ty::Uuid))), code: None });
        for p in &g.scope {
            let ty = Ty::from_ir(&p.ty);
            steps.push(Step::Let {
                name: p.name.clone(),
                sql: sql(format!("SELECT \"scope\" ->> {} FROM \"_aip_grant\" WHERE \"id\" = ({}::text)::uuid", lit(&p.name), marker("__grant"))),
                code: None,
            });
            c.bind(&p.name, self.param_val(&p.name, &ty));
        }
        // the scope the link was issued for fixes the tenant of what redeeming it writes
        steps.extend(self.tenant_steps(&mut c, &g.scope, false, true));
        if let Some(actor) = self.actor_val() {
            c.bind("holder", actor);
        }
        for r in &g.requires {
            let cond = c.pred(&r.cond);
            steps.push(Step::Check { kind: CheckKind::Require, sql: sql(format!("SELECT {cond}")), code: Some(r.code.clone()) });
        }
        steps.push(Step::Exec {
            sql: sql(format!("UPDATE \"_aip_grant\" SET \"uses_left\" = \"uses_left\" - 1 WHERE \"id\" = ({}::text)::uuid", marker("__grant"))),
            bind: None,
            label: "consume link".into(),
        });
        if let Some((rel_call, role)) = &g.grants {
            match self.grant_insert(rel_call, role, &g.redeem_params) {
                Ok(stmt) => self.stmt(&mut c, &stmt, &mut steps),
                Err(msg) => self.fail(msg),
            }
        }
        if let Some(b) = &g.on_redeem {
            self.stmts(&mut c, b, &mut steps);
        }
        c.pop();
        self.absorb(&mut c);
        let redeem = self.finish(&intents.redeem, params, steps, None);
        vec![issue, redeem]
    }

    /// `grants: rel(a, b) as ROLE` where `rel` is `the E x where x.f = p and ...`:
    /// insert an E with the equalities, the role, and redeem inputs of the same name.
    fn grant_insert(&self, rel_call: &Expr, role: &str, redeem: &[ir::Param]) -> Result<Stmt, String> {
        let Node::Call(call) = &rel_call.node else { return Err("grants needs a relation call".into()) };
        let rel = self.core.relations.get(callee_name(call)).ok_or("grants needs a declared relation")?;
        let Node::The { set: se } = &rel.body.node else { return Err("grants needs a relation of the form 'the E x where ...'".into()) };
        let SetSource::Entity { entity: ent } = &se.source else { return Err("grants relation must select an entity".into()) };
        let alias = se.alias.clone().unwrap_or_default();
        let mut fields: Vec<(String, Expr)> = Vec::new();
        fn eqs<'e>(e: &'e Expr, out: &mut Vec<(&'e Expr, &'e Expr)>) {
            match &e.node {
                Node::Binary { op: ir::BinOp::And, l, r } => {
                    eqs(l, out);
                    eqs(r, out);
                }
                Node::Binary { op: ir::BinOp::Eq, l, r } => out.push((l, r)),
                _ => {}
            }
        }
        let mut pairs = Vec::new();
        if let Some(f) = &se.filter {
            eqs(f, &mut pairs);
        }
        for (l, r) in pairs {
            let (Node::Field { base: b, field: fname, .. }, Some(pname)) = (&l.node, param_name(r)) else { continue };
            if !matches!(&b.node, Node::Local { name } if *name == alias) {
                continue;
            }
            let idx = rel.params.iter().position(|p| p.name == pname).ok_or("grants relation compares with a non-parameter")?;
            let arg = call.args.get(idx).ok_or("missing relation argument")?;
            fields.push((fname.clone(), arg.value.clone()));
        }
        let info = self.core.entities.get(ent).ok_or("unknown entity")?;
        let role_field = info
            .fields
            .iter()
            .find(|f| matches!(&f.ty, Type::Enum { name: en } if self.core.enums.get(en).is_some_and(|d| d.values.iter().any(|v| v == role))))
            .ok_or("no enum field takes this role")?;
        fields.push((
            role_field.name.clone(),
            Expr {
                ty: Type::Text { min: None, max: None, trim: false, lower: false, pattern: None },
                node: Node::Lit { lit: Literal::Text(role.to_string()) },
            },
        ));
        for p in redeem {
            if info.fields.iter().any(|f| f.name == p.name) {
                fields.push((p.name.clone(), bound(&p.name, &p.ty)));
            }
        }
        let missing: Vec<String> =
            info.fields.iter().filter(|f| field_required(f) && !fields.iter().any(|(n, _)| *n == f.name)).map(|f| f.name.clone()).collect();
        if !missing.is_empty() {
            return Err(format!("grants cannot fill required field(s) {} of {}; add them with 'redeem with (...)'", missing.join(", "), ent));
        }
        Ok(Stmt::Insert { entity: ent.clone(), from: None, values: fields, bind: None })
    }

    /// `verification X { ... }` → `RequestX(target)` and `VerifyX(target, code)`.
    pub fn verification(&mut self, v: &ir::Verification) -> Vec<CommandPlan> {
        let intents = form_intents::verification(self.core, v);
        let name = lit(&v.name);
        let ttl = v.ttl_seconds.max(0) as u64;
        let actor = typed_marker(ACTOR, &Ty::Uuid);
        // ---- request
        self.begin();
        let mut c = self.compiler();
        c.push();
        let params = self.params(&mut c, &intents.request.params);
        let mut steps = vec![Step::Check { kind: CheckKind::Allow, sql: sql(format!("SELECT {actor} IS NOT NULL")), code: None }];
        steps.extend(self.tenant_steps(&mut c, &intents.request.params, false, false));
        if let Some(w) = &v.target_where {
            let cond = c.pred(w);
            steps.push(Step::Check {
                kind: CheckKind::Require,
                sql: sql(format!("SELECT {cond}")),
                code: Some("VERIFICATION_TARGET_INVALID".into()),
            });
        }
        if let Some(r) = &v.resend_after_seconds {
            steps.push(Step::Check {
                kind: CheckKind::Require,
                sql: sql(format!(
                    "SELECT NOT EXISTS (SELECT 1 FROM \"_aip_verification\" WHERE \"name\" = {name} AND \"subject\" = {actor} AND \"created_at\" > now() - make_interval(secs => {}))",
                    (*r).max(0) as u64
                )),
                code: Some("VERIFICATION_RESEND_TOO_SOON".into()),
            });
        }
        let code_sql = if v.alnum {
            format!("SELECT upper(substr(encode(gen_random_bytes(16), 'hex'), 1, {}))", v.length)
        } else {
            format!(
                "SELECT lpad(((('x' || encode(gen_random_bytes(4), 'hex'))::bit(32)::bigint % (10 ^ {n})::bigint))::text, {n}, '0')",
                n = v.length
            )
        };
        steps.push(Step::Let { name: "__code".into(), sql: sql(code_sql), code: None });
        steps.push(Step::Exec {
            sql: sql(format!("DELETE FROM \"_aip_verification\" WHERE \"name\" = {name} AND \"subject\" = {actor}")),
            bind: None,
            label: "replace pending code".into(),
        });
        steps.push(Step::Exec {
            sql: sql(format!(
                "INSERT INTO \"_aip_verification\" (\"name\", \"subject\", \"target\", \"code_hash\", \"expires_at\", \"attempts_left\") VALUES ({name}, {actor}, {t}, encode(digest({code}, 'sha256'), 'hex'), now() + make_interval(secs => {ttl}), {a})",
                t = marker("target"),
                code = marker("__code"),
                a = v.attempts
            )),
            bind: None,
            label: "store code hash".into(),
        });
        let template = match v.deliver.args.first().and_then(|a| lit_text(&a.value)) {
            Some(s) => s.to_string(),
            None => v.name.clone(),
        };
        steps.push(Step::Deferred {
            effect: callee_name(&v.deliver).to_string(),
            args: sql(format!(
                "SELECT jsonb_build_object('template', {}, 'to', ({}::text), 'code', ({}::text))",
                lit(&template),
                marker("target"),
                marker("__code")
            )),
            key: None,
            on_failure: Vec::new(),
        });
        c.pop();
        self.absorb(&mut c);
        let request = self.finish(&intents.request, params, steps, None);

        // ---- verify
        self.begin();
        let mut c = self.compiler();
        c.push();
        let params = self.params(&mut c, &intents.verify.params);
        let mut steps = vec![Step::Check { kind: CheckKind::Allow, sql: sql(format!("SELECT {actor} IS NOT NULL")), code: None }];
        steps.extend(self.tenant_steps(&mut c, &intents.verify.params, false, true));
        steps.push(Step::Let {
            name: "__vid".into(),
            sql: sql(format!(
                "SELECT \"id\"::text FROM \"_aip_verification\" WHERE \"name\" = {name} AND \"subject\" = {actor} AND \"target\" = ({}::text) AND \"expires_at\" > now() AND \"attempts_left\" > 0 FOR UPDATE",
                marker("target")
            )),
            code: Some("VERIFICATION_NOT_FOUND".into()),
        });
        steps.push(Step::Exec {
            sql: sql(format!(
                "UPDATE \"_aip_verification\" SET \"attempts_left\" = \"attempts_left\" - 1 WHERE \"id\" = ({}::text)::uuid",
                marker("__vid")
            )),
            bind: None,
            label: "spend an attempt".into(),
        });
        steps.push(Step::When {
            cond: sql(format!(
                "SELECT NOT EXISTS (SELECT 1 FROM \"_aip_verification\" WHERE \"id\" = ({}::text)::uuid AND \"code_hash\" = encode(digest(({}::text), 'sha256'), 'hex'))",
                marker("__vid"),
                marker("code")
            )),
            // the spent attempt must survive the failure
            steps: vec![Step::Fail { code: "VERIFICATION_CODE_MISMATCH".into(), commit: true }],
        });
        steps.push(Step::Exec {
            sql: sql(format!("DELETE FROM \"_aip_verification\" WHERE \"id\" = ({}::text)::uuid", marker("__vid"))),
            bind: None,
            label: "consume code".into(),
        });
        let (subj, tgt, block) = &v.on_verified;
        if let Some(actor) = self.actor_val() {
            c.bind(subj, actor);
        }
        let tty = Ty::from_ir(&v.target);
        c.bind(tgt, self.param_val("target", &tty));
        self.stmts(&mut c, block, &mut steps);
        c.pop();
        self.absorb(&mut c);
        let verify = self.finish(&intents.verify, params, steps, None);
        vec![request, verify]
    }
}

/// Must be given on insert: stored, not optional, no default, not generated by the runtime.
fn field_required(f: &ir::Field) -> bool {
    let writable = matches!(f.kind, ir::FieldKind::Stored | ir::FieldKind::Ref { .. } | ir::FieldKind::RefUnion { .. });
    let generated = matches!(f.generated, Some(ir::Generated::Sequence { .. } | ir::Generated::Slug { .. } | ir::Generated::Position { .. }));
    writable && !f.optional && f.default.is_none() && !generated
}

impl Planner<'_> {
    /// For each assigned `Json validated by <snap>` field, validate the value
    /// against the question list stored in the pinned history version.
    fn dynamic_validations(&mut self, c: &mut Compiler<'_>, entity: &str, fields: &[(String, Expr)]) {
        let core = self.core;
        let Some(info) = core.entities.get(entity) else { return };
        let assigned = |name: &str| fields.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        for f in &info.fields {
            let Type::Json { validated_by: Some(path), .. } = &f.ty else { continue };
            let first = path.split('.').next().unwrap_or_default();
            let (Some(value_expr), Some(snap_expr)) = (assigned(&f.name), assigned(first)) else { continue };
            let Val::Snap { entity: target, id, version } = c.expr(&snap_expr) else { continue };
            let dyn_field = core.entities.get(&target).and_then(|t| t.traits.dynamic_schema.clone());
            let Some(dyn_field) = dyn_field else { continue };
            let col = match self.s.table(&target).col(&dyn_field) {
                Some(Col::Scalar { col, .. }) => col.clone(),
                _ => continue,
            };
            let v = c.sql(&value_expr);
            let hist = self.s.table(&target).history_table.clone();
            self.validations.push(Step::ValidateDynamic {
                value: sql(format!("SELECT ({v})::jsonb")),
                schema: sql(format!("SELECT h.\"data\" -> {} FROM {} h WHERE h.\"id\" = {id} AND h.\"version\" = {version}", lit(&col), q(&hist))),
                path: f.name.clone(),
            });
        }
    }
}

impl Planner<'_> {
    fn query_output(&self, qd: &ir::Query) -> serde_json::Value {
        // search-backed queries have no plan, hence no output shape
        if matches!(qd.source, Some(ir::QuerySource::Call { .. })) {
            return serde_json::Value::Null;
        }
        crate::shape::to_json(&qd.output)
    }
}
