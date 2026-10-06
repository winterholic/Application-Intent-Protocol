//! Structural validation of Core IR, independent of the frontend that
//! produced it: a program from a TS or Python frontend (or written by a
//! tool) gets the same checks as one lowered from `.aip`.
//!
//! Codes:
//! - `AIP-I101` version tag differs from [`CORE_IR_VERSION`]
//! - `AIP-I102` type names an enum that does not exist
//! - `AIP-I103` type names a record that does not exist
//! - `AIP-I104` type names an entity that does not exist (`Ref`, `RefUnion`, `Snapshot`)
//! - `AIP-I105` `Set`/`List` bound is zero
//! - `AIP-I106` reference field targets an entity that does not exist
//! - `AIP-I107` inverse field targets a missing entity or a missing `via` field
//! - `AIP-I108` `Field` expression names a missing entity or field
//! - `AIP-I109` `RowField` names a missing entity or field
//! - `AIP-I110` `EnumValue` names a missing enum or value
//! - `AIP-I111` callee names a missing fn, relation or intent
//! - `AIP-I112` insert/upsert/toggle names a missing entity or field
//! - `AIP-I113` lifecycle names a missing field, a non-enum field or a missing enum value
//! - `AIP-I114` reaction, emit, projection, upcast or event type names a missing event
//! - `AIP-I115` predicate application names a missing entity or predicate
//! - `AIP-I118` `RecordField` names a missing record or field
//! - `AIP-I119` counter `store` is not an extension declared in `uses`
//! - `AIP-I116` an entity reference (actor, source, rule, retain, type test, ...) names a missing entity
//! - `AIP-I117` constraint names a missing field
//! - `AIP-I120` two constraints of one entity share an ordinal
//! - `AIP-I121` `Json validated by` names a field the entity does not have
//! - `AIP-I122` a `Search` source names a search that does not exist, or a `SearchRank` names an alias no `Search` source binds

use crate::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrDiagnostic {
    /// Stable code: `AIP-I1xx` from [`validate`], the registered `AIP-E`/`AIP-W` code of a semantic rule from [`crate::analyze::analyze`].
    pub code: String,
    /// IR path of the offending node (`intents.ApplyClub.body[2]`).
    pub path: String,
    pub message: String,
    /// How to repair it; semantic rules carry the same help text the frontend used to print.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// IR path of a second node the message refers to (`than at <path>`); a consumer with a source map may print its line instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related: Option<String>,
}

impl IrDiagnostic {
    /// Warnings are the registry's `AIP-W` codes; everything else stops compilation.
    pub fn is_warning(&self) -> bool {
        codes::lookup(&self.code).is_some_and(|c| c.severity == codes::Severity::Warning)
    }
}

pub fn validate(p: &Program) -> Vec<IrDiagnostic> {
    let mut v = V { p, out: Vec::new(), search_aliases: Vec::new() };
    v.program();
    v.out
}

struct V<'p> {
    p: &'p Program,
    out: Vec<IrDiagnostic>,
    /// Aliases the `Search` source of the query being checked binds; `SearchRank` is only valid under one of them.
    search_aliases: Vec<String>,
}

impl V<'_> {
    fn err(&mut self, code: &str, path: &str, message: impl Into<String>) {
        self.out.push(IrDiagnostic { code: code.to_string(), path: path.to_string(), message: message.into(), help: None, related: None });
    }

    fn has_entity(&self, name: &str) -> bool {
        self.p.entities.contains_key(name)
    }

    fn has_field(&self, entity: &str, field: &str) -> bool {
        self.p.entities.get(entity).is_some_and(|e| e.fields.iter().any(|f| f.name == field))
    }

    fn entity_ref(&mut self, name: &str, path: &str) {
        if !self.has_entity(name) {
            self.err(codes::I116, path, format!("unknown entity '{name}'"));
        }
    }

    fn event_ref(&mut self, name: &str, path: &str) {
        if !self.p.events.contains_key(name) {
            self.err(codes::I114, path, format!("unknown event '{name}'"));
        }
    }

    fn is_intent_name(&self, name: &str) -> bool {
        self.p.intents.contains_key(name)
            || self.p.forms.iter().any(|f| match f {
                Form::Subscribe(x) => x.name == name,
                Form::Job(x) => x.name == name,
                Form::Verification(x) => x.name == name,
                Form::GrantLink(x) => x.name == name,
                Form::Approval(x) => x.name == name,
                _ => false,
            })
            || self.p.reactions.iter().any(|r| matches!(r, Reaction::Webhook(w) if w.name == name))
    }

    // ---------- program ----------

    fn program(&mut self) {
        let p = self.p;
        if p.aip_core != CORE_IR_VERSION {
            self.err(codes::I101, "aip_core", format!("program is '{}', this validator reads '{CORE_IR_VERSION}'", p.aip_core));
        }
        if let Some(a) = &p.actor {
            self.entity_ref(&a.entity, "actor.entity");
            self.call(&a.provider, "actor.provider");
            if let Some(s) = &a.superuser {
                self.expr(s, "actor.superuser");
            }
        }
        for (name, r) in &p.records {
            let base = format!("records.{name}");
            for f in &r.fields {
                self.field(f, &format!("{base}.fields.{}", f.name));
            }
            for (i, c) in r.checks.iter().enumerate() {
                self.require(c, &format!("{base}.checks[{i}]"));
            }
        }
        for (name, e) in &p.entities {
            self.entity(name, e);
        }
        for (name, r) in &p.relations {
            let base = format!("relations.{name}");
            self.params(&r.params, &base);
            if let Some(res) = &r.result {
                self.entity_ref(res, &format!("{base}.result"));
            }
            self.expr(&r.body, &format!("{base}.body"));
        }
        for (name, f) in &p.fns {
            let base = format!("fns.{name}");
            self.params(&f.params, &base);
            self.ty(&f.ret, &format!("{base}.ret"));
            if let FnBody::Expr { expr } = &f.body {
                self.expr(expr, &format!("{base}.body"));
            }
        }
        for (name, e) in &p.events {
            for (f, t) in &e.fields {
                self.ty(t, &format!("events.{name}.fields.{f}"));
            }
        }
        for (name, c) in &p.configs {
            let base = format!("configs.{name}");
            self.ty(&c.ty, &format!("{base}.ty"));
            if let Some(d) = &c.default {
                self.expr(d, &format!("{base}.default"));
            }
        }
        for (name, i) in &p.intents {
            let base = format!("intents.{name}");
            match i {
                Intent::Query(q) => self.query(q, &base),
                Intent::Command(c) => self.command(c, &base),
            }
        }
        for (i, r) in p.reactions.iter().enumerate() {
            self.reaction(r, &format!("reactions[{i}]"));
        }
        for (i, f) in p.forms.iter().enumerate() {
            self.form(f, &format!("forms[{i}]"));
        }
    }

    // ---------- types ----------

    fn ty(&mut self, t: &Type, path: &str) {
        match t {
            Type::Enum { name } if !self.p.enums.contains_key(name) => self.err(codes::I102, path, format!("unknown enum '{name}'")),
            Type::Record { name } | Type::Json { schema: Some(name), .. } if !self.p.records.contains_key(name) => {
                self.err(codes::I103, path, format!("unknown record '{name}'"))
            }
            Type::Event { name } => self.event_ref(name, path),
            Type::Ref { entity } | Type::Snapshot { entity } if !self.has_entity(entity) => {
                self.err(codes::I104, path, format!("unknown entity '{entity}'"))
            }
            Type::RefUnion { entities } => {
                for e in entities {
                    if !self.has_entity(e) {
                        self.err(codes::I104, path, format!("unknown entity '{e}'"));
                    }
                }
            }
            Type::Set { of, max } | Type::List { of, max } => {
                if *max == 0 {
                    self.err(codes::I105, path, "collection bound must be greater than zero");
                }
                self.ty(of, path);
            }
            Type::Range { of } | Type::Localized { of } | Type::Many { of } => self.ty(of, path),
            _ => {}
        }
    }

    fn params(&mut self, ps: &[Param], path: &str) {
        for p in ps {
            self.ty(&p.ty, &format!("{path}.params.{}", p.name));
            if let Some(d) = &p.default {
                self.expr(d, &format!("{path}.params.{}.default", p.name));
            }
        }
    }

    // ---------- entities ----------

    fn field(&mut self, f: &Field, path: &str) {
        self.ty(&f.ty, &format!("{path}.ty"));
        if let Some(d) = &f.default {
            self.expr(d, &format!("{path}.default"));
        }
        if let Some(v) = &f.visible_to {
            self.expr(v, &format!("{path}.visible_to"));
        }
        if let Some(m) = &f.masked {
            self.expr(&m.unless, &format!("{path}.masked.unless"));
            self.call(&m.with, &format!("{path}.masked.with"));
        }
        match &f.kind {
            FieldKind::Ref { target, on_delete, on_erase } => {
                if !self.has_entity(target) {
                    self.err(codes::I106, &format!("{path}.kind"), format!("reference target '{target}' does not exist"));
                }
                for (which, pol) in [("on_delete", on_delete), ("on_erase", on_erase)] {
                    if let Some(RefPolicy::Reassign { to }) = pol {
                        self.expr(to, &format!("{path}.kind.{which}"));
                    }
                }
            }
            FieldKind::RefUnion { targets, on_delete, on_erase } => {
                for t in targets {
                    if !self.has_entity(t) {
                        self.err(codes::I106, &format!("{path}.kind"), format!("reference target '{t}' does not exist"));
                    }
                }
                for (which, pol) in [("on_delete", on_delete), ("on_erase", on_erase)] {
                    if let Some(RefPolicy::Reassign { to }) = pol {
                        self.expr(to, &format!("{path}.kind.{which}"));
                    }
                }
            }
            FieldKind::Counter { store, .. } => {
                let declared = self.p.uses.iter().any(|u| u == store || u.starts_with(&format!("{store}.")) || store.starts_with(&format!("{u}.")));
                if !declared {
                    self.err(codes::I119, &format!("{path}.kind"), format!("counter store '{store}' is not an extension declared in `uses`"));
                }
            }
            FieldKind::Inverse { target, via } => {
                if !self.has_entity(target) {
                    self.err(codes::I107, &format!("{path}.kind"), format!("inverse target '{target}' does not exist"));
                } else if !self.has_field(target, via) {
                    self.err(codes::I107, &format!("{path}.kind"), format!("inverse field '{target}.{via}' does not exist"));
                }
            }
            FieldKind::Stored | FieldKind::Implicit => {}
        }
    }

    fn entity(&mut self, name: &str, e: &Entity) {
        let base = format!("entities.{name}");
        for f in &e.fields {
            let fpath = format!("{base}.fields.{}", f.name);
            self.field(f, &fpath);
            if let Type::Json { validated_by: Some(by), .. } = &f.ty {
                let head = by.split('.').next().unwrap_or_default();
                if !e.fields.iter().any(|x| x.name == head) {
                    self.err(codes::I121, &format!("{fpath}.ty"), format!("'validated by {by}' names a field '{name}' does not have"));
                }
            }
        }
        let mut ordinals = std::collections::BTreeSet::new();
        for (i, c) in e.constraints.iter().enumerate() {
            let path = format!("{base}.constraints[{i}]");
            if !ordinals.insert(c.ordinal()) {
                self.err(codes::I120, &path, format!("ordinal {} is used by another constraint of '{name}'", c.ordinal()));
            }
            self.constraint(name, c, &path);
        }
        for (i, l) in e.lifecycles.iter().enumerate() {
            self.lifecycle(name, e, l, &format!("{base}.lifecycles[{i}]"));
        }
        if let Some(v) = &e.visibility {
            self.expr(&v.cond, &format!("{base}.visibility"));
        }
        for (pn, body) in &e.predicates {
            self.expr(body, &format!("{base}.predicates.{pn}"));
        }
        if let Some(by) = &e.traits.publish_by {
            self.expr(by, &format!("{base}.traits.publish_by"));
        }
    }

    fn lifecycle(&mut self, entity: &str, e: &Entity, l: &Lifecycle, path: &str) {
        let Some(f) = e.fields.iter().find(|f| f.name == l.field) else {
            self.err(codes::I113, path, format!("lifecycle field '{entity}.{}' does not exist", l.field));
            return;
        };
        let Type::Enum { name: en } = &f.ty else {
            self.err(codes::I113, path, format!("lifecycle field '{entity}.{}' is not an enum", l.field));
            return;
        };
        let Some(def) = self.p.enums.get(en) else { return };
        for (i, t) in l.transitions.iter().enumerate() {
            for v in t.from.iter().chain(&t.to) {
                if !def.values.contains(v) {
                    self.err(codes::I113, &format!("{path}.transitions[{i}]"), format!("'{v}' is not a value of {en}"));
                }
            }
        }
    }

    fn constraint(&mut self, entity: &str, c: &Constraint, path: &str) {
        let need = |s: &mut Self, f: &str| {
            if !s.has_field(entity, f) {
                s.err(codes::I117, path, format!("'{entity}.{f}' does not exist"));
            }
        };
        match c {
            Constraint::Unique { fields, filter, .. } => {
                for f in fields {
                    need(self, f);
                }
                if let Some(f) = filter {
                    self.expr(f, &format!("{path}.filter"));
                }
            }
            Constraint::Cardinality { filter, per, repair, .. } => {
                need(self, per);
                self.expr(filter, &format!("{path}.filter"));
                if let Some(r) = repair {
                    self.stmts(r, &format!("{path}.repair"));
                }
            }
            Constraint::NoOverlap { range, per, .. } => {
                need(self, range);
                need(self, per);
            }
            Constraint::Capacity { count, limit, .. } => {
                self.set(count, &format!("{path}.count"));
                self.expr(limit, &format!("{path}.limit"));
            }
            Constraint::Invariant { cond, .. } => self.expr(cond, &format!("{path}.cond")),
        }
    }

    // ---------- intents ----------

    fn policy(&mut self, p: &Policy, path: &str) {
        self.expr(&p.cond, &format!("{path}.cond"));
    }

    fn require(&mut self, r: &Require, path: &str) {
        if let Some(w) = &r.when {
            self.expr(w, &format!("{path}.when"));
        }
        self.expr(&r.cond, &format!("{path}.cond"));
    }

    fn let_(&mut self, l: &Let, path: &str) {
        self.expr(&l.value, &format!("{path}.{}", l.name));
    }

    fn selection(&mut self, s: &Selection, path: &str) {
        for it in &s.items {
            let p = format!("{path}.{}", it.name);
            if let Some(v) = &it.value {
                self.expr(v, &p);
            }
            if let Some(sub) = &it.sub {
                self.selection(sub, &p);
            }
        }
    }

    fn shape(&mut self, s: &Shape, path: &str) {
        match s {
            Shape::None => {}
            Shape::Value { ty, .. } => self.ty(ty, path),
            Shape::Object { fields, .. } => {
                for (n, f) in fields {
                    self.shape(f, &format!("{path}.{n}"));
                }
            }
            Shape::List { of, .. } => self.shape(of, path),
        }
    }

    fn sort_keys(&mut self, keys: &[SortKey], path: &str) {
        for (i, k) in keys.iter().enumerate() {
            self.expr(&k.expr, &format!("{path}[{i}]"));
        }
    }

    fn query_source(&mut self, s: &QuerySource, path: &str) {
        match s {
            QuerySource::Entity { entity, .. } => self.entity_ref(entity, &format!("{path}.source")),
            QuerySource::Param { entity, .. } => self.entity_ref(entity, &format!("{path}.source")),
            QuerySource::Call { call, .. } => self.call(call, &format!("{path}.source")),
            QuerySource::Search { search, query, alias } => {
                let found = self.p.forms.iter().find_map(|f| match f {
                    Form::Search(x) if x.name == *search => Some(x),
                    _ => None,
                });
                if found.is_none() {
                    self.err(codes::I122, &format!("{path}.source"), format!("unknown search '{search}'"));
                }
                self.search_aliases.push(alias.clone());
                self.expr(query, &format!("{path}.source"));
            }
        }
    }

    fn query(&mut self, q: &Query, path: &str) {
        self.params(&q.params, path);
        for (i, l) in q.lets.iter().enumerate() {
            self.let_(l, &format!("{path}.lets[{i}]"));
        }
        if let Some(a) = &q.allow {
            self.policy(a, &format!("{path}.allow"));
        }
        for (i, (c, _)) in q.fetches.iter().enumerate() {
            self.call(c, &format!("{path}.fetches[{i}]"));
        }
        self.search_aliases.clear();
        if let Some(s) = &q.source {
            self.query_source(s, path);
        }
        if let Some(f) = &q.filter {
            self.expr(f, &format!("{path}.filter"));
        }
        for (i, g) in q.group_by.iter().enumerate() {
            self.expr(g, &format!("{path}.group_by[{i}]"));
        }
        match &q.sort {
            Some(Sort::Keys { keys }) => self.sort_keys(keys, &format!("{path}.sort")),
            Some(Sort::ByParam { cases, .. }) => {
                for (k, keys) in cases {
                    self.sort_keys(keys, &format!("{path}.sort.{k}"));
                }
            }
            None => {}
        }
        self.selection(&q.select, &format!("{path}.select"));
        for (i, t) in q.touches.iter().enumerate() {
            self.expr(t, &format!("{path}.touches[{i}]"));
        }
        self.shape(&q.output, &format!("{path}.output"));
        self.search_aliases.clear();
    }

    fn command(&mut self, c: &Command, path: &str) {
        self.params(&c.params, path);
        if let Idempotency::Derived { key } = &c.idempotency {
            self.expr(key, &format!("{path}.idempotency"));
        }
        for (i, l) in c.lets.iter().enumerate() {
            self.let_(l, &format!("{path}.lets[{i}]"));
        }
        if let Some(a) = &c.allow {
            self.policy(a, &format!("{path}.allow"));
        }
        for (i, r) in c.requires.iter().enumerate() {
            self.require(r, &format!("{path}.requires[{i}]"));
        }
        self.stmts(&c.body, &format!("{path}.body"));
        for (i, e) in c.emits.iter().enumerate() {
            self.emit(e, &format!("{path}.emits[{i}]"));
        }
        if let Some(r) = &c.returns {
            self.expr(&r.value, &format!("{path}.returns"));
            if let Some(s) = &r.select {
                self.selection(s, &format!("{path}.returns.select"));
            }
        }
        self.shape(&c.output, &format!("{path}.output"));
    }

    fn emit(&mut self, e: &Emit, path: &str) {
        self.event_ref(&e.event, path);
        for (n, v) in &e.fields {
            self.expr(v, &format!("{path}.{n}"));
        }
        if let Some(k) = e.to.as_ref().and_then(|t| t.key.as_ref()) {
            self.expr(k, &format!("{path}.to.key"));
        }
    }

    // ---------- statements ----------

    fn values(&mut self, entity: &str, values: &[(String, Expr)], path: &str) {
        for (f, v) in values {
            if self.has_entity(entity) && !self.has_field(entity, f) {
                self.err(codes::I112, path, format!("'{entity}.{f}' does not exist"));
            }
            self.expr(v, &format!("{path}.{f}"));
        }
    }

    fn stmts(&mut self, v: &[Stmt], path: &str) {
        for (i, s) in v.iter().enumerate() {
            self.stmt(s, &format!("{path}[{i}]"));
        }
    }

    fn stmt(&mut self, s: &Stmt, path: &str) {
        match s {
            Stmt::Let(l) => self.expr(&l.value, path),
            Stmt::Insert { entity, from, values, .. } => {
                self.write_entity(entity, path);
                if let Some(f) = from {
                    self.set(f, path);
                }
                self.values(entity, values, path);
            }
            Stmt::Upsert { entity, keys, values, .. } => {
                self.write_entity(entity, path);
                for k in keys {
                    if self.has_entity(entity) && !self.has_field(entity, k) {
                        self.err(codes::I112, path, format!("'{entity}.{k}' does not exist"));
                    }
                }
                self.values(entity, values, path);
            }
            Stmt::Toggle { entity, values, .. } => {
                self.write_entity(entity, path);
                self.values(entity, values, path);
            }
            Stmt::Update { target, via, assigns } => {
                self.set(target, path);
                if let Some(v) = via {
                    self.expr(&v.path, path);
                }
                self.assigns(assigns, path);
            }
            Stmt::Delete { target } | Stmt::Purge { target } => self.set(target, path),
            Stmt::Erase { target } => self.expr(target, path),
            Stmt::Set { assigns } => self.assigns(assigns, path),
            Stmt::When { cond, body } => {
                self.expr(cond, path);
                self.stmts(body, &format!("{path}.body"));
            }
            Stmt::Each { source, body } => {
                self.set(source, path);
                self.stmts(body, &format!("{path}.body"));
            }
            Stmt::Effect { call, into, on_failure, .. } => {
                self.call(call, path);
                if let Some(i) = into {
                    self.expr(i, path);
                }
                if let Some(f) = on_failure {
                    self.stmts(f, &format!("{path}.on_failure"));
                }
            }
            Stmt::Reserve { of, .. } => self.expr(of, path),
            Stmt::AtRun { at, intent, args } => {
                self.expr(at, path);
                if !self.is_intent_name(intent) {
                    self.err(codes::I111, path, format!("unknown intent '{intent}'"));
                }
                for a in args {
                    self.expr(&a.value, path);
                }
            }
            Stmt::Notify(n) => {
                self.set(&n.to, path);
                self.call(&n.via, path);
                for (f, v) in &n.fields {
                    self.expr(v, &format!("{path}.{f}"));
                }
            }
            Stmt::ExportPersonalData { of, notify, .. } => {
                self.expr(of, path);
                if let Some(n) = notify {
                    self.expr(n, path);
                }
            }
        }
    }

    fn write_entity(&mut self, entity: &str, path: &str) {
        if !self.has_entity(entity) {
            self.err(codes::I112, path, format!("unknown entity '{entity}'"));
        }
    }

    fn assigns(&mut self, assigns: &[Assign], path: &str) {
        for (i, a) in assigns.iter().enumerate() {
            self.expr(&a.target, &format!("{path}.assigns[{i}].target"));
            self.expr(&a.value, &format!("{path}.assigns[{i}].value"));
        }
    }

    // ---------- expressions ----------

    fn set(&mut self, s: &SetExpr, path: &str) {
        match &s.source {
            SetSource::Entity { entity } => self.entity_ref(entity, path),
            SetSource::Expr { expr } => self.expr(expr, path),
        }
        if let Some(f) = &s.filter {
            self.expr(f, path);
        }
    }

    fn call(&mut self, c: &Call, path: &str) {
        match &c.callee {
            Callee::Fn { name } if !self.p.fns.contains_key(name) => self.err(codes::I111, path, format!("unknown fn '{name}'")),
            Callee::Relation { name } if !self.p.relations.contains_key(name) => self.err(codes::I111, path, format!("unknown relation '{name}'")),
            Callee::Intent { name } if !self.is_intent_name(name) => self.err(codes::I111, path, format!("unknown intent '{name}'")),
            _ => {}
        }
        for a in &c.args {
            self.expr(&a.value, path);
        }
    }

    fn expr(&mut self, e: &Expr, path: &str) {
        self.ty(&e.ty, path);
        match &e.node {
            Node::Lit { .. }
            | Node::Param { .. }
            | Node::Local { .. }
            | Node::Symbol { .. }
            | Node::Config { .. }
            | Node::Actor
            | Node::SelfRow
            | Node::This
            | Node::Now
            | Node::Today
            | Node::Public
            | Node::Authenticated => {}
            Node::SigningSecret { base } => self.expr(base, path),
            Node::SearchRank { alias } => {
                if !self.search_aliases.contains(alias) {
                    self.err(codes::I122, path, format!("'{alias}.rank' is only defined for the alias of a search source"));
                }
            }
            Node::RowField { entity, field } => {
                if !self.has_field(entity, field) {
                    self.err(codes::I109, path, format!("row field '{entity}.{field}' does not exist"));
                }
            }
            Node::RecordField { record, field } => {
                if !self.p.records.get(record).is_some_and(|r| r.fields.iter().any(|f| f.name == *field)) {
                    self.err(codes::I118, path, format!("record field '{record}.{field}' does not exist"));
                }
            }
            Node::EnumValue { enum_name, value } => match self.p.enums.get(enum_name) {
                None => self.err(codes::I110, path, format!("unknown enum '{enum_name}'")),
                Some(d) if !d.values.contains(value) => self.err(codes::I110, path, format!("'{value}' is not a value of {enum_name}")),
                Some(_) => {}
            },
            Node::Field { base, entity, field } => {
                if let Some(en) = entity {
                    if !self.has_entity(en) {
                        self.err(codes::I108, path, format!("unknown entity '{en}'"));
                    } else if !self.has_field(en, field) {
                        self.err(codes::I108, path, format!("'{en}.{field}' does not exist"));
                    }
                }
                self.expr(base, path);
            }
            Node::Unary { arg, .. } => self.expr(arg, path),
            Node::Binary { l, r, .. } => {
                self.expr(l, path);
                self.expr(r, path);
            }
            Node::InList { value, list } => {
                self.expr(value, path);
                for i in list {
                    self.expr(i, path);
                }
            }
            Node::InRange { value, lo, hi } => {
                self.expr(value, path);
                self.expr(lo, path);
                self.expr(hi, path);
            }
            Node::InSet { value, set: other } | Node::InRangeValue { value, range: other } => {
                self.expr(value, path);
                self.expr(other, path);
            }
            Node::TypeTest { value, entity } => {
                self.entity_ref(entity, path);
                self.expr(value, path);
            }
            Node::Pred { entity, name, row } => {
                match self.p.entities.get(entity) {
                    None => self.err(codes::I115, path, format!("unknown entity '{entity}'")),
                    Some(en) if !en.predicates.contains_key(name) => self.err(codes::I115, path, format!("'{entity}' has no predicate '{name}'")),
                    Some(_) => {}
                }
                self.expr(row, path);
            }
            Node::HasScope { subject, .. } => self.expr(subject, path),
            Node::Exists { set } | Node::The { set } => self.set(set, path),
            Node::Latest { set, by } => {
                self.set(set, path);
                self.expr(by, path);
            }
            Node::First { set, order } => {
                self.set(set, path);
                self.sort_keys(order, path);
            }
            Node::Agg { set, value, .. } => {
                if let Some(s) = set {
                    self.set(s, path);
                }
                if let Some(v) = value {
                    self.expr(v, path);
                }
            }
            Node::Quant { set, body, .. } | Node::Binder { set, body } => {
                self.set(set, path);
                self.expr(body, path);
            }
            Node::RunningSum { value, order, .. } => {
                self.expr(value, path);
                self.expr(order, path);
            }
            Node::If { cond, then, otherwise } => {
                self.expr(cond, path);
                self.expr(then, path);
                self.expr(otherwise, path);
            }
            Node::List { items } => {
                for i in items {
                    self.expr(i, path);
                }
            }
            Node::Call(c) => self.call(c, path),
        }
    }

    // ---------- reactions and forms ----------

    fn reaction(&mut self, r: &Reaction, path: &str) {
        match r {
            Reaction::Event { event, when, body, .. } => {
                self.event_ref(event, path);
                if let Some(w) = when {
                    self.expr(w, path);
                }
                self.stmts(body, &format!("{path}.body"));
            }
            Reaction::Schedule(s) => {
                if let Some(f) = &s.for_each {
                    self.set(f, path);
                }
                self.stmts(&s.body, &format!("{path}.body"));
            }
            Reaction::Retain { entity, notify, .. } => {
                self.entity_ref(entity, path);
                if let Some(n) = notify {
                    self.call(&n.via, path);
                }
            }
            Reaction::Rule { entity, when, body, .. } => {
                self.entity_ref(entity, path);
                self.expr(when, path);
                self.stmts(body, &format!("{path}.body"));
            }
            Reaction::Webhook(w) => {
                self.call(&w.via, path);
                for (i, h) in w.handlers.iter().enumerate() {
                    if let Some(t) = &h.ty {
                        self.ty(t, &format!("{path}.handlers[{i}]"));
                    }
                    self.stmts(&h.body, &format!("{path}.handlers[{i}].body"));
                }
            }
            Reaction::Consume { body, .. } => self.stmts(body, &format!("{path}.body")),
        }
    }

    fn form(&mut self, f: &Form, path: &str) {
        match f {
            Form::Subscribe(s) => {
                self.params(&s.params, path);
                self.policy(&s.allow, path);
                self.query_source(&s.source, path);
                if let Some(x) = &s.filter {
                    self.expr(x, path);
                }
                self.selection(&s.select, path);
            }
            Form::Projection(p) => {
                for e in &p.events {
                    self.event_ref(e, path);
                }
                for (_, _, st) in &p.handlers {
                    self.stmt(st, path);
                }
            }
            Form::Search(s) => {
                self.entity_ref(&s.entity, path);
                for (f, _) in &s.fields {
                    if self.has_entity(&s.entity) && !self.has_field(&s.entity, f) {
                        self.err(codes::I117, path, format!("'{}.{f}' does not exist", s.entity));
                    }
                }
            }
            Form::Job(j) => {
                self.params(&j.params, path);
                self.policy(&j.allow, path);
                if let Some(p) = &j.progress {
                    self.set(p, path);
                }
                if let Some(b) = &j.body {
                    self.stmts(b, &format!("{path}.body"));
                }
                if let Some((e, c)) = &j.notify {
                    self.expr(e, path);
                    self.call(c, path);
                }
            }
            Form::Verification(v) => {
                self.entity_ref(&v.subject, path);
                self.ty(&v.target, path);
                if let Some(w) = &v.target_where {
                    self.expr(w, path);
                }
                self.call(&v.deliver, path);
                self.stmts(&v.on_verified.2, &format!("{path}.on_verified"));
            }
            Form::GrantLink(g) => {
                self.params(&g.scope, path);
                self.params(&g.redeem_params, path);
                self.expr(&g.issued_by, path);
                if let Some(t) = &g.to {
                    self.expr(t, path);
                }
                if let Some((e, _)) = &g.grants {
                    self.expr(e, path);
                }
                for (i, r) in g.requires.iter().enumerate() {
                    self.require(r, &format!("{path}.requires[{i}]"));
                }
                if let Some(b) = &g.on_redeem {
                    self.stmts(b, &format!("{path}.on_redeem"));
                }
            }
            Form::Approval(a) => {
                self.entity_ref(&a.entity, path);
                self.set(&a.approvers, path);
                if let Some(r) = &a.requested_by {
                    self.expr(r, path);
                }
                self.stmts(&a.on_approved, &format!("{path}.on_approved"));
                self.stmts(&a.on_rejected, &format!("{path}.on_rejected"));
            }
            Form::OutboundWebhooks(o) => {
                self.entity_ref(&o.entity, path);
                for e in &o.events {
                    self.event_ref(e, path);
                }
                if let Some(x) = &o.filter {
                    self.expr(x, path);
                }
            }
            Form::Consent(c) => {
                for i in &c.intents {
                    if !self.is_intent_name(i) {
                        self.err(codes::I111, path, format!("unknown intent '{i}'"));
                    }
                }
            }
            Form::Impersonate(i) => {
                self.entity_ref(&i.entity, path);
                self.expr(&i.by, path);
            }
            Form::Migration(m) => self.stmts(&m.body, &format!("{path}.body")),
            Form::Upcast(u) => {
                self.event_ref(&u.event, path);
                for (f, v) in &u.with {
                    self.expr(v, &format!("{path}.{f}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn int() -> Type {
        Type::Int { min: None, max: None }
    }

    fn text() -> Type {
        Type::Text { min: None, max: None, trim: false, lower: false, pattern: None }
    }

    fn field(name: &str, ty: Type, kind: FieldKind) -> Field {
        Field {
            name: name.into(),
            was: None,
            ty,
            optional: false,
            default: None,
            kind,
            personal: false,
            encrypted: false,
            visible_to: None,
            masked: None,
            generated: None,
            variants: Vec::new(),
        }
    }

    fn entity(fields: Vec<Field>) -> Entity {
        Entity {
            was: None,
            removed_fields: Vec::new(),
            personal: false,
            access_audited: false,
            fields,
            constraints: Vec::new(),
            lifecycles: Vec::new(),
            visibility: None,
            predicates: BTreeMap::new(),
            traits: Traits::default(),
        }
    }

    fn ex(ty: Type, node: Node) -> Expr {
        Expr { ty, node }
    }

    fn lit_true() -> Expr {
        ex(Type::Bool, Node::Lit { lit: Literal::Bool(true) })
    }

    fn command(body: Vec<Stmt>) -> Command {
        Command {
            internal: false,
            cross_tenant: false,
            params: Vec::new(),
            idempotency: Idempotency::None,
            audited: false,
            rate_limits: Vec::new(),
            lets: Vec::new(),
            allow: Some(Policy { cond: ex(Type::Bool, Node::Public), code: None }),
            requires: Vec::new(),
            body,
            emits: Vec::new(),
            returns: None,
            output: Shape::None,
        }
    }

    /// Entities `Club(id, name, status: Status)` and `Post(id, club: Club)`, enum `Status`, one event; every reference resolves.
    fn valid() -> Program {
        let mut p = Program {
            aip_core: CORE_IR_VERSION.into(),
            uses: Vec::new(),
            actor: None,
            enums: BTreeMap::new(),
            records: BTreeMap::new(),
            entities: BTreeMap::new(),
            relations: BTreeMap::new(),
            fns: BTreeMap::new(),
            events: BTreeMap::new(),
            configs: BTreeMap::new(),
            flags: BTreeMap::new(),
            intents: BTreeMap::new(),
            reactions: Vec::new(),
            forms: Vec::new(),
            removed_entities: Vec::new(),
        };
        p.enums.insert("Status".into(), EnumDef { ordered: false, values: vec!["OPEN".into(), "CLOSED".into()] });
        let mut club = entity(vec![
            field("id", Type::Uuid, FieldKind::Implicit),
            field("name", text(), FieldKind::Stored),
            field("status", Type::Enum { name: "Status".into() }, FieldKind::Stored),
            field(
                "posts",
                Type::Many { of: Box::new(Type::Ref { entity: "Post".into() }) },
                FieldKind::Inverse { target: "Post".into(), via: "club".into() },
            ),
        ]);
        club.lifecycles
            .push(Lifecycle { field: "status".into(), transitions: vec![Transition { from: vec!["OPEN".into()], to: vec!["CLOSED".into()] }] });
        p.entities.insert("Club".into(), club);
        p.entities.insert(
            "Post".into(),
            entity(vec![
                field("id", Type::Uuid, FieldKind::Implicit),
                field("club", Type::Ref { entity: "Club".into() }, FieldKind::Ref { target: "Club".into(), on_delete: None, on_erase: None }),
            ]),
        );
        p.events
            .insert("Opened".into(), EventDef { version: None, fields: vec![("club".into(), Type::Ref { entity: "Club".into() })], declared: true });
        let ins = Stmt::Insert {
            entity: "Club".into(),
            from: None,
            values: vec![("name".into(), ex(text(), Node::Lit { lit: Literal::Text("a".into()) }))],
            bind: Some("c".into()),
        };
        let mut cmd = command(vec![ins]);
        cmd.emits.push(Emit { event: "Opened".into(), fields: Vec::new(), to: None });
        p.intents.insert("Open".into(), Intent::Command(cmd));
        p
    }

    fn codes(p: &Program) -> Vec<String> {
        validate(p).into_iter().map(|d| d.code).collect()
    }

    fn only(p: &Program, code: &str) -> IrDiagnostic {
        let d = validate(p);
        assert_eq!(d.len(), 1, "expected exactly {code}, got {d:?}");
        assert_eq!(d[0].code, code, "{d:?}");
        d.into_iter().next().expect("one diagnostic")
    }

    fn with_cmd(p: &mut Program, body: Vec<Stmt>) {
        p.intents.insert("T".into(), Intent::Command(command(body)));
    }

    fn when(cond: Expr) -> Vec<Stmt> {
        vec![Stmt::When { cond, body: Vec::new() }]
    }

    #[test]
    fn valid_program_has_no_diagnostics() {
        // positive control: the negative tests below each break exactly one thing in this program
        assert!(validate(&valid()).is_empty());
    }

    #[test]
    fn i101_version_mismatch() {
        let mut p = valid();
        p.aip_core = "aip-core/9.9".into();
        assert_eq!(only(&p, "AIP-I101").path, "aip_core");
    }

    #[test]
    fn i102_unknown_enum_type() {
        let mut p = valid();
        p.entities.entry("Club".into()).and_modify(|e| e.fields.push(field("x", Type::Enum { name: "Nope".into() }, FieldKind::Stored)));
        assert_eq!(only(&p, "AIP-I102").path, "entities.Club.fields.x.ty");
    }

    #[test]
    fn i103_unknown_record_type() {
        let mut p = valid();
        p.entities.entry("Club".into()).and_modify(|e| e.fields.push(field("x", Type::Record { name: "Nope".into() }, FieldKind::Stored)));
        assert_eq!(only(&p, "AIP-I103").path, "entities.Club.fields.x.ty");
        // the old `event:` spelling is no longer a record name
        let mut p = valid();
        p.entities.entry("Club".into()).and_modify(|e| e.fields.push(field("x", Type::Record { name: "event:Opened".into() }, FieldKind::Stored)));
        only(&p, "AIP-I103");
    }

    #[test]
    fn json_schema_record_and_validated_by_field_must_exist() {
        let mut p = valid();
        let ty = Type::Json { schema: Some("Nope".into()), validated_by: None };
        p.entities.entry("Club".into()).and_modify(|e| e.fields.push(field("x", ty, FieldKind::Stored)));
        assert_eq!(only(&p, "AIP-I103").path, "entities.Club.fields.x.ty");

        let mut p = valid();
        let ty = Type::Json { schema: None, validated_by: Some("nope.questions".into()) };
        p.entities.entry("Club".into()).and_modify(|e| e.fields.push(field("x", ty, FieldKind::Stored)));
        assert_eq!(only(&p, "AIP-I121").path, "entities.Club.fields.x.ty");

        let mut ok = valid();
        let ty = Type::Json { schema: None, validated_by: Some("name".into()) };
        ok.entities.entry("Club".into()).and_modify(|e| e.fields.push(field("x", ty, FieldKind::Stored)));
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn i114_event_type() {
        let mut p = valid();
        p.entities.entry("Club".into()).and_modify(|e| e.fields.push(field("x", Type::Event { name: "Nope".into() }, FieldKind::Stored)));
        assert_eq!(only(&p, "AIP-I114").path, "entities.Club.fields.x.ty");
        let mut ok = valid();
        ok.entities.entry("Club".into()).and_modify(|e| e.fields.push(field("x", Type::Event { name: "Opened".into() }, FieldKind::Stored)));
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn i104_unknown_entity_in_type() {
        for ty in [
            Type::Ref { entity: "Nope".into() },
            Type::Snapshot { entity: "Nope".into() },
            Type::RefUnion { entities: vec!["Club".into(), "Nope".into()] },
        ] {
            let mut p = valid();
            p.entities.entry("Club".into()).and_modify(|e| e.fields.push(field("x", ty, FieldKind::Stored)));
            only(&p, "AIP-I104");
        }
    }

    #[test]
    fn i105_zero_bound() {
        let mut p = valid();
        let ty = Type::Set { of: Box::new(int()), max: 0 };
        p.entities.entry("Club".into()).and_modify(|e| e.fields.push(field("x", ty, FieldKind::Stored)));
        only(&p, "AIP-I105");
        let mut p = valid();
        let ty = Type::List { of: Box::new(int()), max: 0 };
        p.entities.entry("Club".into()).and_modify(|e| e.fields.push(field("x", ty, FieldKind::Stored)));
        only(&p, "AIP-I105");
        let mut ok = valid();
        ok.entities.entry("Club".into()).and_modify(|e| e.fields.push(field("x", Type::List { of: Box::new(int()), max: 3 }, FieldKind::Stored)));
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn i106_unknown_reference_target() {
        let mut p = valid();
        p.entities
            .entry("Post".into())
            .and_modify(|e| e.fields.push(field("x", int(), FieldKind::Ref { target: "Nope".into(), on_delete: None, on_erase: None })));
        assert_eq!(only(&p, "AIP-I106").path, "entities.Post.fields.x.kind");
        let mut p = valid();
        p.entities
            .entry("Post".into())
            .and_modify(|e| e.fields.push(field("x", int(), FieldKind::RefUnion { targets: vec!["Nope".into()], on_delete: None, on_erase: None })));
        only(&p, "AIP-I106");
    }

    #[test]
    fn i107_inverse_target_and_via() {
        let mut p = valid();
        p.entities
            .entry("Club".into())
            .and_modify(|e| e.fields.push(field("x", int(), FieldKind::Inverse { target: "Nope".into(), via: "club".into() })));
        only(&p, "AIP-I107");
        let mut p = valid();
        p.entities
            .entry("Club".into())
            .and_modify(|e| e.fields.push(field("x", int(), FieldKind::Inverse { target: "Post".into(), via: "missing".into() })));
        only(&p, "AIP-I107");
    }

    #[test]
    fn i108_field_expression() {
        let base = ex(Type::Ref { entity: "Club".into() }, Node::Param { name: "c".into() });
        let mk =
            |entity: &str, field: &str| ex(int(), Node::Field { base: Box::new(base.clone()), entity: Some(entity.into()), field: field.into() });
        let mut p = valid();
        with_cmd(&mut p, when(mk("Club", "missing")));
        assert_eq!(only(&p, "AIP-I108").path, "intents.T.body[0]");
        let mut p = valid();
        with_cmd(&mut p, when(mk("Nope", "name")));
        only(&p, "AIP-I108");
        let mut ok = valid();
        with_cmd(&mut ok, when(mk("Club", "name")));
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn i109_row_field() {
        let mut p = valid();
        p.entities.entry("Club".into()).and_modify(|e| {
            e.predicates.insert("bad".into(), ex(Type::Bool, Node::RowField { entity: "Club".into(), field: "missing".into() }));
        });
        assert_eq!(only(&p, "AIP-I109").path, "entities.Club.predicates.bad");
        let mut p = valid();
        p.entities.entry("Club".into()).and_modify(|e| {
            e.predicates.insert("bad".into(), ex(Type::Bool, Node::RowField { entity: "Nope".into(), field: "name".into() }));
        });
        only(&p, "AIP-I109");
    }

    #[test]
    fn i109_row_field_is_entity_only() {
        // a record name is not an entity: bare record fields have their own node
        let mut p = valid();
        p.records.insert("R".into(), RecordDef { fields: vec![field("a", int(), FieldKind::Stored)], checks: Vec::new() });
        p.entities.entry("Club".into()).and_modify(|e| {
            e.predicates.insert("bad".into(), ex(Type::Bool, Node::RowField { entity: "R".into(), field: "a".into() }));
        });
        only(&p, "AIP-I109");
    }

    #[test]
    fn i118_record_field() {
        let mk = |record: &str, field: &str| RecordDef {
            fields: vec![self::field("a", int(), FieldKind::Stored)],
            checks: vec![Require {
                when: None,
                cond: ex(Type::Bool, Node::RecordField { record: record.into(), field: field.into() }),
                code: "X".into(),
            }],
        };
        let mut p = valid();
        p.records.insert("R".into(), mk("R", "missing"));
        assert_eq!(only(&p, "AIP-I118").path, "records.R.checks[0].cond");
        let mut p = valid();
        p.records.insert("R".into(), mk("Nope", "a"));
        only(&p, "AIP-I118");
        let mut ok = valid();
        ok.records.insert("R".into(), mk("R", "a"));
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn i119_counter_store() {
        let counter =
            |store: &str| field("views", int(), FieldKind::Counter { store: store.into(), dedupe: None, window_seconds: None, sharded: None });
        let mut p = valid();
        p.entities.entry("Club".into()).and_modify(|e| e.fields.push(counter("redis")));
        assert_eq!(only(&p, "AIP-I119").path, "entities.Club.fields.views.kind");
        // declared as another extension: still an error
        p.uses.push("s3".into());
        only(&p, "AIP-I119");
        p.uses.push("redis".into());
        assert!(validate(&p).is_empty());
        // `use redis.cluster` also declares the `redis` family
        let mut q = valid();
        q.uses.push("redis.cluster".into());
        q.entities.entry("Club".into()).and_modify(|e| e.fields.push(counter("redis")));
        assert!(validate(&q).is_empty());
    }

    #[test]
    fn i116_type_test_entity() {
        let value = ex(Type::Ref { entity: "Club".into() }, Node::Actor);
        let mk = |entity: &str| ex(Type::Bool, Node::TypeTest { value: Box::new(value.clone()), entity: entity.into() });
        let mut p = valid();
        with_cmd(&mut p, when(mk("Nope")));
        only(&p, "AIP-I116");
        let mut ok = valid();
        with_cmd(&mut ok, when(mk("Club")));
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn new_nodes_validate_their_operands() {
        // InRangeValue, Symbol and Update.via are walked: a bad enum value inside each is found
        let bad = ex(Type::Enum { name: "Status".into() }, Node::EnumValue { enum_name: "Status".into(), value: "NOPE".into() });
        let range = ex(Type::Range { of: Box::new(int()) }, Node::Param { name: "r".into() });
        let mut p = valid();
        with_cmd(&mut p, when(ex(Type::Bool, Node::InRangeValue { value: Box::new(bad.clone()), range: Box::new(range.clone()) })));
        only(&p, "AIP-I110");
        let mut p = valid();
        let target = SetExpr { source: SetSource::Entity { entity: "Club".into() }, alias: None, filter: None };
        with_cmd(&mut p, vec![Stmt::Update { target, via: Some(UpdateVia { path: bad, alias: "x".into() }), assigns: Vec::new() }]);
        only(&p, "AIP-I110");
        let mut ok = valid();
        with_cmd(&mut ok, when(ex(Type::Bool, Node::InRangeValue { value: Box::new(lit_true()), range: Box::new(range) })));
        with_cmd(&mut ok, when(ex(Type::Symbol, Node::Symbol { name: "kakao".into() })));
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn i110_enum_value() {
        let mk = |en: &str, v: &str| ex(Type::Enum { name: "Status".into() }, Node::EnumValue { enum_name: en.into(), value: v.into() });
        let mut p = valid();
        with_cmd(&mut p, when(mk("Status", "NOPE")));
        only(&p, "AIP-I110");
        let mut p = valid();
        with_cmd(&mut p, when(mk("Nope", "OPEN")));
        only(&p, "AIP-I110");
        let mut ok = valid();
        with_cmd(&mut ok, when(mk("Status", "OPEN")));
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn i111_callee() {
        for callee in [Callee::Fn { name: "nope".into() }, Callee::Relation { name: "nope".into() }, Callee::Intent { name: "nope".into() }] {
            let mut p = valid();
            with_cmd(&mut p, when(ex(Type::Bool, Node::Call(Call { callee, args: Vec::new() }))));
            only(&p, "AIP-I111");
        }
        let mut ok = valid();
        with_cmd(&mut ok, when(ex(Type::Bool, Node::Call(Call { callee: Callee::Intent { name: "Open".into() }, args: Vec::new() }))));
        assert!(validate(&ok).is_empty());
        let mut p = valid();
        with_cmd(&mut p, vec![Stmt::AtRun { at: ex(Type::Time, Node::Now), intent: "nope".into(), args: Vec::new() }]);
        only(&p, "AIP-I111");
    }

    #[test]
    fn i112_write_targets() {
        let mut p = valid();
        with_cmd(&mut p, vec![Stmt::Insert { entity: "Nope".into(), from: None, values: Vec::new(), bind: None }]);
        only(&p, "AIP-I112");
        let v = vec![("missing".to_string(), lit_true())];
        let mut p = valid();
        with_cmd(&mut p, vec![Stmt::Insert { entity: "Club".into(), from: None, values: v.clone(), bind: None }]);
        only(&p, "AIP-I112");
        let mut p = valid();
        with_cmd(&mut p, vec![Stmt::Upsert { entity: "Club".into(), keys: vec!["missing".into()], values: Vec::new(), bind: None }]);
        only(&p, "AIP-I112");
        let mut p = valid();
        with_cmd(&mut p, vec![Stmt::Toggle { entity: "Club".into(), values: v }]);
        only(&p, "AIP-I112");
    }

    #[test]
    fn i113_lifecycle() {
        let mut p = valid();
        p.entities.entry("Club".into()).and_modify(|e| e.lifecycles[0].field = "missing".into());
        only(&p, "AIP-I113");
        let mut p = valid();
        p.entities.entry("Club".into()).and_modify(|e| e.lifecycles[0].field = "name".into());
        only(&p, "AIP-I113");
        let mut p = valid();
        p.entities.entry("Club".into()).and_modify(|e| e.lifecycles[0].transitions[0].to = vec!["NOPE".into()]);
        assert_eq!(only(&p, "AIP-I113").path, "entities.Club.lifecycles[0].transitions[0]");
    }

    #[test]
    fn i114_events() {
        let mut p = valid();
        p.reactions.push(Reaction::Event { cross_tenant: false, event: "Nope".into(), binding: "e".into(), when: None, body: Vec::new() });
        assert_eq!(only(&p, "AIP-I114").path, "reactions[0]");
        let mut p = valid();
        if let Some(Intent::Command(c)) = p.intents.get_mut("Open") {
            c.emits[0].event = "Nope".into();
        }
        only(&p, "AIP-I114");
        let mut ok = valid();
        ok.reactions.push(Reaction::Event { cross_tenant: false, event: "Opened".into(), binding: "e".into(), when: None, body: Vec::new() });
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn i115_predicate() {
        let row = ex(Type::Ref { entity: "Club".into() }, Node::Param { name: "c".into() });
        let mk = |entity: &str, name: &str| ex(Type::Bool, Node::Pred { entity: entity.into(), name: name.into(), row: Box::new(row.clone()) });
        let mut p = valid();
        with_cmd(&mut p, when(mk("Club", "missing")));
        only(&p, "AIP-I115");
        let mut p = valid();
        with_cmd(&mut p, when(mk("Nope", "x")));
        only(&p, "AIP-I115");
        let mut ok = valid();
        ok.entities.entry("Club".into()).and_modify(|e| {
            e.predicates.insert("open".into(), lit_true());
        });
        with_cmd(&mut ok, when(mk("Club", "open")));
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn i116_entity_reference() {
        let mut p = valid();
        p.reactions.push(Reaction::Rule {
            cross_tenant: false,
            name: "r".into(),
            entity: "Nope".into(),
            alias: "x".into(),
            when: lit_true(),
            body: Vec::new(),
        });
        only(&p, "AIP-I116");
        let mut p = valid();
        with_cmd(&mut p, vec![Stmt::Delete { target: SetExpr { source: SetSource::Entity { entity: "Nope".into() }, alias: None, filter: None } }]);
        only(&p, "AIP-I116");
        let mut p = valid();
        p.actor = Some(Actor {
            entity: "Nope".into(),
            provider: Call { callee: Callee::Ext { name: "auth.oidc".into() }, args: Vec::new() },
            scopes: Vec::new(),
            superuser: None,
        });
        assert_eq!(only(&p, "AIP-I116").path, "actor.entity");
    }

    #[test]
    fn i117_constraint_fields() {
        let mut p = valid();
        p.entities
            .entry("Club".into())
            .and_modify(|e| e.constraints.push(Constraint::Unique { ordinal: 0, fields: vec!["missing".into()], filter: None, code: None }));
        assert_eq!(only(&p, "AIP-I117").path, "entities.Club.constraints[0]");
        let mut ok = valid();
        ok.entities
            .entry("Club".into())
            .and_modify(|e| e.constraints.push(Constraint::Unique { ordinal: 0, fields: vec!["name".into()], filter: None, code: None }));
        assert!(validate(&ok).is_empty());
    }

    #[test]
    fn i120_constraint_ordinals_are_distinct() {
        let mut p = valid();
        p.entities.entry("Club".into()).and_modify(|e| {
            e.constraints.push(Constraint::Unique { ordinal: 3, fields: vec!["name".into()], filter: None, code: None });
            e.constraints.push(Constraint::Unique { ordinal: 3, fields: vec!["name".into()], filter: None, code: None });
        });
        assert_eq!(only(&p, "AIP-I120").path, "entities.Club.constraints[1]");
    }

    #[test]
    fn codes_are_stable_strings() {
        // guards the documented numbering: a renumbering must be a conscious change
        let mut p = valid();
        p.aip_core = "x".into();
        assert_eq!(codes(&p), vec!["AIP-I101".to_string()]);
    }
}
