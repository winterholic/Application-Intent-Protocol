//! Semantic rules over Core IR, independent of the frontend that produced it.
//!
//! A frontend decides what names mean and what types expressions have; once
//! that is settled, whether the program is *sound* (every intent says who may
//! call it, a toggle has the unique constraint it relies on, an event is
//! emitted with one shape, ...) is a property of the IR alone. These rules
//! live here so a TS or Python frontend gets exactly the checks `.aip` gets.
//! [`crate::validate::validate`] checks that the IR is well-formed; this checks
//! that a well-formed program follows the safety rules of
//! `docs/design/04-safety-matrix.md`. Run both.
//!
//! Codes keep their registered meaning (`spec/diagnostics.md`):
//! - `AIP-E100` a query has neither `from` nor `fetch`
//! - `AIP-E101` a webhook handles one event twice, or a form's generated intent has the name of a declared one
//! - `AIP-E103` `has scope` names a scope the actor does not declare
//! - `AIP-E106` an inverse field's `via` is not a reference back to the entity
//! - `AIP-E110` `actor`, `authenticated`, actor visibility, `publishable by` or `impersonate` without an actor declaration
//! - `AIP-E201` generator, counter, `set null`, unique, `no overlap` and dynamic-schema fields of the wrong shape
//! - `AIP-E204` `touch` on a field that is not a counter
//! - `AIP-E205` insert, upsert or toggle leaves a required field out
//! - `AIP-E207` upsert or toggle without the unique constraint it relies on
//! - `AIP-E208` an `Upload` parameter outside a command, a `drafts` query that reads no publishable entity or is public, a `consent` on something that is not a callable intent, an `impersonate` of an entity other than the actor, `signingSecret` read anywhere but the `returns` of the not idempotent command that inserted the endpoint
//! - `AIP-E210` an empty enum, an empty `in [...]` list, an approval that needs no approvers, a consent below version 1 or a non-positive `ttl`
//! - `AIP-E214` malformed webhook options
//! - `AIP-E216` an event emitted with a shape other than its first one
//! - `AIP-E221` `sort by <param> of {...}` misses an enum value
//! - `AIP-E301` an intent without `allow`, or a `publishable` entity nobody is allowed to publish
//! - `AIP-E302` `Json validated by` a snapshot of an entity without a dynamic schema
//! - `AIP-E306` `self` outside field visibility of the actor entity
//! - `AIP-E309` a reference to personal data without an erase policy, or `anonymize` on a required field
//! - `AIP-E310` `erase` of an entity that is not personal
//! - `AIP-E311` `notify` whose recipient cannot be told from the row
//! - `AIP-E312` `Json validated by` a field that is not a pinned snapshot
//! - `AIP-E313` a `tenant via` path that is not a chain of required references ending at one tenant entity
//! - `AIP-E314` an intent reads tenant-scoped rows but nothing fixes its tenant
//! - `AIP-E315` `cross tenant` on an intent that is not `internal`
//! - `AIP-E316` a `search` with an unsupported language, a weight outside A to D, a non-text or repeated field, no field, or over a `publishable` entity
//! - `AIP-E317` an `outbound webhooks` form with an unsupported scheme, a retry count outside 0 to 16, a non-positive duration, no event, an endpoint entity without `url`, or a second form for one entity
//! - `AIP-E318` an `encrypted` field that is not plain stored text, has a default or generated value, or is in a unique, no overlap, at most one, search, slug or outbound url
//! - `AIP-E319` an `encrypted` field read inside an expression
//! - `AIP-E320` an `encrypted` field written from anything but a parameter, or over more than one row
//! - `AIP-W402` both sides of a comparison, or both arguments of a call, are the same expression
//! - `AIP-W403` an unbounded query result
//! - `AIP-W404` a schedule with a time of day and no timezone
//! - `AIP-W501` a command creates rows and is not idempotent
//! - `AIP-W502` a `use` of an extension that is not first-party

use crate::facts::{callee_name, entity_of_type, set_entity};
use crate::validate::IrDiagnostic;
use crate::*;

/// Every code [`analyze`] can report. Conformance tests use it to tell the semantic cases from the frontend ones.
pub const CODES: &[&str] = &[
    codes::E100,
    codes::E101,
    codes::E103,
    codes::E106,
    codes::E110,
    codes::E201,
    codes::E204,
    codes::E205,
    codes::E207,
    codes::E208,
    codes::E210,
    codes::E214,
    codes::E216,
    codes::E221,
    codes::E301,
    codes::E302,
    codes::E306,
    codes::E309,
    codes::E310,
    codes::E311,
    codes::E312,
    codes::E313,
    codes::E314,
    codes::E315,
    codes::E316,
    codes::E317,
    codes::E318,
    codes::E319,
    codes::E320,
    codes::W402,
    codes::W403,
    codes::W404,
    codes::W501,
    codes::W502,
];

pub fn analyze(p: &Program) -> Vec<IrDiagnostic> {
    let mut a = A { p, out: Vec::new(), pending: Vec::new(), self_ok: false, emits: Vec::new(), tn: TenantCtx::default(), secret_rows: None };
    a.program();
    a.events();
    a.out
}

/// Extensions the language ships with; others get a warning.
const FIRST_PARTY: &[&str] =
    &["postgres", "redis", "kafka", "s3", "http", "mail", "payments", "auth", "notify", "inventory", "geo", "search", "kms", "fx"];

const WEBHOOK_OPTIONS: &[&str] = &["secret", "header", "event", "id", "payload"];

struct A<'p> {
    p: &'p Program,
    out: Vec<IrDiagnostic>,
    /// `(binding, field)` pairs that an `effect ... into binding.field` of an enclosing statement list fills later.
    pending: Vec<(String, String)>,
    /// `self` is legal: inside field visibility or masking of the actor entity.
    self_ok: bool,
    /// Every `emit` of a command, with its IR path, for the event-shape rule.
    emits: Vec<(String, &'p Emit)>,
    tn: TenantCtx<'p>,
    /// `(binding, entity)` of the rows the command being analysed inserts, while its `returns` selection is read: the only
    /// place `row.signingSecret` is legal. `None` everywhere else.
    secret_rows: Option<(Vec<(String, String)>, bool)>,
}

/// What fixes the tenant in the declaration being analysed (see [`tenant`]).
#[derive(Default)]
struct TenantCtx<'p> {
    /// Reads of tenant-scoped rows are checked. Off for system contexts and `cross tenant`.
    enforce: bool,
    /// A required parameter or the row being processed fixes the tenant.
    anchored: bool,
    /// Entity parameters whose rows all share one tenant, so sets reached from them are in it.
    checked: Vec<&'p Param>,
}

// ---------- types ----------

/// A type as expression typing sees it: refinements dropped, collections multi-valued.
fn norm(t: &Type) -> Type {
    use Type as T;
    match t {
        T::Int { .. } => T::Int { min: None, max: None },
        T::Decimal { .. } => T::Decimal { precision: None, scale: None },
        T::Text { .. } => T::Text { min: None, max: None, trim: false, lower: false, pattern: None },
        T::RichText { .. } => T::RichText { policy: None },
        T::Phone { .. } => T::Phone { region: None },
        T::Json { .. } => T::Json { schema: None, validated_by: None },
        T::Upload { .. } => T::Upload { max_bytes: None, types: Vec::new() },
        T::Credential { .. } => T::Credential { kind: String::new() },
        T::Set { of, .. } | T::List { of, .. } | T::Many { of } => T::Many { of: Box::new(norm(of)) },
        T::Range { of } => T::Range { of: Box::new(norm(of)) },
        T::Localized { of } => T::Localized { of: Box::new(norm(of)) },
        other => other.clone(),
    }
}

fn textual(t: &Type) -> bool {
    matches!(t, Type::Text { .. } | Type::RichText { .. } | Type::Email | Type::Url | Type::Phone { .. })
}

fn temporal(t: &Type) -> bool {
    matches!(t, Type::Time | Type::Date)
}

/// Can a value of `b` be used where `a` is expected (or be compared with it)? Both are [`norm`]alized.
fn accepts(a: &Type, b: &Type) -> bool {
    use Type as T;
    match (a, b) {
        (T::Unknown, _) | (_, T::Unknown) | (_, T::Null) | (T::Null, _) => true,
        (T::Localized { of }, o) | (o, T::Localized { of }) => accepts(of, o),
        (x, y) if textual(x) && textual(y) => true,
        (x, y) if temporal(x) && temporal(y) => true,
        (T::Int { .. }, T::Decimal { .. }) | (T::Decimal { .. }, T::Int { .. }) => true,
        (T::Money { .. }, T::Int { .. }) | (T::Int { .. }, T::Money { .. }) | (T::Money { .. }, T::Decimal { .. }) => true,
        (T::Ref { entity: x }, T::Snapshot { entity: y }) | (T::Snapshot { entity: x }, T::Ref { entity: y }) => x == y,
        (T::RefUnion { entities }, T::Ref { entity }) | (T::Ref { entity }, T::RefUnion { entities }) => entities.contains(entity),
        (T::Ref { .. }, T::Uuid) | (T::Uuid, T::Ref { .. }) => true,
        (T::Ext { .. }, _) | (_, T::Ext { .. }) => true,
        (T::Object, T::Upload { .. }) => true,
        (T::Json { .. }, T::Record { .. }) | (T::Record { .. }, T::Json { .. }) => true,
        (T::Many { of: x }, T::Many { of: y }) => accepts(x, y),
        (x, y) => x == y,
    }
}

/// Most retries an `outbound webhooks` form may ask for: the schedule doubles the wait each time.
pub const MAX_OUTBOUND_RETRIES: i64 = 16;

/// The rows a statement list inserts under a binding, nested bodies included.
fn inserted_rows(stmts: &[Stmt], out: &mut Vec<(String, String)>) {
    for s in stmts {
        match s {
            Stmt::Insert { entity, bind: Some(b), .. } => out.push((b.clone(), entity.clone())),
            Stmt::When { body, .. } | Stmt::Each { body, .. } => inserted_rows(body, out),
            _ => {}
        }
    }
}

/// How the frontend printed a type in a message.
fn ty_name(t: &Type) -> String {
    use Type as T;
    match t {
        T::Money { currency } => format!("Money({currency})"),
        T::Enum { name } | T::Record { name } | T::Ref { entity: name } | T::Ext { name } | T::Event { name } => name.clone(),
        T::RefUnion { entities } => format!("ref {}", entities.join(" | ")),
        T::Snapshot { entity } => format!("Snapshot<{entity}>"),
        T::Range { of } => format!("Range<{}>", ty_name(of)),
        T::Localized { of } => format!("Localized<{}>", ty_name(of)),
        T::Many { of } | T::Set { of, .. } | T::List { of, .. } => format!("{}[]", ty_name(of)),
        T::Bool => "Bool".into(),
        T::Int { .. } => "Int".into(),
        T::Decimal { .. } => "Decimal".into(),
        T::Text { .. } => "Text".into(),
        T::RichText { .. } => "RichText".into(),
        T::Email => "Email".into(),
        T::Url => "Url".into(),
        T::Phone { .. } => "Phone".into(),
        T::Time => "Time".into(),
        T::Date => "Date".into(),
        T::Duration => "Duration".into(),
        T::Size => "Size".into(),
        T::Uuid => "Uuid".into(),
        T::Json { .. } => "Json".into(),
        T::Object => "Object".into(),
        T::Upload { .. } => "Upload".into(),
        T::Credential { .. } => "Credential".into(),
        T::Recurrence => "Recurrence".into(),
        T::Null => "Null".into(),
        T::Symbol => "Symbol".into(),
        T::Unknown => "Unknown".into(),
    }
}

fn sig(fields: &[(String, Type)]) -> String {
    let mut v: Vec<String> = fields.iter().map(|(n, t)| format!("{n}: {}", ty_name(t))).collect();
    v.sort();
    v.join(", ")
}

/// The entity a field access reads from: the one the frontend recorded, or the one the base expression is a row of.
fn row_entity(base: &Expr, entity: &Option<String>) -> Option<String> {
    entity.clone().or_else(|| match &base.ty {
        Type::Ref { entity } | Type::Snapshot { entity } => Some(entity.clone()),
        _ => None,
    })
}

/// A value the runtime can encrypt as it arrives: a parameter or binding, or one member of a record parameter.
fn plain_value(e: &Expr) -> bool {
    match &e.node {
        Node::Param { .. } | Node::Local { .. } => true,
        Node::Field { base, entity: None, .. } => matches!(base.node, Node::Param { .. } | Node::Local { .. }),
        _ => false,
    }
}

impl<'p> A<'p> {
    fn diag(&mut self, code: &str, path: &str, message: impl Into<String>) -> &mut IrDiagnostic {
        self.out.push(IrDiagnostic { code: code.to_string(), path: path.to_string(), message: message.into(), help: None, related: None });
        self.out.last_mut().expect("just pushed")
    }

    fn entity(&self, name: &str) -> Option<&'p Entity> {
        self.p.entities.get(name)
    }

    fn encrypted(&self, entity: &str, field: &str) -> bool {
        self.entity(entity).is_some_and(|e| e.fields.iter().any(|f| f.name == field && f.encrypted))
    }

    fn actor_entity(&self) -> Option<&'p str> {
        self.p.actor.as_ref().map(|a| a.entity.as_str())
    }

    // ---------- program ----------

    fn program(&mut self) {
        let p = self.p;
        for (i, u) in p.uses.iter().enumerate() {
            if !FIRST_PARTY.contains(&u.as_str()) {
                self.diag(codes::W502, &format!("uses[{i}]"), format!("'{u}' is not a first-party extension"));
            }
        }
        for (name, e) in &p.enums {
            if e.values.is_empty() {
                self.diag(codes::E210, &format!("enums.{name}"), format!("enum {name} has no values"));
            }
        }
        if let Some(a) = &p.actor
            && let Some(s) = &a.superuser
        {
            self.expr(s, "actor.superuser");
        }
        for (name, r) in &p.records {
            for (i, c) in r.checks.iter().enumerate() {
                self.require(c, &format!("records.{name}.checks[{i}]"));
            }
        }
        for (name, e) in &p.entities {
            self.entity_rules(name, e);
        }
        self.tenant_paths();
        for (name, r) in &p.relations {
            let base = format!("relations.{name}");
            self.params(&r.params, &base);
            self.expr(&r.body, &format!("{base}.body"));
        }
        for (name, f) in &p.fns {
            if let FnBody::Expr { expr } = &f.body {
                self.expr(expr, &format!("fns.{name}.body"));
            }
        }
        for (name, c) in &p.configs {
            if let Some(d) = &c.default {
                self.expr(d, &format!("configs.{name}.default"));
            }
        }
        for (name, i) in &p.intents {
            let base = format!("intents.{name}");
            match i {
                Intent::Query(q) => {
                    self.intent_tenant(&q.params, q.internal, q.cross_tenant, &base);
                    self.query(name, q, &base);
                }
                Intent::Command(c) => {
                    self.intent_tenant(&c.params, c.internal, c.cross_tenant, &base);
                    self.command(name, c, &base);
                }
            }
            self.tn = TenantCtx::default();
        }
        for (i, r) in p.reactions.iter().enumerate() {
            self.reaction(r, &format!("reactions[{i}]"));
            self.tn = TenantCtx::default();
        }
        for (i, f) in p.forms.iter().enumerate() {
            self.form(f, &format!("forms[{i}]"));
            self.tn = TenantCtx::default();
        }
    }

    fn params(&mut self, ps: &[Param], path: &str) {
        for p in ps {
            if matches!(p.ty, Type::Upload { .. }) {
                self.diag(codes::E208, &format!("{path}.params.{}", p.name), "Upload parameters are only allowed on commands");
            }
            if let Some(d) = &p.default {
                self.expr(d, &format!("{path}.params.{}.default", p.name));
            }
        }
    }

    /// Params of a command may be uploads; every other declaration's may not.
    fn command_params(&mut self, ps: &[Param], path: &str) {
        for p in ps {
            if let Some(d) = &p.default {
                self.expr(d, &format!("{path}.params.{}.default", p.name));
            }
        }
    }

    // ---------- tenants ----------

    /// `tenant via` paths are chains of required references that all end at one entity.
    fn tenant_paths(&mut self) {
        let p = self.p;
        let mut ends: Vec<(String, String)> = Vec::new();
        for (name, e) in &p.entities {
            if e.traits.tenant.is_none() {
                continue;
            }
            let at = format!("entities.{name}.traits.tenant");
            match tenant::path(p, name) {
                Ok(hops) => ends.push((name.clone(), hops.last().map(|h| h.target.clone()).unwrap_or_default())),
                Err(why) => {
                    self.diag(codes::E313, &at, format!("the tenant path of {name} is not usable: {why}")).help =
                        Some("write 'tenant via a.b' with required reference fields, ending at the entity that is the tenant".into());
                }
            }
        }
        if let Some((first, root)) = ends.first().cloned() {
            for (name, end) in &ends {
                if *end != root {
                    self.diag(
                        codes::E313,
                        &format!("entities.{name}.traits.tenant"),
                        format!("{name} ends its tenant path at {end}, but {first} ends at {root}; one application has one kind of tenant"),
                    );
                }
            }
        }
    }

    /// Sets the tenant context of an intent from its parameters.
    fn intent_tenant(&mut self, params: &'p [Param], internal: bool, cross: bool, path: &str) {
        if cross && !internal {
            self.diag(codes::E315, &format!("{path}.name"), "'cross tenant' is only allowed on 'internal' intents").help =
                Some("write 'internal cross tenant', or drop 'cross tenant' and pass a parameter that fixes the tenant".into());
        }
        let checked = tenant::checked_params(self.p, params);
        self.tn = TenantCtx { enforce: !cross, anchored: tenant::anchor(&checked).is_some(), checked };
    }

    /// The tenant context of a form whose parameters are its intents' parameters.
    fn form_tenant(&mut self, params: &'p [Param]) {
        let checked = tenant::checked_params(self.p, params);
        self.tn = TenantCtx { enforce: true, anchored: tenant::anchor(&checked).is_some(), checked };
    }

    /// Reading rows of a tenant-scoped entity needs a tenant to read them in, unless the set is bound
    /// to the call's own rows. The backend filters unbound sets to the tenant; here the rule is that one exists.
    fn tenant_read(&mut self, entity: &str, bound: bool, path: &str) {
        if !self.tn.enforce || self.tn.anchored || bound || !tenant::scoped(self.p, entity) {
            return;
        }
        self.diag(codes::E314, path, format!("a read of {entity} needs a parameter that fixes the tenant")).help =
            Some("add a required parameter of the tenant entity (or of a tenant-scoped one), or make the intent 'internal cross tenant'".into());
    }

    // ---------- entities ----------

    fn entity_rules(&mut self, name: &str, e: &'p Entity) {
        let base = format!("entities.{name}");
        let is_actor = Some(name) == self.actor_entity();
        for f in &e.fields {
            let path = format!("{base}.fields.{}", f.name);
            self.field_rules(name, e, f, &path);
            if let Some(d) = &f.default {
                self.expr(d, &format!("{path}.default"));
            }
            let saved = self.self_ok;
            self.self_ok = is_actor;
            if let Some(v) = &f.visible_to {
                self.expr(v, &format!("{path}.visible_to"));
            }
            if let Some(m) = &f.masked {
                self.expr(&m.unless, &format!("{path}.masked.unless"));
            }
            self.self_ok = saved;
            if let Some(m) = &f.masked {
                self.call_args(&m.with, &format!("{path}.masked.with"));
            }
        }
        for (i, c) in e.constraints.iter().enumerate() {
            self.constraint_rules(e, c, &format!("{base}.constraints[{i}]"));
        }
        if let Some(v) = &e.visibility {
            if self.p.actor.is_none() {
                self.diag(codes::E110, &format!("{base}.visibility"), "'visible to actor' needs an actor declaration");
            }
            self.expr(&v.cond, &format!("{base}.visibility"));
        }
        for (pn, body) in &e.predicates {
            self.expr(body, &format!("{base}.predicates.{pn}"));
        }
        if e.traits.publishable {
            self.publish_rules(name, e, &base);
        }
        if let Some(from) = &e.traits.dynamic_schema
            && let Some(f) = e.fields.iter().find(|f| f.name == *from)
            && !matches!(norm(&f.ty), Type::Many { of } if matches!(*of, Type::Record { .. }))
        {
            self.diag(
                codes::E201,
                &format!("{base}.traits.dynamic_schema"),
                format!("dynamic schema needs a List<Record> field, {from} is {}", ty_name(&norm(&f.ty))),
            );
        }
    }

    /// Who may publish: the `by` condition, else the superuser, else nobody, which is a declaration without `allow`.
    fn publish_rules(&mut self, name: &str, e: &'p Entity, base: &str) {
        let at = format!("{base}.traits.publishable");
        self.clashes(&[format!("Publish{name}"), format!("Discard{name}Draft")], &at);
        match &e.traits.publish_by {
            Some(by) => {
                if self.p.actor.is_none() {
                    self.diag(codes::E110, &at, "'publishable by' needs an actor declaration");
                }
                // the row being published fixes the tenant of what the condition reads
                let saved =
                    std::mem::replace(&mut self.tn, TenantCtx { enforce: true, anchored: tenant::has_tenant(self.p, name), checked: Vec::new() });
                self.expr(by, &format!("{base}.traits.publish_by"));
                self.tn = saved;
            }
            None if self.p.actor.as_ref().is_none_or(|a| a.superuser.is_none()) => {
                self.diag(
                    codes::E301,
                    &at,
                    format!("nobody may publish {name}: it has no 'publishable by' condition and the program declares no superuser"),
                )
                .help = Some("write 'publishable by <condition>', for example 'publishable by memberOf(actor, workspace)'".into());
            }
            None => {}
        }
    }

    /// Generated intent names share the namespace of the declared ones.
    fn clashes(&mut self, names: &[String], path: &str) {
        for n in names {
            if self.p.intents.contains_key(n) {
                self.diag(codes::E101, path, format!("the generated intent '{n}' has the name of a declared one")).help =
                    Some(format!("rename the declared '{n}'"));
            }
        }
    }

    fn field_rules(&mut self, entity: &str, e: &Entity, f: &Field, path: &str) {
        let policies: Vec<(&str, Option<&RefPolicy>)> = match &f.kind {
            FieldKind::Ref { on_delete, on_erase, .. } | FieldKind::RefUnion { on_delete, on_erase, .. } => {
                vec![("on_delete", on_delete.as_ref()), ("on_erase", on_erase.as_ref())]
            }
            _ => Vec::new(),
        };
        // references to personal data must say what happens on erase
        let target_personal = |t: &str| self.entity(t).is_some_and(|x| x.personal);
        let refs_personal = match &f.kind {
            FieldKind::Ref { target, .. } => target_personal(target),
            FieldKind::RefUnion { targets, .. } => targets.iter().any(|t| target_personal(t)),
            _ => false,
        };
        let own_target = matches!(&f.kind, FieldKind::Ref { target, .. } if target == entity);
        let on_erase = policies.iter().find(|(w, _)| *w == "on_erase").and_then(|(_, p)| *p);
        if refs_personal && on_erase.is_none() && !own_target {
            self.diag(codes::E309, path, format!("{entity}.{} references personal data but does not say what happens when it is erased", f.name))
                .help = Some("add 'on erase cascade | anonymize | restrict | reassign to <expr>'".into());
        }
        if matches!(on_erase, Some(RefPolicy::Anonymize)) && !f.optional {
            self.diag(
                codes::E309,
                &format!("{path}.kind.on_erase"),
                format!("{entity}.{} is anonymized on erase and must be optional ('{}?')", f.name, ty_name(&f.ty)),
            );
        }
        for (which, pol) in &policies {
            if matches!(pol, Some(RefPolicy::SetNull)) && !f.optional {
                self.diag(codes::E201, &format!("{path}.kind.{which}"), format!("'set null' needs an optional field ('{}?')", ty_name(&f.ty)));
            }
            if let Some(RefPolicy::Reassign { to }) = pol {
                self.expr(to, &format!("{path}.kind.{which}"));
            }
        }
        match &f.generated {
            Some(Generated::Tree { .. }) if entity_of_type(&f.ty).as_deref() != Some(entity) => {
                self.diag(codes::E201, &format!("{path}.generated"), format!("'tree' needs a reference to {entity} itself"));
            }
            Some(Generated::Sequence { .. } | Generated::Slug { .. } | Generated::Position { .. }) if !matches!(f.ty, Type::Text { .. }) => {
                self.diag(codes::E201, &format!("{path}.generated"), "sequence/slug/position fields are Text");
            }
            _ => {}
        }
        if f.encrypted {
            let at = format!("{path}.encrypted");
            if !matches!(f.kind, FieldKind::Stored) || !matches!(f.ty, Type::Text { .. } | Type::RichText { .. } | Type::Email | Type::Url | Type::Phone { .. }) {
                self.diag(codes::E318, &at, format!("{entity}.{} is encrypted and must be a stored text field, not {}", f.name, ty_name(&norm(&f.ty))))
                    .help = Some("encrypt Text, Email, Url or Phone fields only".into());
            }
            if f.default.is_some() || f.generated.is_some() {
                self.diag(codes::E318, &at, format!("{entity}.{} is encrypted and cannot have a default or a generated value: it would be stored in the clear", f.name));
            }
            for other in &e.fields {
                if let Some(Generated::Slug { from, .. }) = &other.generated
                    && *from == f.name
                {
                    self.diag(codes::E318, &at, format!("{entity}.{} is encrypted, so the slug of {} cannot be made from it", f.name, other.name));
                }
            }
        }
        if matches!(f.kind, FieldKind::Counter { .. }) && !matches!(f.ty, Type::Int { .. }) {
            self.diag(codes::E201, &format!("{path}.kind"), "counter fields are Int");
        }
        if let FieldKind::Inverse { target, via } = &f.kind {
            // a missing entity or field is `validate`'s I107; here the field exists but does not point back
            let back_ok = self.entity(target).and_then(|t| t.fields.iter().find(|x| x.name == *via)).is_none_or(|b| match &b.kind {
                FieldKind::Ref { target: t, .. } => t == entity,
                _ => false,
            });
            if !back_ok {
                self.diag(
                    codes::E106,
                    &format!("{path}.kind"),
                    format!("{entity}.{} is 'via {via}', but {target}.{via} is not a reference to {entity}", f.name),
                )
                .help = Some(format!("declare '{via}: {entity}' on {target}"));
            }
        }
        if let Type::Json { validated_by: Some(by), .. } = &f.ty {
            let head = by.split('.').next().unwrap_or_default();
            let vpath = format!("{path}.ty");
            match e.fields.iter().find(|x| x.name == head).map(|x| &x.ty) {
                Some(Type::Snapshot { entity: t }) => {
                    if self.entity(t).is_none_or(|x| x.traits.dynamic_schema.is_none()) {
                        self.diag(codes::E302, &vpath, format!("{t} has no 'dynamic schema from ...' and cannot validate {}", f.name));
                    }
                }
                Some(Type::Ref { entity: t }) => {
                    self.diag(
                        codes::E312,
                        &vpath,
                        format!("'{}' validates against {t}, which can change later; pin it with Snapshot<{t}>", f.name),
                    )
                    .help = Some(format!("declare '{head}: Snapshot<{t}>' so old answers keep the schema they were written against"));
                }
                // a head that is no field at all is `validate`'s I121
                Some(_) => {
                    self.diag(codes::E312, &vpath, format!("'validated by {by}' must name a Snapshot field of {entity}"));
                }
                None => {}
            }
        }
    }

    fn constraint_rules(&mut self, e: &Entity, c: &'p Constraint, path: &str) {
        let field = |n: &str| e.fields.iter().find(|f| f.name == n);
        match c {
            Constraint::Unique { fields, filter, .. } => {
                for col in fields {
                    if field(col).is_some_and(|f| f.encrypted) {
                        self.diag(codes::E318, path, format!("'{col}' is encrypted and cannot be part of a unique constraint: equal values have different ciphertexts"));
                    }
                    if field(col).is_some_and(|f| matches!(f.kind, FieldKind::Inverse { .. } | FieldKind::Counter { .. })) {
                        self.diag(codes::E201, path, format!("'{col}' cannot be part of a unique constraint"));
                    }
                }
                if let Some(f) = filter {
                    self.expr(f, &format!("{path}.filter"));
                }
            }
            Constraint::Cardinality { filter, repair, per, .. } => {
                if field(per).is_some_and(|f| f.encrypted) {
                    self.diag(codes::E318, path, format!("'{per}' is encrypted and cannot group rows: equal values have different ciphertexts"));
                }
                self.expr(filter, &format!("{path}.filter"));
                if let Some(r) = repair {
                    self.stmts(r, &format!("{path}.repair"));
                }
            }
            Constraint::NoOverlap { range, per, .. } => {
                if field(per).is_some_and(|f| f.encrypted) || field(range).is_some_and(|f| f.encrypted) {
                    self.diag(codes::E318, path, "an encrypted field cannot be used by no overlap: the database compares the ciphertext");
                }
                if let Some(f) = field(range)
                    && !matches!(f.ty, Type::Range { .. })
                {
                    self.diag(codes::E201, path, format!("no overlap needs a Range field; {range} is {}", ty_name(&norm(&f.ty))));
                }
            }
            Constraint::Capacity { count, limit, .. } => {
                self.set(count, &format!("{path}.count"));
                self.expr(limit, &format!("{path}.limit"));
            }
            Constraint::Invariant { cond, .. } => self.expr(cond, &format!("{path}.cond")),
        }
    }

    // ---------- intents ----------

    fn require(&mut self, r: &Require, path: &str) {
        if let Some(w) = &r.when {
            self.expr(w, &format!("{path}.when"));
        }
        self.expr(&r.cond, &format!("{path}.cond"));
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

    fn query(&mut self, name: &str, q: &'p Query, path: &str) {
        self.params(&q.params, path);
        for (i, l) in q.lets.iter().enumerate() {
            self.expr(&l.value, &format!("{path}.lets[{i}]"));
        }
        match &q.allow {
            Some(a) => self.expr(&a.cond, &format!("{path}.allow")),
            None => {
                self.diag(codes::E301, &format!("{path}.name"), format!("query {name} has no 'allow' clause")).help = Some(NO_ALLOW_HELP.into());
            }
        }
        for (i, (c, _)) in q.fetches.iter().enumerate() {
            self.call_args(c, &format!("{path}.fetches[{i}]"));
        }
        if q.source.is_none() && q.fetches.is_empty() {
            self.diag(codes::E100, &format!("{path}.name"), format!("query {name} needs 'from' (or 'fetch' for external data)"));
        }
        if let Some(QuerySource::Call { call, .. }) = &q.source {
            self.call_args(call, &format!("{path}.source"));
        }
        if let Some(QuerySource::Entity { entity, .. }) = &q.source {
            self.tenant_read(entity, false, &format!("{path}.source"));
        }
        if let Some(QuerySource::Search { search, query, .. }) = &q.source {
            self.expr(query, &format!("{path}.source"));
            let entity = self.p.forms.iter().find_map(|f| match f {
                Form::Search(x) if x.name == *search => Some(x.entity.clone()),
                _ => None,
            });
            if let Some(e) = entity {
                self.tenant_read(&e, false, &format!("{path}.source"));
            }
        }
        if q.drafts {
            self.drafts_rules(name, q, path);
        }
        if let Some(f) = &q.filter {
            self.expr(f, &format!("{path}.filter"));
        }
        for (i, g) in q.group_by.iter().enumerate() {
            self.expr(g, &format!("{path}.group_by[{i}]"));
        }
        match &q.sort {
            Some(Sort::Keys { keys }) => self.sort_keys(keys, &format!("{path}.sort")),
            Some(Sort::ByParam { param, cases }) => {
                for (k, keys) in cases {
                    self.sort_keys(keys, &format!("{path}.sort.{k}"));
                }
                let en = q.params.iter().find(|x| x.name == *param).and_then(|x| match &x.ty {
                    Type::Enum { name } => self.p.enums.get(name),
                    _ => None,
                });
                if let Some(def) = en {
                    let missing: Vec<&String> = def.values.iter().filter(|v| !cases.iter().any(|(k, _)| k == *v)).collect();
                    if !missing.is_empty() {
                        let list = missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
                        self.diag(codes::E221, &format!("{path}.sort"), format!("sort by {param} does not cover {list}"));
                    }
                }
            }
            None => {}
        }
        if q.page.is_none()
            && matches!(q.source, Some(QuerySource::Entity { .. } | QuerySource::Call { .. } | QuerySource::Search { .. }))
            && q.group_by.is_empty()
        {
            self.diag(codes::W403, &format!("{path}.name"), format!("query {name} returns an unbounded list")).help =
                Some("add 'page <n> by keyset' (or 'by offset max page <n>')".into());
        }
        self.selection(&q.select, &format!("{path}.select"));
        for (i, t) in q.touches.iter().enumerate() {
            self.expr(t, &format!("{path}.touches[{i}]"));
            let tpath = format!("{path}.touches[{i}]");
            match &t.node {
                Node::Field { entity, field, .. } => {
                    let counter = entity
                        .as_deref()
                        .and_then(|e| self.entity(e))
                        .and_then(|e| e.fields.iter().find(|f| f.name == *field))
                        .is_some_and(|f| matches!(f.kind, FieldKind::Counter { .. }));
                    if !counter {
                        self.diag(codes::E204, &tpath, format!("'touch' needs a counter field, '{field}' is not one"));
                    }
                }
                _ => {
                    self.diag(codes::E204, &tpath, "'touch' needs a counter field path");
                }
            }
        }
    }

    /// A `drafts` query reads unpublished work: it needs a publishable entity to read it from and a caller condition of its own.
    fn drafts_rules(&mut self, name: &str, q: &Query, path: &str) {
        let source = match &q.source {
            Some(QuerySource::Entity { entity, .. }) => Some(entity.clone()),
            Some(QuerySource::Param { param, .. }) => q.params.iter().find(|p| p.name == *param).and_then(|p| entity_of_type(&p.ty)),
            _ => None,
        };
        if !source.as_ref().and_then(|e| self.entity(e)).is_some_and(|e| e.traits.publishable) {
            self.diag(codes::E208, &format!("{path}.name"), format!("'drafts' on query {name} needs it to read a publishable entity"));
        }
        if q.allow.as_ref().is_some_and(|a| matches!(a.cond.node, Node::Public)) {
            self.diag(codes::E208, &format!("{path}.allow"), format!("query {name} reads drafts and cannot be 'allow public'")).help =
                Some("unpublished work is for editors: use 'allow authenticated' or a condition on the actor".into());
        }
    }

    fn command(&mut self, name: &str, c: &'p Command, path: &str) {
        self.command_params(&c.params, path);
        if let Idempotency::Derived { key } = &c.idempotency {
            self.expr(key, &format!("{path}.idempotency"));
        }
        for (i, l) in c.lets.iter().enumerate() {
            self.expr(&l.value, &format!("{path}.lets[{i}]"));
        }
        match &c.allow {
            Some(a) => self.expr(&a.cond, &format!("{path}.allow")),
            None => {
                self.diag(codes::E301, &format!("{path}.name"), format!("command {name} has no 'allow' clause")).help = Some(NO_ALLOW_HELP.into());
            }
        }
        for (i, r) in c.requires.iter().enumerate() {
            self.require(r, &format!("{path}.requires[{i}]"));
        }
        self.stmts(&c.body, &format!("{path}.body"));
        // a command that hands out a signing secret cannot be idempotent (its stored response would keep the secret), so it is not asked to be
        let hands_out_secret = c
            .returns
            .as_ref()
            .and_then(|r| r.select.as_ref())
            .is_some_and(|sel| sel.items.iter().any(|it| it.value.as_ref().is_some_and(|v| matches!(v.node, Node::SigningSecret { .. }))));
        if matches!(c.idempotency, Idempotency::None) && !hands_out_secret && c.body.iter().any(|s| matches!(s, Stmt::Insert { bind: Some(_), .. })) {
            self.diag(
                codes::W501,
                &format!("{path}.name"),
                format!("command {name} creates rows but is not idempotent; a client retry creates duplicates"),
            )
            .help = Some("add 'idempotent' so callers send an Idempotency-Key, or rely on a unique constraint".into());
        }
        for (i, e) in c.emits.iter().enumerate() {
            let epath = format!("{path}.emits[{i}]");
            for (n, v) in &e.fields {
                self.expr(v, &format!("{epath}.{n}"));
            }
            if let Some(k) = e.to.as_ref().and_then(|t| t.key.as_ref()) {
                self.expr(k, &format!("{epath}.to.key"));
            }
            self.emits.push((epath, e));
        }
        if let Some(r) = &c.returns {
            self.expr(&r.value, &format!("{path}.returns"));
            if let Some(s) = &r.select {
                let mut rows = Vec::new();
                inserted_rows(&c.body, &mut rows);
                self.secret_rows = Some((rows, !matches!(c.idempotency, Idempotency::None)));
                self.selection(s, &format!("{path}.returns.select"));
                self.secret_rows = None;
            }
        }
    }

    /// One shape per event: the declaration, or the first emission of an undeclared event.
    fn events(&mut self) {
        let emits = std::mem::take(&mut self.emits);
        for (path, e) in &emits {
            let Some(def) = self.p.events.get(&e.event) else { continue };
            let prev: Vec<(String, Type)> = def.fields.iter().map(|(n, t)| (n.clone(), norm(t))).collect();
            let shape_of = |e: &Emit| -> Vec<(String, Type)> { e.fields.iter().map(|(n, v)| (n.clone(), norm(&v.ty))).collect() };
            let compatible = |got: &[(String, Type)]| {
                prev.len() == got.len() && prev.iter().all(|(n, t)| got.iter().any(|(m, u)| m == n && (accepts(t, u) || accepts(u, t))))
            };
            let got = shape_of(e);
            if compatible(&got) {
                continue;
            }
            // the shape this one disagrees with: the declaration, else the first emission that has it
            let related = if def.declared {
                format!("events.{}", e.event)
            } else {
                emits
                    .iter()
                    .find(|(_, o)| o.event == e.event && compatible(&shape_of(o)))
                    .map(|(p, _)| p.clone())
                    .unwrap_or_else(|| format!("events.{}", e.event))
            };
            let d = self.diag(codes::E216, path, format!("event {} is emitted with a different shape than at {related}", e.event));
            d.related = Some(related);
            d.help = Some(format!("expected {{ {} }}, got {{ {} }}", sig(&prev), sig(&got)));
        }
    }

    // ---------- statements ----------

    fn stmts(&mut self, v: &'p [Stmt], path: &str) {
        let saved = self.pending.len();
        for s in v {
            if let Stmt::Effect { into: Some(Expr { node: Node::Field { base, field, .. }, .. }), .. } = s
                && let Node::Local { name } = &base.node
            {
                self.pending.push((name.clone(), field.clone()));
            }
        }
        for (i, s) in v.iter().enumerate() {
            self.stmt(s, &format!("{path}[{i}]"));
        }
        self.pending.truncate(saved);
    }

    /// Fields an insert must give: written by callers, not optional, no default, not generated by the runtime.
    fn missing_fields(&self, entity: &str, given: &[&str], bind: Option<&str>) -> Vec<String> {
        let Some(e) = self.entity(entity) else { return Vec::new() };
        let filled_later = |f: &str| bind.is_some_and(|b| self.pending.iter().any(|(pb, pf)| pb == b && pf == f));
        e.fields
            .iter()
            .filter(|f| {
                let writable = matches!(f.kind, FieldKind::Stored | FieldKind::Ref { .. } | FieldKind::RefUnion { .. });
                let runtime_value = f.default.is_some()
                    || matches!(f.generated, Some(Generated::Sequence { .. } | Generated::Slug { .. } | Generated::Position { .. }));
                writable && !f.optional && !runtime_value && !given.contains(&f.name.as_str()) && !filled_later(&f.name)
            })
            .map(|f| f.name.clone())
            .collect()
    }

    fn insert_rule(&mut self, entity: &str, values: &[(String, Expr)], bind: Option<&str>, path: &str) {
        let given: Vec<&str> = values.iter().map(|(n, _)| n.as_str()).collect();
        let missing = self.missing_fields(entity, &given, bind);
        if !missing.is_empty() {
            self.diag(codes::E205, path, format!("insert {entity} is missing {}", missing.join(", ")));
        }
    }

    fn stmt(&mut self, s: &'p Stmt, path: &str) {
        match s {
            Stmt::Let(l) => self.expr(&l.value, path),
            Stmt::Insert { entity, from, values, bind } => {
                if let Some(f) = from {
                    self.set(f, path);
                }
                self.values(entity, values, from.is_none(), path);
                self.insert_rule(entity, values, bind.as_deref(), path);
            }
            Stmt::Upsert { entity, keys, values, bind } => {
                let has_unique = self
                    .entity(entity)
                    .is_some_and(|e| e.constraints.iter().any(|c| matches!(c, Constraint::Unique { fields, filter: None, .. } if fields == keys)));
                if !has_unique && self.entity(entity).is_some() {
                    let list = keys.join(", ");
                    self.diag(codes::E207, path, format!("upsert {entity} by ({list}) needs a matching 'unique ({list})' constraint"));
                }
                self.values(entity, values, false, path);
                self.insert_rule(entity, values, bind.as_deref(), path);
            }
            Stmt::Toggle { entity, values } => {
                let names: Vec<&str> = values.iter().map(|(n, _)| n.as_str()).collect();
                let has = self.entity(entity).is_some_and(|e| {
                    e.constraints.iter().any(|c| match c {
                        Constraint::Unique { fields, filter: None, .. } => {
                            let mut a: Vec<&str> = fields.iter().map(String::as_str).collect();
                            let mut b = names.clone();
                            a.sort();
                            b.sort();
                            a == b
                        }
                        _ => false,
                    })
                });
                if !has && self.entity(entity).is_some() {
                    self.diag(codes::E207, path, format!("toggle {entity} needs a unique constraint over exactly ({})", names.join(", ")));
                }
                self.values(entity, values, false, path);
                self.insert_rule(entity, values, None, path);
            }
            Stmt::Update { target, via, assigns } => {
                self.set(target, &format!("{path}.filter"));
                if let Some(v) = via {
                    self.expr(&v.path, path);
                }
                self.assigns(assigns, true, path);
            }
            Stmt::Delete { target } | Stmt::Purge { target } => self.set(target, &format!("{path}.filter")),
            Stmt::Erase { target } => {
                self.expr(target, path);
                let entity = match &target.ty {
                    Type::Ref { entity } | Type::Snapshot { entity } => Some(entity),
                    _ => None,
                };
                if let Some(e) = entity
                    && !self.entity(e).is_some_and(|x| x.personal)
                {
                    self.diag(codes::E310, &format!("{path}.target"), format!("erase applies to 'personal' entities; {e} is not personal"));
                }
            }
            Stmt::Set { assigns } => self.assigns(assigns, false, path),
            Stmt::When { cond, body } => {
                self.expr(cond, &format!("{path}.cond"));
                self.stmts(body, &format!("{path}.body"));
            }
            Stmt::Each { source, body } => {
                self.set(source, &format!("{path}.filter"));
                self.stmts(body, &format!("{path}.body"));
            }
            Stmt::Effect { call, into, on_failure, .. } => {
                self.call_args(call, path);
                if let Some(i) = into {
                    self.expr(i, path);
                }
                if let Some(f) = on_failure {
                    self.stmts(f, &format!("{path}.on_failure"));
                }
            }
            Stmt::Reserve { of, .. } => self.expr(of, path),
            Stmt::AtRun { at, args, .. } => {
                self.expr(at, path);
                for a in args {
                    self.expr(&a.value, path);
                }
            }
            Stmt::Notify(n) => {
                self.set(&n.to, path);
                self.call_args(&n.via, path);
                for (f, v) in &n.fields {
                    self.expr(v, &format!("{path}.{f}"));
                }
                if let (Some(e), Some(actor)) = (set_entity(&n.to), self.actor_entity())
                    && e != actor
                {
                    let refs = self
                        .entity(&e)
                        .map(|x| x.fields.iter().filter(|f| matches!(&f.kind, FieldKind::Ref { target, .. } if target == actor)).count())
                        .unwrap_or(0);
                    if refs != 1 {
                        self.diag(
                            codes::E311,
                            &format!("{path}.to"),
                            format!("cannot tell who to notify from {e}: it has {refs} references to {actor}"),
                        )
                        .help = Some(format!("notify a set of {actor} rows, or an entity with exactly one {actor} field"));
                    }
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

    /// `one_row`: the statement writes exactly one new row, so the value can be encrypted for that row's id.
    fn values(&mut self, entity: &str, values: &[(String, Expr)], one_row: bool, path: &str) {
        for (f, v) in values {
            if self.encrypted(entity, f) && (!one_row || !plain_value(v)) {
                self.diag(codes::E320, &format!("{path}.{f}"), format!("{entity}.{f} is encrypted: it can only be written to one new row from a parameter"))
                    .help = Some("insert the row with the parameter as the value, or use `set row.field = param`".into());
            }
            self.expr(v, &format!("{path}.{f}"));
        }
    }

    /// `many`: the target is a set of rows (`update ... where`), not one row.
    fn assigns(&mut self, assigns: &[Assign], many: bool, path: &str) {
        for (i, a) in assigns.iter().enumerate() {
            let target = format!("{path}.assigns[{i}].target");
            let enc = match &a.target.node {
                Node::Field { base, entity, field } => row_entity(base, entity).filter(|e| self.encrypted(e, field)).map(|e| (e, field.clone(), Some(base))),
                Node::RowField { entity, field } if self.encrypted(entity, field) => Some((entity.clone(), field.clone(), None)),
                _ => None,
            };
            match enc {
                Some((entity, field, base)) => {
                    // the target names the field to write, it does not read it
                    if let Some(b) = base {
                        self.expr(b, &target);
                    }
                    if many || !matches!(a.op, AssignOp::Set) || !plain_value(&a.value) {
                        self.diag(codes::E320, &target, format!("{entity}.{field} is encrypted: write it with `set row.{field} = <parameter>` on one row"))
                            .help = Some("an update over many rows, `+=`, or a computed value cannot be encrypted for each row".into());
                    }
                }
                None => self.expr(&a.target, &target),
            }
            self.expr(&a.value, &format!("{path}.assigns[{i}].value"));
        }
    }

    // ---------- expressions ----------

    fn sort_keys(&mut self, keys: &[SortKey], path: &str) {
        for (i, k) in keys.iter().enumerate() {
            self.expr(&k.expr, &format!("{path}[{i}]"));
        }
    }

    fn set(&mut self, s: &SetExpr, path: &str) {
        if let Some(entity) = set_entity(s) {
            let bound = matches!(&s.source, SetSource::Expr { expr } if tenant::bound(self.p, &self.tn.checked, expr));
            self.tenant_read(&entity, bound, path);
        }
        if let SetSource::Expr { expr } = &s.source {
            self.expr(expr, path);
        }
        if let Some(f) = &s.filter {
            self.expr(f, path);
        }
    }

    fn call_args(&mut self, c: &Call, path: &str) {
        if matches!(c.callee, Callee::Fn { .. } | Callee::Relation { .. }) && c.args.len() == 2 && c.args[0].value == c.args[1].value {
            self.diag(codes::W402, path, format!("{} is called with the same expression twice", callee_name(c)));
        }
        for a in &c.args {
            self.expr(&a.value, path);
        }
    }

    fn encrypted_read(&mut self, entity: &str, field: &str, path: &str) {
        self.diag(codes::E319, path, format!("{entity}.{field} is encrypted: the database only holds ciphertext, so an expression cannot use its value")).help =
            Some("select the field by name, or keep an unencrypted field for what you need to compare, filter or sort by".into());
    }

    fn expr(&mut self, e: &Expr, path: &str) {
        match &e.node {
            Node::Lit { .. }
            | Node::Param { .. }
            | Node::Local { .. }
            | Node::Symbol { .. }
            | Node::Config { .. }
            | Node::RecordField { .. }
            | Node::EnumValue { .. }
            | Node::This
            | Node::Now
            | Node::Today
            | Node::SearchRank { .. }
            | Node::Public => {}
            Node::SigningSecret { base } => {
                self.expr(base, path);
                let entity = match &base.node {
                    Node::Local { name } => {
                        self.secret_rows.as_ref().and_then(|(rows, _)| rows.iter().find(|(b, _)| b == name)).map(|(_, e)| e.clone())
                    }
                    _ => None,
                };
                if self.secret_rows.as_ref().is_some_and(|(_, idempotent)| *idempotent) {
                    self.diag(codes::E208, path, "an idempotent command keeps its response, which would store the signing secret").help =
                        Some("remove `idempotent` and make a repeated registration harmless with a unique constraint on the endpoint".into());
                }
                let endpoint = entity.as_ref().is_some_and(|e| self.p.forms.iter().any(|f| matches!(f, Form::OutboundWebhooks(o) if o.entity == *e)));
                if !endpoint {
                    self.diag(codes::E208, path, "'signingSecret' is only readable in the 'returns' of the command that inserted the endpoint row").help = Some(
                        "write `insert Endpoint { ... } as ep` and `returns ep { id secret: ep.signingSecret }`: the secret is shown once, when the endpoint is created".into(),
                    );
                }
            }
            Node::Actor => {
                if self.p.actor.is_none() {
                    self.diag(codes::E110, path, "'actor' used but no actor is declared").help =
                        Some("add 'actor <Entity> via auth.oidc(...)' at top level".into());
                }
            }
            Node::Authenticated => {
                if self.p.actor.is_none() {
                    self.diag(codes::E110, path, "'authenticated' used but no actor is declared");
                }
            }
            Node::SelfRow => {
                if !self.self_ok {
                    self.diag(codes::E306, path, "'self' is only valid in field visibility/masking of the actor entity").help =
                        Some("in other entities, name the owner explicitly, e.g. 'visible to author = actor'".into());
                }
            }
            Node::Field { base, entity, field } => {
                if let Some(e) = row_entity(base, entity).filter(|e| self.encrypted(e, field)) {
                    self.encrypted_read(&e, field, path);
                }
                self.expr(base, path);
            }
            Node::RowField { entity, field } => {
                if self.encrypted(entity, field) {
                    self.encrypted_read(entity, field, path);
                }
            }
            Node::Unary { arg, .. } => self.expr(arg, path),
            Node::Binary { op, l, r } => {
                if matches!(op, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) && l == r {
                    self.diag(codes::W402, path, "both sides of this comparison are the same expression").help =
                        Some("this is almost always a mistake (e.g. comparing a member's role with itself)".into());
                }
                self.expr(l, path);
                self.expr(r, path);
            }
            Node::InList { value, list } => {
                if list.is_empty() {
                    self.diag(codes::E210, path, "'in' needs at least one value");
                }
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
            Node::TypeTest { value, .. } => self.expr(value, path),
            Node::Pred { row, .. } => self.expr(row, path),
            Node::HasScope { subject, scope } => {
                if !self.p.actor.as_ref().is_some_and(|a| a.scopes.iter().any(|s| s == scope)) {
                    self.diag(codes::E103, path, format!("scope '{scope}' is not declared on the actor"));
                }
                self.expr(subject, path);
            }
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
            Node::Call(c) => self.call_args(c, path),
        }
    }

    // ---------- reactions and forms ----------

    fn reaction(&mut self, r: &'p Reaction, path: &str) {
        let p = self.p;
        match r {
            Reaction::Event { cross_tenant, event, when, body, .. } => {
                // the row the event names fixes the tenant; an event without one cannot read scoped rows
                self.tn = TenantCtx { enforce: !cross_tenant, anchored: tenant::event_anchor(p, event).is_some(), checked: Vec::new() };
                if let Some(w) = when {
                    self.expr(w, path);
                }
                self.stmts(body, &format!("{path}.body"));
            }
            Reaction::Schedule(s) => {
                if s.at.is_some() && s.tz.is_none() {
                    self.diag(codes::W404, path, format!("schedule {} has a time of day but no timezone", s.name)).help =
                        Some("add tz \"Asia/Seoul\" (or another IANA zone)".into());
                }
                // the sweep enumerates every tenant; each item then fixes the tenant of its own body
                self.tn = TenantCtx::default();
                let item_anchored = s.for_each.as_ref().and_then(set_entity).is_some_and(|e| tenant::has_tenant(p, &e));
                if let Some(f) = &s.for_each {
                    self.set(f, path);
                }
                self.tn = TenantCtx { enforce: !s.cross_tenant, anchored: item_anchored, checked: Vec::new() };
                self.stmts(&s.body, &format!("{path}.body"));
            }
            Reaction::Retain { notify, .. } => {
                if let Some(n) = notify {
                    self.call_args(&n.via, path);
                }
            }
            Reaction::Rule { cross_tenant, entity, when, body, .. } => {
                self.tn = TenantCtx { enforce: !cross_tenant, anchored: tenant::has_tenant(p, entity), checked: Vec::new() };
                self.expr(when, path);
                self.stmts(body, &format!("{path}.body"));
            }
            // no row names the tenant before the payload is read, so these read without a tenant filter;
            // their writes are still bound to one tenant by the database unless the declaration says `cross tenant`
            Reaction::Webhook(w) => {
                self.webhook(w, path);
                for (i, h) in w.handlers.iter().enumerate() {
                    self.stmts(&h.body, &format!("{path}.handlers[{i}].body"));
                }
            }
            Reaction::Consume { body, .. } => self.stmts(body, &format!("{path}.body")),
        }
    }

    fn webhook(&mut self, w: &Webhook, path: &str) {
        let via = callee_name(&w.via);
        for (i, a) in w.via.args.iter().enumerate() {
            let apath = format!("{path}.via.args[{i}]");
            match &a.name {
                Some(n) if WEBHOOK_OPTIONS.contains(&n.as_str()) => {
                    if !matches!(a.value.node, Node::Lit { lit: Literal::Text(_) }) {
                        self.diag(codes::E214, &apath, format!("'{n}' must be a string literal"));
                    }
                }
                Some(n) => {
                    self.diag(codes::E214, &apath, format!("unknown webhook option '{n}' (known: {})", WEBHOOK_OPTIONS.join(", ")));
                }
                None => {
                    self.diag(codes::E214, &apath, "webhook options are named (e.g. secret: \"ENV_NAME\")");
                }
            }
        }
        if via == "http.webhook" && !w.via.args.iter().any(|a| a.name.as_deref() == Some("secret")) {
            self.diag(codes::E214, &format!("{path}.via"), "http.webhook needs secret: \"<ENV VAR NAME>\" (the secret itself never goes in source)");
        }
        let mut seen = std::collections::BTreeSet::new();
        for (i, h) in w.handlers.iter().enumerate() {
            if !seen.insert(h.event.as_str()) {
                self.diag(codes::E101, &format!("{path}.handlers[{i}]"), format!("event '{}' is handled twice", h.event));
            }
        }
    }

    fn outbound_rules(&mut self, o: &OutboundWebhooks, path: &str) {
        if self.encrypted(&o.entity, "url") {
            self.diag(codes::E318, path, format!("{}.url is encrypted, but the delivery worker reads it as the address to call", o.entity));
        }
        if o.sign != "hmac_sha256" {
            self.diag(codes::E317, path, format!("signature scheme '{}' is not supported", o.sign)).help = Some("use `sign hmac_sha256`".into());
        }
        if !(0..=MAX_OUTBOUND_RETRIES).contains(&o.retry) {
            self.diag(codes::E317, path, format!("retry {} is outside 0 to {MAX_OUTBOUND_RETRIES}", o.retry));
        }
        if o.over_seconds < 1 {
            self.diag(codes::E317, path, "the retry window (`over`) must be positive");
        }
        if o.disable_after_seconds.is_some_and(|d| d < 1) {
            self.diag(codes::E317, path, "`disable after` must be a positive duration");
        }
        if o.events.is_empty() {
            self.diag(codes::E317, path, "no event to deliver");
        }
        let first = self.p.forms.iter().position(|f| matches!(f, Form::OutboundWebhooks(x) if x.entity == o.entity));
        let me = self.p.forms.iter().position(|f| matches!(f, Form::OutboundWebhooks(x) if std::ptr::eq(x, o)));
        if first != me {
            self.diag(codes::E317, path, format!("{} already has an outbound webhooks form", o.entity));
        }
        let url_ok = self
            .p
            .entities
            .get(&o.entity)
            .and_then(|e| e.fields.iter().find(|f| f.name == "url"))
            .is_some_and(|f| matches!(f.ty, Type::Url | Type::Text { .. }));
        if self.p.entities.contains_key(&o.entity) && !url_ok {
            self.diag(codes::E317, path, format!("{} has no `url: Url` field to deliver to", o.entity));
        }
        // the event names the tenant of the endpoints it goes to; an event without one would reach every tenant's endpoints
        if tenant::has_tenant(self.p, &o.entity) {
            for e in &o.events {
                if tenant::event_anchor(self.p, e).is_none() {
                    self.diag(
                        codes::E314,
                        path,
                        format!("event {e} names no tenant-scoped row, so it cannot be delivered to the endpoints of one tenant"),
                    )
                    .help = Some("give the event a field that references a row of a tenant-scoped entity".into());
                }
            }
        }
    }

    fn search_rules(&mut self, s: &Search, path: &str) {
        let lang = s.language.as_deref();
        if let Some(l) = lang
            && !builtin::SEARCH_LANGUAGES.contains(&l)
        {
            self.diag(codes::E316, path, format!("search {} names language '{l}', which has no text analysis here", s.name)).help =
                Some(format!("use one of: {}", builtin::SEARCH_LANGUAGES.join(", ")));
        }
        if s.fields.is_empty() {
            self.diag(codes::E316, path, format!("search {} has no field", s.name));
        }
        if self.p.entities.get(&s.entity).is_some_and(|e| e.traits.publishable) {
            self.diag(codes::E316, path, format!("search {} covers {}, which is publishable: readers see the published copy", s.name, s.entity));
        }
        let mut seen = std::collections::BTreeSet::new();
        for (f, w) in &s.fields {
            if !seen.insert(f.as_str()) {
                self.diag(codes::E316, path, format!("search {} names field '{f}' twice", s.name));
            }
            if let Some(w) = w
                && !builtin::SEARCH_WEIGHTS.contains(&w.as_str())
            {
                self.diag(codes::E316, path, format!("weight '{w}' of '{f}' is not one of A, B, C, D"));
            }
            if self.encrypted(&s.entity, f) {
                self.diag(codes::E318, path, format!("'{}.{f}' is encrypted, so a search cannot index it", s.entity));
            }
            let ty = self.p.entities.get(&s.entity).and_then(|e| e.fields.iter().find(|x| x.name == *f)).map(|x| &x.ty);
            if let Some(t) = ty
                && !matches!(t, Type::Text { .. } | Type::RichText { .. } | Type::Email | Type::Url | Type::Phone { .. })
            {
                self.diag(codes::E316, path, format!("'{}.{f}' is not text, so a search cannot index it", s.entity));
            }
        }
    }

    fn form(&mut self, f: &'p Form, path: &str) {
        match f {
            Form::Subscribe(s) => {
                // a subscription is read like a query: its parameters fix the tenant it reads in
                self.form_tenant(&s.params);
                self.params(&s.params, path);
                self.expr(&s.allow.cond, path);
                if let QuerySource::Call { call, .. } = &s.source {
                    self.call_args(call, path);
                }
                if let QuerySource::Entity { entity, .. } = &s.source {
                    self.tenant_read(entity, false, &format!("{path}.source"));
                }
                if let Some(x) = &s.filter {
                    self.expr(x, path);
                }
                self.selection(&s.select, path);
            }
            Form::Projection(p) => {
                for (_, _, st) in &p.handlers {
                    self.stmt(st, path);
                }
            }
            Form::Search(x) => self.search_rules(x, path),
            Form::Consent(c) => self.consent_rules(c, path),
            Form::Job(j) => {
                self.form_tenant(&j.params);
                self.params(&j.params, path);
                self.expr(&j.allow.cond, path);
                if let Some(p) = &j.progress {
                    self.set(p, path);
                }
                if let Some(b) = &j.body {
                    self.stmts(b, &format!("{path}.body"));
                }
                if let Some((e, c)) = &j.notify {
                    self.expr(e, path);
                    self.call_args(c, path);
                }
            }
            Form::Verification(v) => {
                // the target is the form's parameter: an entity of a tenant fixes it
                self.tn = TenantCtx {
                    enforce: true,
                    anchored: entity_of_type(&v.target).is_some_and(|e| tenant::has_tenant(self.p, &e)),
                    checked: Vec::new(),
                };
                if let Some(w) = &v.target_where {
                    self.expr(w, path);
                }
                self.call_args(&v.deliver, path);
                self.stmts(&v.on_verified.2, &format!("{path}.on_verified"));
            }
            Form::GrantLink(g) => {
                self.form_tenant(&g.scope);
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
                // the row the approval is about fixes the tenant of the voters and of what approval does
                self.tn = TenantCtx { enforce: true, anchored: tenant::has_tenant(self.p, &a.entity), checked: Vec::new() };
                if a.required < 1 {
                    self.diag(codes::E210, path, "an approval needs at least 1 approval");
                }
                self.set(&a.approvers, path);
                if let Some(r) = &a.requested_by {
                    self.expr(r, path);
                }
                self.stmts(&a.on_approved, &format!("{path}.on_approved"));
                self.stmts(&a.on_rejected, &format!("{path}.on_rejected"));
            }
            Form::OutboundWebhooks(o) => {
                self.outbound_rules(o, path);
                // the event fixes the tenant of the endpoints it goes to, like the handlers of that event
                let anchored = o.events.iter().all(|e| tenant::event_anchor(self.p, e).is_some());
                self.tn = TenantCtx { enforce: true, anchored, checked: Vec::new() };
                if let Some(x) = &o.filter {
                    self.expr(x, path);
                }
            }
            Form::Impersonate(i) => self.impersonate_rules(i, path),
            Form::Migration(m) => self.stmts(&m.body, &format!("{path}.body")),
            Form::Upcast(u) => {
                for (f, v) in &u.with {
                    self.expr(v, &format!("{path}.{f}"));
                }
            }
        }
    }
}

impl<'p> A<'p> {
    fn consent_rules(&mut self, c: &Consent, path: &str) {
        if c.version < 1 {
            self.diag(codes::E210, path, format!("consent {} has version {}; versions start at 1", c.name, c.version));
        }
        self.clashes(&[format!("Give{}Consent", c.name), format!("Withdraw{}Consent", c.name), format!("{}ConsentStatus", c.name)], path);
        for i in &c.intents {
            let callable = match self.p.intents.get(i) {
                Some(Intent::Query(q)) => !q.internal,
                Some(Intent::Command(cmd)) => !cmd.internal,
                None => self.p.forms.iter().any(|f| matches!(f, Form::Job(j) if j.name == *i)),
            };
            if !callable {
                self.diag(
                    codes::E208,
                    path,
                    format!(
                        "consent {} lists '{i}', which a caller cannot invoke: only declared commands, queries and jobs can need consent",
                        c.name
                    ),
                );
            }
        }
    }

    fn impersonate_rules(&mut self, i: &Impersonate, path: &str) {
        match self.actor_entity() {
            None => {
                self.diag(codes::E110, path, "'impersonate' needs an actor declaration");
            }
            Some(a) if a != i.entity => {
                self.diag(
                    codes::E208,
                    path,
                    format!("'impersonate {}' must name the actor entity {a}: an operator acts as a user of the app", i.entity),
                );
            }
            Some(_) => {}
        }
        if i.ttl_seconds < 1 {
            self.diag(codes::E210, path, "an impersonation needs a 'ttl' of at least one second");
        }
        let n = self.p.forms.iter().filter(|f| matches!(f, Form::Impersonate(_))).count();
        if n > 1 {
            self.diag(codes::E101, path, "'impersonate' is declared more than once; one app has one impersonation rule");
        }
        self.clashes(&[format!("Start{}Impersonation", i.entity), format!("Stop{}Impersonation", i.entity)], path);
        self.expr(&i.by, path);
    }
}

const NO_ALLOW_HELP: &str =
    "nothing is exposed implicitly; write 'allow public', 'allow authenticated' or a relation such as 'allow managerOf(actor, club)'";
