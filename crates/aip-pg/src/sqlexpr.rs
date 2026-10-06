//! Typed AIP expressions -> PostgreSQL SQL.
//!
//! Every policy, precondition, filter and projection goes through here, so the
//! semantics of the language are defined by this file together with the checker.
//!
//! Conventions:
//! - Parameters are bound as text and cast: `($3::text)::uuid`.
//! - `=` compiles to `=` (index friendly); predicates are wrapped in
//!   `coalesce(..., false)` at the top so NULL never grants anything.
//!   `!=` compiles to `IS DISTINCT FROM` so a missing value counts as different.
//! - Entity values are their ids.

use crate::names::{lit, q};
use crate::schema::{Col, Schema, cast};
use crate::ty::Ty;
use aip_ir::codes;
use aip_ir::{self as ir, AggFn, BinOp, Expr, Literal, Node, SetExpr, SetSource, UnOp};
use aip_plan::{Diagnostic, Severity};
use std::collections::HashMap;

pub const ACTOR: &str = "__actor";
/// Environment name of the impersonation session id; NULL when the caller is acting as themselves.
pub const IMPERSONATION: &str = "__imp";

/// A compiled value.
#[derive(Debug, Clone)]
pub enum Val {
    /// A row in the current FROM clause.
    Row { entity: String, alias: String },
    /// A single entity identified by an id expression (may be NULL).
    Id { entity: String, sql: String },
    /// A scalar SQL expression.
    Scalar { sql: String, ty: Ty },
    /// A JSON value (record input, event payload, JSON column).
    Json { sql: String, record: Option<String> },
    /// A set of entity rows: `sql` is a subquery returning ids.
    IdSet { entity: String, sql: String },
    /// Rows of `entity` whose `via_col` references `parent`.
    Inverse { entity: String, via_col: String, parent: String },
    /// A JSON array (List<...> input or column).
    JsonArray { sql: String, elem: Ty },
    /// A set of scalars: `sql` is a subquery returning one column.
    ScalarSet { sql: String, ty: Ty },
    /// A pinned version of a history entity.
    Snap { entity: String, id: String, version: String },
}

/// Parameter references are written as markers (`\u{1}name\u{2}`) so that
/// compiled fragments can be reused across statements; `finalize` numbers them.
#[derive(Debug, Default, Clone)]
pub struct Params;

pub const MARK_OPEN: char = '\u{1}';
pub const MARK_CLOSE: char = '\u{2}';

impl Params {
    pub fn bind(&mut self, name: &str) -> String {
        marker(name)
    }

    pub fn typed(&mut self, name: &str, ty: &Ty) -> String {
        typed_marker(name, ty)
    }
}

pub fn marker(name: &str) -> String {
    format!("{MARK_OPEN}{name}{MARK_CLOSE}")
}

/// The name a compiled value is made of when it is nothing but one bound value (`($1::text)::text`): the value of a
/// parameter or binding, passed on as it arrived. Anything computed from it, or from the database, is `None`.
pub fn lone_marker(sql: &str) -> Option<String> {
    let rest = sql.strip_prefix('(')?.strip_prefix(MARK_OPEN)?;
    let (name, tail) = rest.split_once(MARK_CLOSE)?;
    let ty = tail.strip_prefix("::text)::")?;
    (!name.is_empty() && !name.contains(MARK_OPEN) && ty.chars().all(|c| c.is_ascii_alphanumeric())).then(|| name.to_string())
}

pub fn typed_marker(name: &str, ty: &Ty) -> String {
    format!("({}::text)::{}", marker(name), cast(ty))
}

/// Replaces markers with `$1..$n` (first occurrence order) and returns the SQL.
pub fn finalize(text: &str) -> aip_plan::Sql {
    let mut params: Vec<String> = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(MARK_OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + MARK_OPEN.len_utf8()..];
        let end = after.find(MARK_CLOSE).unwrap_or(after.len());
        let name = &after[..end];
        let idx = match params.iter().position(|p| p == name) {
            Some(i) => i,
            None => {
                params.push(name.to_string());
                params.len() - 1
            }
        };
        out.push_str(&format!("${}", idx + 1));
        rest = if end < after.len() { &after[end + MARK_CLOSE.len_utf8()..] } else { "" };
    }
    out.push_str(rest);
    aip_plan::Sql { text: out, params }
}

pub struct Compiler<'x> {
    pub core: &'x ir::Program,
    pub s: &'x Schema,
    pub p: Params,
    scopes: Vec<HashMap<String, Val>>,
    rows: Vec<Val>,
    counter: usize,
    /// Apply `visible to` when reading rows (data returned to clients).
    pub apply_visibility: bool,
    /// When set, `self` means "this row is the actor".
    self_row: Option<Val>,
    /// Inside `group by`: `count()` is an aggregate of the group.
    pub group_context: bool,
    /// Inside `update X via path i set ...`: aggregates over `i` are plain aggregates of the joined rows.
    pub via_alias: Option<String>,
    depth: usize,
    pub errors: Vec<Diagnostic>,
    /// IR path of the declaration being compiled; diagnostics point at it.
    pub at: String,
    /// The tenant every set of tenant-scoped rows is filtered to, unless it is bound to the call's own rows.
    /// `None` in system contexts, `cross tenant` intents and entity-level declarations.
    pub tenant: Option<crate::tenant::TenantScope>,
    /// Match score of the search hit each search alias stands for, as SQL.
    pub search_ranks: HashMap<String, String>,
    /// Keys and `[]` from the root of the result to the object being selected, while a selection is compiled.
    pub path_stack: Vec<String>,
    /// Encrypted values the selections compiled so far put in the result.
    pub enc_paths: Vec<aip_plan::DecryptPath>,
}

pub fn duration_sql(d: &ir::Dur) -> String {
    use ir::DurUnit::*;
    let unit = match d.unit {
        Second => "seconds",
        Minute => "minutes",
        Hour => "hours",
        Day => "days",
        Week => "weeks",
        Month => "months",
        Year => "years",
    };
    format!("interval '{} {unit}'", d.n)
}

impl<'x> Compiler<'x> {
    pub fn new(core: &'x ir::Program, s: &'x Schema, at: &str) -> Self {
        Compiler {
            core,
            s,
            p: Params,
            scopes: vec![HashMap::new()],
            rows: Vec::new(),
            counter: 0,
            apply_visibility: false,
            self_row: None,
            group_context: false,
            via_alias: None,
            depth: 0,
            errors: Vec::new(),
            at: at.to_string(),
            tenant: None,
            search_ranks: HashMap::new(),
            path_stack: Vec::new(),
            enc_paths: Vec::new(),
        }
    }

    pub fn fresh_alias(&mut self) -> String {
        self.counter += 1;
        format!("t{}", self.counter)
    }

    pub fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn pop(&mut self) {
        self.scopes.pop();
    }

    pub fn bind(&mut self, name: &str, v: Val) {
        if let Some(s) = self.scopes.last_mut() {
            s.insert(name.to_string(), v);
        }
    }

    pub fn push_row(&mut self, v: Val) {
        self.rows.push(v);
    }

    pub fn pop_row(&mut self) {
        self.rows.pop();
    }

    pub fn set_self_row(&mut self, v: Option<Val>) {
        self.self_row = v;
    }

    fn lookup(&self, name: &str) -> Option<Val> {
        self.scopes.iter().rev().find_map(|s| s.get(name).cloned())
    }

    fn fail(&mut self, msg: impl Into<String>) -> Val {
        self.fail_code(codes::E600, msg)
    }

    fn fail_code(&mut self, code: &str, msg: impl Into<String>) -> Val {
        self.errors.push(Diagnostic {
            severity: Severity::Error,
            code: code.into(),
            message: msg.into(),
            path: self.at.clone(),
            line: 0,
            col: 0,
        });
        Val::Scalar { sql: "NULL".into(), ty: Ty::Unknown }
    }

    fn ty(&self, e: &Expr) -> Ty {
        Ty::from_ir(&e.ty)
    }

    /// Whether `entity.field` is stored encrypted.
    pub fn is_encrypted(&self, entity: &str, field: &str) -> bool {
        self.core.entities.get(entity).is_some_and(|e| e.fields.iter().any(|f| f.name == field && f.encrypted))
    }

    /// Reading an encrypted column anywhere but a selection would hand the database's ciphertext to an expression.
    /// The semantic check (`AIP-E319`) normally stops such a program first; this keeps another frontend honest.
    fn refuse_encrypted(&mut self, entity: &str, field: &str) -> Option<Val> {
        if !self.is_encrypted(entity, field) {
            return None;
        }
        Some(self.fail_code(codes::E319, format!("{entity}.{field} is encrypted and cannot be used in an expression")))
    }

    pub fn actor_entity(&self) -> Option<&str> {
        self.core.actor.as_ref().map(|a| a.entity.as_str())
    }

    // ---------- value helpers ----------

    /// SQL for a single value (ids for entities).
    pub fn scalar(&mut self, v: &Val) -> String {
        match v {
            Val::Row { alias, .. } => format!("{alias}.\"id\""),
            Val::Id { sql, .. } => sql.clone(),
            Val::Scalar { sql, .. } => sql.clone(),
            Val::Json { sql, .. } => sql.clone(),
            Val::JsonArray { sql, .. } => sql.clone(),
            Val::Snap { id, .. } => id.clone(),
            Val::IdSet { sql, .. } | Val::ScalarSet { sql, .. } => format!("(SELECT min(x::text) FROM ({sql}) x)"),
            Val::Inverse { entity, via_col, parent } => {
                let t = self.s.table(entity).table.clone();
                format!("(SELECT min(i.\"id\"::text) FROM {} i WHERE i.{} = {parent})", q(&t), q(via_col))
            }
        }
    }

    pub fn sql(&mut self, e: &Expr) -> String {
        let v = self.expr(e);
        self.scalar(&v)
    }

    /// Scalar SQL for a value that is about to become JSON. Decimal and Money go out as text so the
    /// JSON carries a string: a JSON number would pass through a double in the next reader.
    pub fn json_sql(&mut self, e: &Expr) -> String {
        let v = self.expr(e);
        match &v {
            Val::Scalar { sql, ty: Ty::Decimal | Ty::Money(_) } => format!("(({sql})::text)"),
            _ => self.scalar(&v),
        }
    }

    /// A predicate that is never NULL.
    pub fn pred(&mut self, e: &Expr) -> String {
        let s = self.sql(e);
        format!("coalesce(({s}), false)")
    }

    fn table_of(&self, entity: &str) -> String {
        q(&self.s.table(entity).table)
    }

    /// Field access on any value.
    pub fn field(&mut self, base: Val, f: &str) -> Val {
        match base {
            Val::Row { entity, alias } => self.entity_field(&entity, &alias.to_string(), true, f),
            Val::Id { entity, sql } => self.entity_field(&entity, &sql, false, f),
            Val::Snap { entity, id, version } => {
                if let Some(v) = self.refuse_encrypted(&entity, f) {
                    return v;
                }
                let col = match self.s.table(&entity).col(f) {
                    Some(Col::Scalar { col, ty }) => (col.clone(), ty.clone()),
                    _ => return self.fail(format!("snapshot field '{f}' is not a plain value")),
                };
                let json = format!(
                    "(SELECT h.\"data\" -> {} FROM {} h WHERE h.\"id\" = {id} AND h.\"version\" = {version})",
                    lit(&col.0),
                    q(&self.s.table(&entity).history_table)
                );
                match col.1 {
                    Ty::Coll(_) | Ty::Json | Ty::Record(_) => Val::JsonArray { sql: json, elem: Ty::Unknown },
                    ty => Val::Scalar { sql: format!("(({json}) #>> '{{}}')::{}", cast(&ty)), ty },
                }
            }
            Val::Json { sql, record } => {
                let fty = record.as_deref().and_then(|r| self.record_field_ty(r, f)).unwrap_or(Ty::Unknown);
                json_field(&sql, f, &fty)
            }
            Val::Scalar { sql, ty: Ty::Range(inner) } => match f {
                "start" => Val::Scalar { sql: format!("lower({sql})"), ty: *inner },
                "end" => Val::Scalar { sql: format!("upper({sql})"), ty: *inner },
                _ => self.fail(format!("ranges have 'start' and 'end', not '{f}'")),
            },
            Val::Scalar { sql, ty: Ty::Json } => Val::Json { sql: format!("({sql} -> {})", lit(f)), record: None },
            Val::IdSet { entity, sql } => {
                if let Some(v) = self.refuse_encrypted(&entity, f) {
                    return v;
                }
                // map a field over a set: `members.member`
                let t = self.table_of(&entity);
                match self.s.table(&entity).col(f).cloned() {
                    Some(Col::Ref { col, target }) => {
                        Val::IdSet { entity: target, sql: format!("SELECT m.{} FROM {t} m WHERE m.\"id\" IN ({sql})", q(&col)) }
                    }
                    Some(Col::Scalar { col, ty }) => Val::ScalarSet { sql: format!("SELECT m.{} FROM {t} m WHERE m.\"id\" IN ({sql})", q(&col)), ty },
                    _ => self.fail(format!("cannot map '{f}' over a set")),
                }
            }
            other => self.fail(format!("cannot access '.{f}' on {other:?}")),
        }
    }

    fn record_field_ty(&self, record: &str, f: &str) -> Option<Ty> {
        record_field_ty(self.core, record, f)
    }

    fn entity_field(&mut self, entity: &str, base: &str, is_alias: bool, f: &str) -> Val {
        if let Some(v) = self.refuse_encrypted(entity, f) {
            return v;
        }
        let table = self.s.table(entity).table.clone();
        let Some(col) = self.s.table(entity).col(f).cloned() else {
            return self.fail(format!("{entity} has no field '{f}'"));
        };
        let access = |this: &mut Self, col: &str| -> String {
            if is_alias {
                format!("{base}.{}", q(col))
            } else {
                let a = this.fresh_alias();
                format!("(SELECT {a}.{} FROM {} {a} WHERE {a}.\"id\" = {base})", q(col), q(&table))
            }
        };
        match col {
            Col::Scalar { col, ty } => {
                let sql = access(self, &col);
                match ty {
                    Ty::Coll(elem) => Val::JsonArray { sql, elem: *elem },
                    Ty::Record(r) => Val::Json { sql, record: Some(r) },
                    Ty::Json => Val::Scalar { sql, ty: Ty::Json },
                    ty => Val::Scalar { sql, ty },
                }
            }
            Col::Counter { col } => Val::Scalar { sql: access(self, &col), ty: Ty::Int },
            Col::Ref { col, target } => Val::Id { entity: target, sql: access(self, &col) },
            Col::Union { id_col, .. } => Val::Scalar { sql: access(self, &id_col), ty: Ty::Uuid },
            Col::Snapshot { id_col, ver_col, target } => {
                let id = access(self, &id_col);
                let version = access(self, &ver_col);
                Val::Snap { entity: target, id, version }
            }
            Col::Inverse { target, via_col } => {
                let parent = if is_alias { format!("{base}.\"id\"") } else { base.to_string() };
                Val::Inverse { entity: target, via_col, parent }
            }
        }
    }

    // ---------- set expressions ----------

    /// Compiles a set expression into `FROM ... WHERE ...` parts and binds the
    /// alias (or row) for the filter. The caller must `pop`/`pop_row` via `end_set`.
    pub fn begin_set(&mut self, se: &SetExpr) -> SetParts {
        let alias = self.fresh_alias();
        let mut conds = Vec::new();
        let (from, elem) = match &se.source {
            SetSource::Entity { entity } => (format!("{} {alias}", self.table_of(entity)), SetElem::Entity(entity.clone())),
            SetSource::Expr { expr } => {
                let v = self.expr(expr);
                match v {
                    Val::Inverse { entity, via_col, parent } => {
                        conds.push(format!("{alias}.{} = {parent}", q(&via_col)));
                        (format!("{} {alias}", self.table_of(&entity)), SetElem::Entity(entity))
                    }
                    Val::IdSet { entity, sql } => {
                        conds.push(format!("{alias}.\"id\" IN ({sql})"));
                        (format!("{} {alias}", self.table_of(&entity)), SetElem::Entity(entity))
                    }
                    Val::Id { entity, sql } => {
                        conds.push(format!("{alias}.\"id\" = {sql}"));
                        (format!("{} {alias}", self.table_of(&entity)), SetElem::Entity(entity))
                    }
                    Val::Row { entity, alias: other } => {
                        conds.push(format!("{alias}.\"id\" = {other}.\"id\""));
                        (format!("{} {alias}", self.table_of(&entity)), SetElem::Entity(entity))
                    }
                    Val::JsonArray { sql, elem } => {
                        let record = match &elem {
                            Ty::Record(r) => Some(r.clone()),
                            _ => None,
                        };
                        (format!("jsonb_array_elements(coalesce({sql}, '[]'::jsonb)) {alias}(v)"), SetElem::Json(record, elem))
                    }
                    Val::ScalarSet { sql, ty } => (format!("({sql}) {alias}(v)"), SetElem::Scalar(ty)),
                    other => {
                        self.fail(format!("not a set: {other:?}"));
                        ("(SELECT NULL::uuid AS id WHERE false) x".into(), SetElem::Scalar(Ty::Unknown))
                    }
                }
            }
        };
        if let SetElem::Entity(e) = &elem
            && self.s.table(e).soft_delete
        {
            conds.push(format!("{alias}.\"deleted_at\" IS NULL"));
        }
        if let (SetElem::Entity(e), Some(ts)) = (&elem, &self.tenant)
            && aip_ir::tenant::scoped(self.core, e)
        {
            let checked: Vec<&ir::Param> = ts.checked.iter().collect();
            let bound = matches!(&se.source, SetSource::Expr { expr } if aip_ir::tenant::bound(self.core, &checked, expr));
            if !bound && let Some(row) = crate::tenant::row_sql(self.core, self.s, e, &alias) {
                conds.push(format!("{row} = {}", ts.sql));
            }
        }
        let row = match &elem {
            SetElem::Entity(e) => Val::Row { entity: e.clone(), alias: alias.clone() },
            SetElem::Json(record, _) => Val::Json { sql: format!("{alias}.v"), record: record.clone() },
            SetElem::Scalar(ty) => Val::Scalar { sql: format!("{alias}.v"), ty: ty.clone() },
        };
        self.push();
        match &se.alias {
            Some(a) => {
                self.bind(a, row.clone());
                self.rows.push(Val::Scalar { sql: "NULL".into(), ty: Ty::Unknown });
            }
            None => self.rows.push(row.clone()),
        }
        if let (SetElem::Entity(e), true) = (&elem, self.apply_visibility)
            && let Some(v) = self.visibility_sql(e, &row)
        {
            conds.push(v);
        }
        if let Some(f) = &se.filter {
            let c = self.pred(f);
            conds.push(c);
        }
        SetParts { from, conds, alias, elem, row }
    }

    pub fn end_set(&mut self) {
        self.rows.pop();
        self.pop();
    }

    pub fn where_sql(conds: &[String]) -> String {
        if conds.is_empty() { String::new() } else { format!(" WHERE {}", conds.join(" AND ")) }
    }

    /// `visible to actor when ...` for a row, with the superuser bypass.
    pub fn visibility_sql(&mut self, entity: &str, row: &Val) -> Option<String> {
        let core = self.core;
        let vis = core.entities.get(entity)?.visibility.as_ref()?;
        let saved = self.apply_visibility;
        self.apply_visibility = false; // policies themselves read without visibility
        self.push();
        self.rows.push(row.clone());
        let c = self.pred(&vis.cond);
        self.rows.pop();
        self.pop();
        self.apply_visibility = saved;
        let c = if vis.unless { format!("NOT {c}") } else { c };
        Some(match self.superuser_sql() {
            Some(su) => format!("({c} OR {su})"),
            None => c,
        })
    }

    pub fn superuser_sql(&mut self) -> Option<String> {
        let core = self.core;
        let actor = core.actor.as_ref()?;
        let cond = actor.superuser.as_ref()?;
        let row = Val::Id { entity: actor.entity.clone(), sql: self.p.typed(ACTOR, &Ty::Uuid) };
        self.push();
        self.rows.push(row);
        let c = self.pred(cond);
        self.rows.pop();
        self.pop();
        // someone acting as a superuser is still the operator: the bypass is not theirs to lend
        if aip_ir::form_intents::impersonation(core).is_some() {
            return Some(format!("({c} AND ({}::text) IS NULL)", marker(IMPERSONATION)));
        }
        Some(c)
    }

    /// A condition over a row given as a value (the row a call loaded), as entity-level conditions are written.
    pub fn row_pred(&mut self, row: Val, cond: &Expr) -> String {
        self.push();
        self.rows.push(row);
        let c = self.pred(cond);
        self.rows.pop();
        self.pop();
        c
    }

    // ---------- expressions ----------

    pub fn expr(&mut self, e: &Expr) -> Val {
        self.depth += 1;
        if self.depth > 64 {
            self.depth -= 1;
            return self.fail("expression nests too deeply (recursive relation?)");
        }
        let v = self.expr_inner(e);
        self.depth -= 1;
        v
    }

    /// A bare name: a binding first, then a field of the innermost row that has one.
    fn name_value(&mut self, name: &str, e: &Expr) -> Val {
        if let Some(v) = self.lookup(name) {
            return v;
        }
        for i in (0..self.rows.len()).rev() {
            let row = self.rows[i].clone();
            let has = match &row {
                Val::Row { entity, .. } | Val::Id { entity, .. } => self.s.table(entity).col(name).is_some(),
                Val::Json { record: Some(r), .. } => self.record_field_ty(r, name).is_some(),
                _ => false,
            };
            if has {
                return self.field(row, name);
            }
        }
        match self.ty(e) {
            Ty::Enum(_) => Val::Scalar { sql: lit(name), ty: self.ty(e) },
            _ => self.fail(format!("cannot compile name '{name}'")),
        }
    }

    fn expr_inner(&mut self, e: &Expr) -> Val {
        let sc = |sql: String, ty: Ty| Val::Scalar { sql, ty };
        match &e.node {
            Node::Lit { lit: l } => match l {
                Literal::Int(n) => sc(n.to_string(), Ty::Int),
                Literal::Decimal(d) => sc(d.clone(), Ty::Decimal),
                Literal::Text(s) => sc(lit(s), Ty::Text),
                Literal::Bool(b) => sc(if *b { "TRUE".into() } else { "FALSE".into() }, Ty::Bool),
                Literal::Null => sc("NULL".into(), Ty::Null),
                Literal::Duration(d) => sc(duration_sql(d), Ty::Duration),
                Literal::SizeBytes(n) => sc(n.to_string(), Ty::Int),
                Literal::TimeOfDay(h, m) => sc(format!("time '{h:02}:{m:02}'"), Ty::Time),
            },
            Node::Param { name } | Node::Local { name } => self.name_value(name, e),
            Node::RowField { field, .. } => self.name_value(field, e),
            Node::RecordField { field, .. } => self.name_value(field, e),
            Node::Symbol { name } => sc(lit(name), Ty::Text),
            Node::EnumValue { value, .. } => sc(lit(value), self.ty(e)),
            Node::Config { name } => self.fail(format!("cannot compile name '{name}'")),
            Node::Actor => match self.actor_entity().map(String::from) {
                Some(ent) => Val::Id { entity: ent, sql: self.p.typed(ACTOR, &Ty::Uuid) },
                None => self.fail("no actor"),
            },
            Node::SelfRow => match self.self_row.clone() {
                Some(row) => {
                    let id = self.scalar(&row);
                    let actor = self.p.typed(ACTOR, &Ty::Uuid);
                    sc(format!("({id} = {actor})"), Ty::Bool)
                }
                None => self.fail("'self' outside field visibility"),
            },
            Node::This => match self.rows.last().cloned() {
                Some(r) => r,
                None => self.fail("'this' outside an entity"),
            },
            Node::SigningSecret { base } => {
                let v = self.expr(base);
                let entity = match &v {
                    Val::Id { entity, .. } | Val::Row { entity, .. } => entity.clone(),
                    _ => return self.fail("'signingSecret' needs an endpoint row"),
                };
                let id = self.scalar(&v);
                // the runtime derives the same value (`aip_runtime::outbound::signing_secret`) to sign deliveries, so nothing is stored
                let key = typed_marker("__secret", &Ty::Text);
                sc(format!("('whsec_' || encode(hmac('aip-outbound-webhook:{entity}:' || ({id})::text, {key}, 'sha256'), 'hex'))"), Ty::Text)
            }
            Node::SearchRank { alias } => match self.search_ranks.get(alias).cloned() {
                Some(rank) => sc(rank, Ty::Decimal),
                None => self.fail(format!("'{alias}.rank' outside the query that searches")),
            },
            Node::Now => sc("now()".into(), Ty::Time),
            Node::Today => sc("current_date".into(), Ty::Date),
            Node::Public => sc("TRUE".into(), Ty::Bool),
            Node::Authenticated => {
                let a = self.p.typed(ACTOR, &Ty::Uuid);
                sc(format!("({a} IS NOT NULL)"), Ty::Bool)
            }
            Node::Field { base, field, .. } => {
                let b = self.expr(base);
                self.field(b, field)
            }
            Node::Call(c) => self.call(c, e),
            Node::Unary { op: UnOp::Neg, arg } => {
                let s = self.sql(arg);
                sc(format!("(-{s})"), self.ty(e))
            }
            Node::Unary { op: UnOp::Not, arg } => {
                let s = self.pred(arg);
                sc(format!("(NOT {s})"), Ty::Bool)
            }
            Node::Binary { op, l, r } => self.binary(*op, l, r, e),
            Node::InList { value, list } => {
                let xs = self.sql(value);
                let parts: Vec<String> = list.iter().map(|i| self.sql(i)).collect();
                sc(format!("({xs} IN ({}))", parts.join(", ")), Ty::Bool)
            }
            Node::InRange { value, lo, hi } => {
                let xs = self.sql(value);
                let a = self.sql(lo);
                let b = self.sql(hi);
                sc(format!("({xs} >= {a} AND {xs} < {b})"), Ty::Bool)
            }
            Node::InSet { value, set: r } | Node::InRangeValue { value, range: r } => {
                let xs = self.sql(value);
                match self.expr(r) {
                    Val::IdSet { sql, .. } | Val::ScalarSet { sql, .. } => sc(format!("({xs} IN ({sql}))"), Ty::Bool),
                    Val::Inverse { entity, via_col, parent } => {
                        let t = self.table_of(&entity);
                        sc(format!("({xs} IN (SELECT i.\"id\" FROM {t} i WHERE i.{} = {parent}))", q(&via_col)), Ty::Bool)
                    }
                    // List filters: an empty list means "no restriction"
                    Val::JsonArray { sql, .. } => sc(
                        format!("(coalesce(jsonb_array_length({sql}), 0) = 0 OR ({xs})::text IN (SELECT jsonb_array_elements_text({sql})))"),
                        Ty::Bool,
                    ),
                    Val::Scalar { sql, ty: Ty::Range(_) } => sc(format!("({sql} @> {xs})"), Ty::Bool),
                    other => self.fail(format!("right side of 'in' is not a collection: {other:?}")),
                }
            }
            Node::TypeTest { value, entity: name } => {
                let v = self.expr(value);
                let entity = match &v {
                    Val::Row { entity, .. } | Val::Id { entity, .. } => entity.clone(),
                    _ => return self.fail("'is' on a non-row"),
                };
                if Some(entity.as_str()) == self.actor_entity() && self.core.entities.contains_key(name) {
                    let s = self.scalar(&v);
                    return sc(format!("({s} IS NOT NULL)"), Ty::Bool);
                }
                self.predicate(v, &entity, name)
            }
            Node::Pred { name, row, .. } => {
                let v = self.expr(row);
                let entity = match &v {
                    Val::Row { entity, .. } | Val::Id { entity, .. } => entity.clone(),
                    _ => return self.fail("'is' on a non-row"),
                };
                self.predicate(v, &entity, name)
            }
            Node::HasScope { scope, .. } => {
                let p = self.p.bind("__scopes");
                sc(format!("({} = ANY(string_to_array(({p}::text), ',')))", lit(scope)), Ty::Bool)
            }
            Node::Exists { set } => {
                let parts = self.begin_set(set);
                self.end_set();
                sc(format!("EXISTS (SELECT 1 FROM {}{})", parts.from, Self::where_sql(&parts.conds)), Ty::Bool)
            }
            Node::The { set } => {
                let parts = self.begin_set(set);
                self.end_set();
                parts.first_value(None)
            }
            Node::Latest { set, by } => {
                let parts = self.begin_set(set);
                let order = self.sql(by);
                self.end_set();
                parts.first_value(Some(format!("{order} DESC NULLS LAST")))
            }
            Node::First { set, order: keys } => {
                let parts = self.begin_set(set);
                let order: Vec<String> = keys
                    .iter()
                    .map(|k| {
                        let s = self.sort_key_sql(&k.expr);
                        format!("{s} {}", if k.desc { "DESC NULLS LAST" } else { "ASC NULLS LAST" })
                    })
                    .collect();
                self.end_set();
                parts.first_value(Some(order.join(", ")))
            }
            Node::Agg { func, set, value: body } => {
                let Some(se) = set else {
                    return sc("count(*)".into(), Ty::Int);
                };
                if let (Some(via), None, None, None) = (&self.via_alias, &se.alias, &se.filter, body)
                    && let SetSource::Expr { expr: source } = &se.source
                    && root_name(source).as_deref() == Some(via.as_str())
                {
                    let v = self.sql(source);
                    let f = match func {
                        AggFn::Count => "count",
                        AggFn::Sum => "sum",
                        AggFn::Min => "min",
                        AggFn::Max => "max",
                        AggFn::Avg => "avg",
                    };
                    return sc(format!("{f}({v})"), self.ty(e));
                }
                let parts = self.begin_set(se);
                let b = body.as_ref().map(|b| self.sql(b));
                self.end_set();
                let value = b.unwrap_or_else(|| match &parts.elem {
                    SetElem::Entity(_) => format!("{}.\"id\"", parts.alias),
                    _ => format!("{}.v", parts.alias),
                });
                let (agg, zero) = match func {
                    AggFn::Count => ("count".to_string(), "0"),
                    AggFn::Sum => ("sum".to_string(), "0"),
                    AggFn::Min => ("min".to_string(), "NULL"),
                    AggFn::Max => ("max".to_string(), "NULL"),
                    AggFn::Avg => ("avg".to_string(), "NULL"),
                };
                let inner = if *func == AggFn::Count { "count(*)".to_string() } else { format!("{agg}({value})") };
                sc(format!("(SELECT coalesce({inner}, {zero}) FROM {}{})", parts.from, Self::where_sql(&parts.conds)), self.ty(e))
            }
            Node::Quant { all, set, body } => {
                let parts = self.begin_set(set);
                let b = self.pred(body);
                self.end_set();
                let mut conds = parts.conds.clone();
                if *all {
                    conds.push(format!("NOT {b}"));
                    sc(format!("(NOT EXISTS (SELECT 1 FROM {}{}))", parts.from, Self::where_sql(&conds)), Ty::Bool)
                } else {
                    conds.push(b);
                    sc(format!("EXISTS (SELECT 1 FROM {}{})", parts.from, Self::where_sql(&conds)), Ty::Bool)
                }
            }
            Node::RunningSum { value, over, order } => {
                let v = self.sql(value);
                let o = self.sql(order);
                let over_v = self.name_value(over, e);
                let part = self.scalar(&over_v);
                sc(format!("sum({v}) OVER (PARTITION BY {part} ORDER BY {o} ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)"), self.ty(e))
            }
            Node::If { cond, then, otherwise } => {
                let cs = self.pred(cond);
                let ts = self.sql(then);
                let fs = self.sql(otherwise);
                let ty = self.ty(e);
                match &ty {
                    Ty::Entity(ent) => Val::Id { entity: ent.clone(), sql: format!("(CASE WHEN {cs} THEN {ts} ELSE {fs} END)") },
                    _ => sc(format!("(CASE WHEN {cs} THEN {ts} ELSE {fs} END)"), ty),
                }
            }
            Node::List { items } => {
                let parts: Vec<String> = items.iter().map(|i| self.sql(i)).collect();
                let elem = match self.ty(e) {
                    Ty::Coll(t) => *t,
                    _ => Ty::Unknown,
                };
                Val::JsonArray { sql: if parts.is_empty() { "'[]'::jsonb".into() } else { format!("jsonb_build_array({})", parts.join(", ")) }, elem }
            }
            Node::Binder { .. } => self.fail("binder arguments are only valid in same(...)"),
        }
    }

    /// `row is <predicate>`: the predicate's body evaluated with `row` as the implicit row.
    fn predicate(&mut self, v: Val, entity: &str, name: &str) -> Val {
        let core = self.core;
        let Some(body) = core.entities.get(entity).and_then(|i| i.predicates.get(name)) else {
            return self.fail(format!("no predicate '{name}'"));
        };
        self.push();
        self.rows.push(v);
        let sql = self.pred(body);
        self.rows.pop();
        self.pop();
        Val::Scalar { sql, ty: Ty::Bool }
    }

    fn sort_key_sql(&mut self, e: &Expr) -> String {
        // ordered enums sort by rank, not alphabetically
        let s = self.sql(e);
        match self.ty(e) {
            Ty::Enum(en) => self.enum_rank(&en, &s).unwrap_or(s),
            _ => s,
        }
    }

    pub fn enum_rank(&self, en: &str, sql: &str) -> Option<String> {
        let d = self.core.enums.get(en)?;
        if !d.ordered {
            return None;
        }
        let vals: Vec<String> = d.values.iter().map(|v| lit(v)).collect();
        Some(format!("array_position(ARRAY[{}]::text[], ({sql})::text)", vals.join(", ")))
    }

    fn binary(&mut self, op: BinOp, l: &Expr, r: &Expr, e: &Expr) -> Val {
        use BinOp::*;
        let sc = |sql: String, ty: Ty| Val::Scalar { sql, ty };
        match op {
            And | Or => {
                let a = self.pred(l);
                let b = self.pred(r);
                sc(format!("({a} {} {b})", if op == And { "AND" } else { "OR" }), Ty::Bool)
            }
            Eq | Ne => {
                let is_null = |x: &Expr| matches!(x.node, Node::Lit { lit: Literal::Null });
                if is_null(l) || is_null(r) {
                    let other = if is_null(l) { r } else { l };
                    let s = self.sql(other);
                    return sc(format!("({s} IS {}NULL)", if op == Eq { "" } else { "NOT " }), Ty::Bool);
                }
                let a = self.sql(l);
                let b = self.sql(r);
                if op == Eq { sc(format!("({a} = {b})"), Ty::Bool) } else { sc(format!("({a} IS DISTINCT FROM {b})"), Ty::Bool) }
            }
            Lt | Le | Gt | Ge => {
                let lt = self.ty(l);
                let rt = self.ty(r);
                let mut a = self.sql(l);
                let mut b = self.sql(r);
                let en = match (&lt, &rt) {
                    (Ty::Enum(x), _) | (_, Ty::Enum(x)) => Some(x.clone()),
                    _ => None,
                };
                if let Some(en) = en
                    && let (Some(ra), Some(rb)) = (self.enum_rank(&en, &a), self.enum_rank(&en, &b))
                {
                    a = ra;
                    b = rb;
                }
                let o = match op {
                    Lt => "<",
                    Le => "<=",
                    Gt => ">",
                    _ => ">=",
                };
                sc(format!("({a} {o} {b})"), Ty::Bool)
            }
            Add | Sub | Mul | Div => {
                let a = self.sql(l);
                let b = self.sql(r);
                let o = match op {
                    Add => "+",
                    Sub => "-",
                    Mul => "*",
                    _ => "/",
                };
                let expr = if op == Div { format!("({a}::numeric / nullif({b}, 0))") } else { format!("({a} {o} {b})") };
                sc(expr, self.ty(e))
            }
        }
    }

    fn call(&mut self, c: &ir::Call, whole: &Expr) -> Val {
        use ir::Callee;
        let core = self.core;
        let (Callee::Fn { name } | Callee::Relation { name } | Callee::Builtin { name } | Callee::Ext { name } | Callee::Intent { name }) = &c.callee;
        if let Callee::Relation { .. } = &c.callee
            && let Some(rel) = core.relations.get(name)
        {
            let args: Vec<Val> = c.args.iter().map(|a| self.expr(&a.value)).collect();
            self.push();
            // relation bodies are evaluated without the caller's rows or visibility
            let saved_rows = std::mem::take(&mut self.rows);
            let saved_vis = self.apply_visibility;
            self.apply_visibility = false;
            for (p, v) in rel.params.iter().zip(args) {
                self.bind(&p.name, v);
            }
            let v = self.expr(&rel.body);
            self.rows = saved_rows;
            self.apply_visibility = saved_vis;
            self.pop();
            return v;
        }
        if let Callee::Fn { .. } = &c.callee
            && let Some(f) = core.fns.get(name)
        {
            let ir::FnBody::Expr { expr: body } = &f.body else {
                return self.fail(format!("wasm function '{name}' cannot run inside SQL"));
            };
            let args: Vec<Val> = c.args.iter().map(|a| self.expr(&a.value)).collect();
            self.push();
            let saved_rows = std::mem::take(&mut self.rows);
            for (p, v) in f.params.iter().zip(args) {
                self.bind(&p.name, v);
            }
            let v = self.expr(body);
            self.rows = saved_rows;
            self.pop();
            return v;
        }
        let sc = |sql: String, ty: Ty| Val::Scalar { sql, ty };
        let arg = |this: &mut Self, i: usize| -> String {
            match c.args.get(i) {
                Some(a) => this.sql(&a.value),
                None => "NULL".into(),
            }
        };
        match name.as_str() {
            "same" => {
                let Some(ir::Arg { value: Expr { node: Node::Binder { set, body }, .. }, .. }) = c.args.first() else {
                    return self.fail("same() needs a binder");
                };
                let parts = self.begin_set(set);
                let b = self.sql(body);
                self.end_set();
                let ty = self.ty(whole);
                let value = format!(
                    "(SELECT CASE WHEN count(DISTINCT ({b})) = 1 AND count(*) = count({b}) THEN min(({b})::text) END FROM {}{})",
                    parts.from,
                    Self::where_sql(&parts.conds)
                );
                match &ty {
                    Ty::Entity(ent) => Val::Id { entity: ent.clone(), sql: format!("({value})::uuid") },
                    t => sc(format!("({value})::{}", cast(t)), ty.clone()),
                }
            }
            "domain" => {
                let a = arg(self, 0);
                sc(format!("lower(split_part({a}, '@', 2))"), Ty::Text)
            }
            "days_until" => {
                let a = arg(self, 0);
                sc(format!("(({a})::date - current_date)"), Ty::Int)
            }
            "contains" => {
                let a = arg(self, 0);
                let b = arg(self, 1);
                sc(format!("(strpos(lower({a}), lower({b})) > 0)"), Ty::Bool)
            }
            "starts_with" => {
                let a = arg(self, 0);
                let b = arg(self, 1);
                sc(format!("starts_with({a}, {b})"), Ty::Bool)
            }
            "lower" | "upper" | "trim" | "length" => {
                let a = arg(self, 0);
                sc(format!("{name}({a})"), self.ty(whole))
            }
            "slugify" => {
                let a = arg(self, 0);
                sc(format!("trim(both '-' from regexp_replace(lower({a}), '[^a-z0-9가-힣]+', '-', 'g'))"), Ty::Text)
            }
            "coalesce" => {
                let parts: Vec<String> = c.args.iter().map(|a| self.sql(&a.value)).collect();
                sc(format!("coalesce({})", parts.join(", ")), self.ty(whole))
            }
            "snapshot" => {
                let v = c.args.first().map(|a| self.expr(&a.value));
                let target = match v {
                    Some(Val::Id { entity, sql }) => Some((entity, sql)),
                    Some(Val::Row { entity, alias }) => Some((entity, format!("{alias}.\"id\""))),
                    _ => None,
                };
                match target {
                    Some((entity, id)) => {
                        let t = self.table_of(&entity);
                        let a = self.fresh_alias();
                        Val::Snap {
                            entity: entity.clone(),
                            id: id.clone(),
                            version: format!("(SELECT {a}.\"version\" FROM {t} {a} WHERE {a}.\"id\" = {id})"),
                        }
                    }
                    None => self.fail("snapshot() needs a row"),
                }
            }
            "flag" => {
                let n = match c.args.first().map(|a| &a.value.node) {
                    Some(Node::Config { name }) | Some(Node::Local { name }) | Some(Node::Param { name }) => name.clone(),
                    _ => return self.fail("flag(name)"),
                };
                let p = self.p.bind(&format!("__flag.{n}"));
                sc(format!("coalesce(({p}::text)::boolean, false)"), Ty::Bool)
            }
            _ => self.fail(format!("'{name}' cannot be evaluated inside the database")),
        }
    }
}

#[derive(Debug, Clone)]
pub enum SetElem {
    Entity(String),
    Json(Option<String>, Ty),
    Scalar(Ty),
}

#[derive(Debug, Clone)]
pub struct SetParts {
    pub from: String,
    pub conds: Vec<String>,
    pub alias: String,
    pub elem: SetElem,
    pub row: Val,
}

impl SetParts {
    fn first_value(&self, order: Option<String>) -> Val {
        let ord = order.map(|o| format!(" ORDER BY {o}")).unwrap_or_default();
        let w = Compiler::where_sql(&self.conds);
        match &self.elem {
            SetElem::Entity(e) => Val::Id { entity: e.clone(), sql: format!("(SELECT {}.\"id\" FROM {}{w}{ord} LIMIT 1)", self.alias, self.from) },
            SetElem::Json(r, _) => Val::Json { sql: format!("(SELECT {}.v FROM {}{w}{ord} LIMIT 1)", self.alias, self.from), record: r.clone() },
            SetElem::Scalar(t) => Val::Scalar { sql: format!("(SELECT {}.v FROM {}{w}{ord} LIMIT 1)", self.alias, self.from), ty: t.clone() },
        }
    }
}

fn json_field(sql: &str, f: &str, ty: &Ty) -> Val {
    match ty {
        Ty::Record(r) => Val::Json { sql: format!("({sql} -> {})", lit(f)), record: Some(r.clone()) },
        Ty::Coll(elem) => Val::JsonArray { sql: format!("({sql} -> {})", lit(f)), elem: (**elem).clone() },
        Ty::Json | Ty::Unknown => Val::Scalar { sql: format!("({sql} -> {})", lit(f)), ty: Ty::Json },
        Ty::Entity(e) => Val::Id { entity: e.clone(), sql: format!("(({sql} ->> {}))::uuid", lit(f)) },
        Ty::Range(_) => Val::Scalar { sql: format!("(({sql} ->> {}))::{}", lit(f), cast(ty)), ty: ty.clone() },
        other => Val::Scalar { sql: format!("(({sql} ->> {}))::{}", lit(f), cast(other)), ty: other.clone() },
    }
}

/// Type of field `f` of a record or of the event payload `event:<Name>`.
pub fn record_field_ty(core: &ir::Program, record: &str, f: &str) -> Option<Ty> {
    if let Some(ev) = record.strip_prefix("event:") {
        return core.events.get(ev)?.fields.iter().find(|(n, _)| n == f).map(|(_, t)| Ty::from_ir(t));
    }
    let rec = core.records.get(record)?;
    rec.fields.iter().find(|x| x.name == f).map(|x| Ty::from_ir(&x.ty))
}

pub fn root_name(e: &Expr) -> Option<String> {
    match &e.node {
        Node::Local { name } | Node::Param { name } => Some(name.clone()),
        Node::Field { base, .. } => root_name(base),
        _ => None,
    }
}
