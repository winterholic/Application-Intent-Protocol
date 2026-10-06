//! Name resolution and type checking for every declaration kind.
//!
//! Diagnostic codes are catalogued in `spec/diagnostics.md`. The checker never
//! stops at the first error; `Ty::Unknown` suppresses cascades.

use crate::model::{EventShape, FieldKind, Model};
use crate::ty::Ty;
use aip_ir::codes;
use aip_syntax::ast::*;
use aip_syntax::{Diagnostic, Span};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ctx {
    /// Entity-level predicates: invariants, visibility, constraints.
    Entity,
    Query,
    Command,
    Relation,
    /// Record checks
    Record,
}

#[derive(Debug, Clone)]
struct Binding {
    ty: Ty,
}

struct Scope {
    vars: HashMap<String, Binding>,
    /// Bare names fall back to fields of this entity/record (row context).
    row: Option<Ty>,
}

pub struct Checker<'m, 'a> {
    m: &'m Model<'a>,
    scopes: Vec<Scope>,
    ctx: Ctx,
    pub diags: Vec<Diagnostic>,
    pub events: BTreeMap<String, EventShape>,
    /// Type of every checked expression, keyed by span; the backend reads it.
    pub types: HashMap<(u32, u32), Ty>,
    /// Aliases that `from Search.match(q) alias` binds in the query being checked; `alias.rank` is the match score there.
    search_aliases: Vec<String>,
}

fn is_upper_name(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

impl<'m, 'a> Checker<'m, 'a> {
    pub fn new(m: &'m Model<'a>) -> Self {
        Checker {
            m,
            scopes: Vec::new(),
            ctx: Ctx::Command,
            diags: Vec::new(),
            events: m.events.clone(),
            types: HashMap::new(),
            search_aliases: Vec::new(),
        }
    }

    fn err(&mut self, code: &str, msg: impl Into<String>, span: Span) -> &mut Diagnostic {
        self.diags.push(Diagnostic::error(code, msg, span));
        self.diags.last_mut().expect("just pushed")
    }

    fn push(&mut self, row: Option<Ty>) {
        self.scopes.push(Scope { vars: HashMap::new(), row });
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn bind(&mut self, name: &Ident, ty: Ty) {
        if let Some(s) = self.scopes.last_mut() {
            s.vars.insert(name.name.clone(), Binding { ty });
        }
    }

    fn bind_str(&mut self, name: &str, ty: Ty) {
        if let Some(s) = self.scopes.last_mut() {
            s.vars.insert(name.to_string(), Binding { ty });
        }
    }

    fn lookup(&self, name: &str) -> Option<Ty> {
        for s in self.scopes.iter().rev() {
            if let Some(b) = s.vars.get(name) {
                return Some(b.ty.clone());
            }
        }
        None
    }

    fn row_field(&self, name: &str) -> Option<Ty> {
        for s in self.scopes.iter().rev() {
            if let Some(row) = &s.row
                && let Some(t) = self.field_ty(row, name)
            {
                return Some(t);
            }
        }
        None
    }

    fn actor_ty(&self) -> Option<Ty> {
        self.m.actor_entity().map(|a| Ty::Entity(a.to_string()))
    }

    // ---------- fields ----------

    fn field_ty(&self, base: &Ty, name: &str) -> Option<Ty> {
        match base {
            Ty::Entity(e) | Ty::Snapshot(e) => {
                let ent = self.m.entity(e)?;
                ent.field(name).map(|f| f.ty.clone())
            }
            Ty::Record(r) if r.starts_with("event:") => {
                let ev = self.events.get(&r["event:".len()..])?;
                ev.fields.iter().find(|(n, _)| n == name).map(|(_, t)| t.clone())
            }
            Ty::Record(r) => {
                let rec = self.m.records.get(r)?;
                let f = rec.fields.iter().find(|f| f.name.name == name)?;
                Some(self.shallow_type(&f.ty))
            }
            Ty::Range(t) if name == "start" || name == "end" => Some((**t).clone()),
            Ty::Union(_) if name == "id" => Some(Ty::Uuid),
            Ty::Localized(t) => self.field_ty(t, name),
            _ => None,
        }
    }

    /// Record field types resolved without mutating the model (records only
    /// reference names that the model already resolved for entities).
    fn shallow_type(&self, t: &TypeExpr) -> Ty {
        let named = |n: &str| -> Ty {
            match n {
                "Bool" => Ty::Bool,
                "Int" => Ty::Int,
                "Decimal" => Ty::Decimal,
                "Text" => Ty::Text,
                "RichText" => Ty::RichText,
                "Email" => Ty::Email,
                "Url" => Ty::Url,
                "Phone" => Ty::Phone,
                "Time" => Ty::Time,
                "Date" => Ty::Date,
                "Duration" => Ty::Duration,
                "Uuid" => Ty::Uuid,
                "Json" => Ty::Json,
                "Upload" => Ty::Upload,
                n if self.m.enums.contains_key(n) => Ty::Enum(n.into()),
                n if self.m.records.contains_key(n) => Ty::Record(n.into()),
                n if self.m.entities.contains_key(n) => Ty::Entity(n.into()),
                _ => Ty::Unknown,
            }
        };
        match &t.kind {
            TypeKind::Name(q) if q.parts.len() == 1 => named(&q.parts[0].name),
            TypeKind::Name(q) => {
                if q.text() == "s3.Object" {
                    Ty::Object
                } else {
                    Ty::Ext(q.text())
                }
            }
            TypeKind::Refined { base, opts } => match base.name.as_str() {
                "Money" => {
                    Ty::Money(opts.iter().find_map(|o| if let RefineOpt::Word(w) = o { Some(w.name.clone()) } else { None }).unwrap_or_default())
                }
                "Upload" => Ty::Upload,
                other => named(other),
            },
            TypeKind::Generic { base, arg, .. } => {
                let inner = self.shallow_type(arg);
                match base.name.as_str() {
                    "Set" | "List" => Ty::Coll(Box::new(inner)),
                    "Range" => Ty::Range(Box::new(inner)),
                    "Localized" => Ty::Localized(Box::new(inner)),
                    "Snapshot" => match inner {
                        Ty::Entity(e) => Ty::Snapshot(e),
                        _ => Ty::Unknown,
                    },
                    "Json" => Ty::Json,
                    _ => Ty::Unknown,
                }
            }
            TypeKind::JsonValidated(_) => Ty::Json,
            TypeKind::Many(n) => Ty::Coll(Box::new(named(&n.name))),
            TypeKind::Union(a) => Ty::Union(a.iter().map(|x| x.name.clone()).collect()),
        }
    }

    fn param_types(&mut self, params: &[Param]) {
        for p in params {
            let ty = self.shallow_type(&p.ty);
            if ty.is_unknown() {
                self.err(codes::E102, format!("unknown type for parameter '{}'", p.name.name), p.ty.span);
            }
            if let Some(d) = &p.default {
                let dt = self.expr(d, Some(&ty));
                if !ty.accepts(&dt) {
                    self.err(codes::E201, format!("default for '{}' must be {ty}, got {dt}", p.name.name), d.span);
                }
            }
            if self.lookup(&p.name.name).is_some() || p.name.name == "actor" {
                self.err(codes::E101, format!("'{}' shadows another name", p.name.name), p.name.span);
            }
            self.bind(&p.name, ty);
        }
    }

    // ---------- expressions ----------

    fn expect_ty(&mut self, e: &Expr, want: &Ty, what: &str) -> Ty {
        let t = self.expr(e, Some(want));
        if matches!(t, Ty::Coll(_)) && !matches!(want, Ty::Coll(_)) && !t.is_unknown() {
            self.err(codes::E213, format!("{what} is multi-valued ({t}); aggregate it or bind one element"), e.span).help =
                Some("use count(...), exists ..., all(xs x: ...), same(xs x: ...) or 'the ...'".into());
            return Ty::Unknown;
        }
        if !want.accepts(&t) {
            self.err(codes::E201, format!("{what} must be {want}, got {t}"), e.span);
        }
        t
    }

    fn cond(&mut self, e: &Expr, what: &str) {
        self.expect_ty(e, &Ty::Bool, what);
    }

    pub fn expr(&mut self, e: &Expr, expected: Option<&Ty>) -> Ty {
        let t = self.expr_inner(e, expected);
        self.types.insert((e.span.start, e.span.end), t.clone());
        t
    }

    fn expr_inner(&mut self, e: &Expr, expected: Option<&Ty>) -> Ty {
        match &e.kind {
            ExprKind::Int(_) => Ty::Int,
            ExprKind::Decimal(_) => Ty::Decimal,
            ExprKind::Str(_) => Ty::Text,
            ExprKind::Bool(_) => Ty::Bool,
            ExprKind::Null => Ty::Null,
            ExprKind::Duration(_) => Ty::Duration,
            ExprKind::Size(_) => Ty::Size,
            ExprKind::TimeOfDay(..) => Ty::Time,
            ExprKind::Name(id) => self.name(id, expected),
            ExprKind::Kw(k) => match k {
                Kw::Actor => self.actor_ty().unwrap_or(Ty::Unknown),
                Kw::SelfRow => Ty::Bool,
                Kw::This => match self.scopes.iter().rev().find_map(|s| s.row.clone()) {
                    Some(t) => t,
                    None => {
                        self.err(codes::E208, "'this' is only valid inside an entity declaration", e.span);
                        Ty::Unknown
                    }
                },
                Kw::Now => Ty::Time,
                Kw::Today => Ty::Date,
                Kw::Public | Kw::Authenticated => Ty::Bool,
            },
            ExprKind::Field(base, f) => {
                let bt = self.expr(base, None);
                // the score is not an entity field: it belongs to the search hit the alias stands for
                if f.name == "rank" && matches!(&base.kind, ExprKind::Name(n) if self.search_aliases.contains(&n.name)) {
                    return Ty::Decimal;
                }
                // the signing secret of an endpoint row is derived, not stored: where it may be read is a rule of the IR (`analyze`)
                if f.name == "signingSecret"
                    && let Ty::Entity(en) = &bt
                    && self.m.endpoints.contains(en)
                    && self.m.entity(en).is_some_and(|e| e.field("signingSecret").is_none())
                {
                    return Ty::Text;
                }
                self.field_access(&bt, f)
            }
            ExprKind::Call(c) => self.call(c, expected),
            ExprKind::Neg(x) => {
                let t = self.expr(x, None);
                if !(t.is_numeric() || t == Ty::Duration || t.is_unknown()) {
                    self.err(codes::E201, format!("cannot negate {t}"), x.span);
                }
                t
            }
            ExprKind::Not(x) => {
                self.cond(x, "operand of 'not'");
                Ty::Bool
            }
            ExprKind::Binary(op, l, r) => self.binary(*op, l, r, e.span),
            ExprKind::InList(x, items) => {
                let t = self.expr(x, None);
                let elem = match &t {
                    Ty::Coll(i) => (**i).clone(),
                    other => other.clone(),
                };
                for it in items {
                    let ti = self.expr(it, Some(&elem));
                    if !elem.accepts(&ti) {
                        self.err(codes::E201, format!("'in' list item is {ti}, expected {elem}"), it.span);
                    }
                }
                Ty::Bool
            }
            ExprKind::InRange(x, lo, hi) => {
                let t = self.expr(x, None);
                let a = self.expr(lo, Some(&t));
                let b = self.expr(hi, Some(&t));
                for (bound, bt) in [(lo, a), (hi, b)] {
                    if !t.accepts(&bt) {
                        self.err(codes::E201, format!("range bound is {bt}, value is {t}"), bound.span);
                    }
                }
                Ty::Bool
            }
            ExprKind::InExpr(x, r) => {
                let t = self.expr(x, None);
                let rt = self.expr(r, None);
                let elem = match &rt {
                    Ty::Coll(i) | Ty::Range(i) => Some((**i).clone()),
                    Ty::Unknown => None,
                    other => {
                        self.err(codes::E201, format!("right side of 'in' must be a collection or range, got {other}"), r.span);
                        None
                    }
                };
                if let Some(el) = elem {
                    let tt = match &t {
                        Ty::Coll(i) => (**i).clone(),
                        o => o.clone(),
                    };
                    if !el.accepts(&tt) {
                        self.err(codes::E201, format!("{tt} cannot be in a collection of {el}"), e.span);
                    }
                }
                Ty::Bool
            }
            ExprKind::Is(x, name) => {
                let t = self.expr(x, None);
                match &t {
                    Ty::Entity(ent) => {
                        let is_type_test = self.m.entities.contains_key(&name.name) && Some(ent.as_str()) == self.m.actor_entity();
                        let has_pred = self.m.entity(ent).and_then(|i| i.predicate(&name.name)).is_some();
                        if !is_type_test && !has_pred {
                            self.err(codes::E103, format!("{ent} has no predicate '{}'", name.name), name.span).help =
                                Some(format!("declare it on {ent}: 'predicate {} = <condition>'", name.name));
                        }
                    }
                    Ty::Unknown => {}
                    other => {
                        self.err(codes::E201, format!("'is' applies to entity rows, not {other}"), x.span);
                    }
                }
                Ty::Bool
            }
            ExprKind::HasScope(x, scope) => {
                let _ = scope;
                self.expr(x, None);
                Ty::Bool
            }
            ExprKind::Exists(se) => {
                self.push(None);
                self.set_expr(se);
                self.pop();
                Ty::Bool
            }
            ExprKind::The(se) => {
                self.push(None);
                let el = self.set_expr(se);
                self.pop();
                el
            }
            ExprKind::Latest(se, by) => {
                self.push(None);
                let el = self.set_expr(se);
                let bt = self.expr(by, None);
                if !(bt.is_temporal() || bt.is_numeric() || bt.is_unknown()) {
                    self.err(codes::E201, format!("'latest ... by' needs a time or number, got {bt}"), by.span);
                }
                self.pop();
                el
            }
            ExprKind::First(se, keys) => {
                self.push(None);
                let el = self.set_expr(se);
                for k in keys {
                    self.expr(&k.expr, None);
                }
                self.pop();
                el
            }
            ExprKind::Agg { func, set, body } => {
                let Some(se) = set else { return Ty::Int };
                self.push(None);
                let el = self.set_expr(se);
                let bt = body.as_ref().map(|b| self.expr(b, None));
                self.pop();
                match func {
                    AggFn::Count => {
                        if body.is_some() {
                            self.err(codes::E212, "count takes no ': body'; filter with 'where'", e.span);
                        }
                        Ty::Int
                    }
                    _ => {
                        let t = bt.unwrap_or(el);
                        if !(t.is_numeric() || t.is_temporal() || t.is_unknown()) {
                            self.err(codes::E201, format!("cannot aggregate {t}"), e.span);
                        }
                        if *func == AggFn::Avg { Ty::Decimal } else { t }
                    }
                }
            }
            ExprKind::Quant { set, body, .. } => {
                self.push(None);
                self.set_expr(set);
                self.cond(body, "quantifier body");
                self.pop();
                Ty::Bool
            }
            ExprKind::RunningSum { value, over, order } => {
                let t = self.expr(value, None);
                if self.row_field(&over.name).is_none() && self.lookup(&over.name).is_none() {
                    self.err(codes::E104, format!("unknown partition '{}'", over.name), over.span);
                }
                self.expr(order, None);
                t
            }
            ExprKind::If(c, t, f) => {
                self.cond(c, "if condition");
                let a = self.expr(t, expected);
                let b = self.expr(f, expected.or(Some(&a)));
                if !a.accepts(&b) {
                    self.err(codes::E201, format!("if branches differ: {a} vs {b}"), e.span);
                }
                if a == Ty::Null { b } else { a }
            }
            ExprKind::List(items) => {
                let elem = match expected {
                    Some(Ty::Coll(i)) => Some((**i).clone()),
                    _ => None,
                };
                let mut first = elem.clone();
                for it in items {
                    let t = self.expr(it, first.as_ref());
                    if first.is_none() {
                        first = Some(t);
                    }
                }
                Ty::Coll(Box::new(first.unwrap_or(Ty::Unknown)))
            }
            ExprKind::Binder(se, body) => {
                self.push(None);
                self.set_expr(se);
                let t = self.expr(body, None);
                self.pop();
                t
            }
        }
    }

    fn name(&mut self, id: &Ident, expected: Option<&Ty>) -> Ty {
        if let Some(t) = self.lookup(&id.name) {
            return t;
        }
        if let Some(t) = self.row_field(&id.name) {
            return t;
        }
        if is_upper_name(&id.name) {
            if let Some(en) = self.m.enum_of_value(&id.name, expected) {
                return Ty::Enum(en);
            }
            if let Some(Ty::Enum(en)) = expected {
                let values = self.m.enums.get(en).map(|d| d.values.iter().map(|v| v.name.clone()).collect::<Vec<_>>().join(", "));
                self.err(codes::E202, format!("'{}' is not a value of {en}", id.name), id.span).help = values.map(|v| format!("values: {v}"));
                return Ty::Unknown;
            }
            if self.m.enums.values().filter(|d| d.values.iter().any(|v| v.name == id.name)).count() > 1 {
                self.err(codes::E202, format!("'{}' belongs to several enums; compare it with a typed value", id.name), id.span);
                return Ty::Unknown;
            }
        }
        if self.m.entities.contains_key(&id.name) {
            self.err(codes::E201, format!("'{}' is an entity type, not a value", id.name), id.span).help =
                Some(format!("to talk about rows use a set expression, e.g. 'exists {} x where ...'", id.name));
            return Ty::Unknown;
        }
        let in_scope: Vec<String> = self.scopes.iter().flat_map(|s| s.vars.keys().cloned()).collect();
        self.err(codes::E104, format!("unknown name '{}'", id.name), id.span).help =
            Some(if in_scope.is_empty() { "nothing is in scope here".to_string() } else { format!("in scope: {}", in_scope.join(", ")) });
        Ty::Unknown
    }

    fn field_access(&mut self, base: &Ty, f: &Ident) -> Ty {
        match base {
            Ty::Unknown | Ty::Json | Ty::Ext(_) => {
                if matches!(base, Ty::Json) {
                    Ty::Json
                } else {
                    Ty::Unknown
                }
            }
            Ty::Coll(inner) => {
                let t = self.field_access(inner, f);
                if t.is_unknown() {
                    t
                } else if let Ty::Coll(_) = t {
                    t
                } else {
                    Ty::Coll(Box::new(t))
                }
            }
            other => match self.field_ty(other, &f.name) {
                Some(t) => t,
                None => {
                    let fields = match other.entity().and_then(|e| self.m.entity(e)) {
                        Some(ent) => format!("fields: {}", ent.fields.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ")),
                        None => String::new(),
                    };
                    let d = self.err(codes::E103, format!("{other} has no field '{}'", f.name), f.span);
                    if !fields.is_empty() {
                        d.help = Some(fields);
                    }
                    Ty::Unknown
                }
            },
        }
    }

    fn binary(&mut self, op: BinOp, l: &Expr, r: &Expr, span: Span) -> Ty {
        use BinOp::*;
        match op {
            And | Or => {
                self.cond(l, "left side of logical operator");
                self.cond(r, "right side of logical operator");
                Ty::Bool
            }
            Eq | Ne | Lt | Le | Gt | Ge => {
                // resolve bare enum values against the other side
                let (tl, tr) = if matches!(&l.kind, ExprKind::Name(n) if is_upper_name(&n.name) && self.lookup(&n.name).is_none()) {
                    let tr = self.expr(r, None);
                    (self.expr(l, Some(&tr)), tr)
                } else {
                    let tl = self.expr(l, None);
                    (tl.clone(), self.expr(r, Some(&tl)))
                };
                if !tl.accepts(&tr) {
                    self.err(codes::E201, format!("cannot compare {tl} with {tr}"), span);
                }
                if matches!(op, Lt | Le | Gt | Ge) {
                    let ordered = |t: &Ty| match t {
                        Ty::Enum(e) => self.m.enums.get(e).is_some_and(|d| d.ordered),
                        t => t.is_numeric() || t.is_temporal() || *t == Ty::Duration || t.is_textual() || t.is_unknown(),
                    };
                    if !ordered(&tl) {
                        self.err(codes::E201, format!("{tl} has no order; only numbers, times, texts and 'ordered' enums compare with < >"), span);
                    }
                }
                Ty::Bool
            }
            Add | Sub => {
                let tl = self.expr(l, None);
                let tr = self.expr(r, None);
                match (&tl, &tr) {
                    (a, Ty::Duration) if a.is_temporal() => tl.clone(),
                    (Ty::Duration, Ty::Duration) => Ty::Duration,
                    (a, b) if a.is_temporal() && b.is_temporal() && op == Sub => Ty::Duration,
                    (a, b) if a.is_numeric() && b.is_numeric() => {
                        if matches!(a, Ty::Money(_)) {
                            a.clone()
                        } else if matches!(b, Ty::Money(_)) {
                            b.clone()
                        } else if *a == Ty::Decimal || *b == Ty::Decimal {
                            Ty::Decimal
                        } else {
                            Ty::Int
                        }
                    }
                    (a, b) if a.is_unknown() || b.is_unknown() => Ty::Unknown,
                    (a, b) => {
                        self.err(codes::E201, format!("cannot apply '{}' to {a} and {b}", if op == Add { "+" } else { "-" }), span);
                        Ty::Unknown
                    }
                }
            }
            Mul | Div => {
                let tl = self.expr(l, None);
                let tr = self.expr(r, None);
                if !(tl.is_numeric() || tl.is_unknown()) || !(tr.is_numeric() || tr.is_unknown()) {
                    self.err(codes::E201, format!("cannot multiply/divide {tl} and {tr}"), span);
                    return Ty::Unknown;
                }
                if matches!(tl, Ty::Money(_)) {
                    tl
                } else if matches!(tr, Ty::Money(_)) {
                    tr
                } else if tl == Ty::Decimal || tr == Ty::Decimal || op == Div {
                    Ty::Decimal
                } else {
                    Ty::Int
                }
            }
        }
    }

    fn call(&mut self, c: &CallExpr, expected: Option<&Ty>) -> Ty {
        let name = c.callee.text();
        if c.callee.parts.len() > 1 {
            let ns = c.callee.parts[0].name.clone();
            if self.m.searches.contains_key(&ns) {
                for a in &c.args {
                    self.expr(&a.value, None);
                }
                return Ty::Unknown;
            }
            if !self.m.uses.contains(&ns) {
                self.err(codes::E501, format!("'{name}' belongs to extension '{ns}', which is not declared"), c.span).help =
                    Some(format!("add 'use {ns}'"));
            }
            for a in &c.args {
                self.expr(&a.value, None);
            }
            return Ty::Ext(name);
        }
        if let Some(rel) = self.m.relations.get(&name).copied() {
            self.check_args(&name, &rel.params, &c.args, c.span);
            return match &rel.result {
                Some(r) => Ty::Entity(r.name.clone()),
                None => Ty::Bool,
            };
        }
        if let Some(f) = self.m.fns.get(&name).copied() {
            self.check_args(&name, &f.params, &c.args, c.span);
            return self.shallow_type(&f.ret);
        }
        let arg_tys: Vec<Ty> = c.args.iter().map(|a| self.expr(&a.value, None)).collect();
        let arity = |this: &mut Self, n: usize| {
            if arg_tys.len() != n {
                this.err(codes::E211, format!("{name} takes {n} argument(s), got {}", arg_tys.len()), c.span);
            }
        };
        match name.as_str() {
            "same" => {
                arity(self, 1);
                if !matches!(c.args.first().map(|a| &a.value.kind), Some(ExprKind::Binder(..))) {
                    self.err(codes::E212, "same() takes a binder: same(items x: x.field)", c.span);
                }
                arg_tys.first().cloned().unwrap_or(Ty::Unknown)
            }
            "domain" => {
                arity(self, 1);
                Ty::Text
            }
            "days_until" => {
                arity(self, 1);
                Ty::Int
            }
            "contains" | "starts_with" => {
                arity(self, 2);
                Ty::Bool
            }
            "lower" | "upper" | "trim" | "slugify" => {
                arity(self, 1);
                Ty::Text
            }
            "length" => {
                arity(self, 1);
                Ty::Int
            }
            "coalesce" => arg_tys.first().cloned().unwrap_or(Ty::Unknown),
            "snapshot" => {
                arity(self, 1);
                match arg_tys.first() {
                    Some(Ty::Entity(e)) => Ty::Snapshot(e.clone()),
                    Some(Ty::Unknown) | None => Ty::Unknown,
                    Some(other) => {
                        self.err(codes::E201, format!("snapshot() takes an entity row, got {other}"), c.span);
                        Ty::Unknown
                    }
                }
            }
            "flag" => {
                arity(self, 1);
                if let Some(Arg { value: Expr { kind: ExprKind::Name(n), .. }, .. }) = c.args.first()
                    && !self.m.flags.contains(&n.name)
                {
                    self.err(codes::E104, format!("unknown flag '{}'", n.name), n.span);
                }
                Ty::Bool
            }
            _ => {
                let _ = expected;
                self.err(codes::E104, format!("unknown function or relation '{name}'"), c.callee.span).help =
                    Some("declare it with 'relation' or 'fn', or use an extension call like ns.fn(...)".into());
                Ty::Unknown
            }
        }
    }

    fn check_args(&mut self, name: &str, params: &[Param], args: &[Arg], span: Span) {
        if params.len() != args.len() {
            self.err(codes::E211, format!("{name} takes {} argument(s), got {}", params.len(), args.len()), span);
        }
        for (p, a) in params.iter().zip(args) {
            let want = self.shallow_type(&p.ty);
            let got = self.expr(&a.value, Some(&want));
            if matches!(got, Ty::Coll(_)) && !matches!(want, Ty::Coll(_)) {
                self.err(codes::E213, format!("argument '{}' of {name} is multi-valued ({got})", p.name.name), a.value.span).help =
                    Some("bind one value first: let x = same(items i: i.field) else CODE".into());
            } else if !want.accepts(&got) {
                self.err(codes::E201, format!("argument '{}' of {name} must be {want}, got {got}", p.name.name), a.value.span);
            }
        }
    }

    /// Checks a set expression, binds its alias in the current scope and
    /// returns the element type.
    fn set_expr(&mut self, se: &SetExpr) -> Ty {
        let elem = match &se.source.kind {
            ExprKind::Name(id) if self.m.entities.contains_key(&id.name) && self.lookup(&id.name).is_none() => Ty::Entity(id.name.clone()),
            _ => match self.expr(&se.source, None) {
                Ty::Coll(i) => *i,
                t @ (Ty::Entity(_) | Ty::Union(_) | Ty::Snapshot(_) | Ty::Unknown) => t,
                Ty::Record(r) => Ty::Record(r),
                other => {
                    self.err(codes::E201, format!("expected a set of rows, got {other}"), se.source.span);
                    Ty::Unknown
                }
            },
        };
        match &se.alias {
            Some(a) => self.bind(a, elem.clone()),
            None => {
                if let Some(s) = self.scopes.last_mut() {
                    s.row = Some(elem.clone());
                }
            }
        }
        if let Some(f) = &se.filter {
            self.cond(f, "'where' condition");
        }
        elem
    }

    // ---------- statements ----------

    fn block(&mut self, b: &Block) {
        self.push(None);
        self.stmts(&b.stmts);
        self.pop();
    }

    fn stmts(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn entity_of(&mut self, id: &Ident) -> Option<String> {
        if self.m.entities.contains_key(&id.name) {
            Some(id.name.clone())
        } else {
            self.err(codes::E102, format!("unknown entity '{}'", id.name), id.span);
            None
        }
    }

    fn field_assigns(&mut self, entity: &str, fields: &[FieldAssign]) {
        let Some(ent) = self.m.entity(entity) else { return };
        let mut given: Vec<String> = Vec::new();
        for fa in fields {
            match fa {
                FieldAssign::Named { name, value } => {
                    let Some(fi) = ent.field(&name.name) else {
                        self.err(codes::E103, format!("{entity} has no field '{}'", name.name), name.span).help = Some(format!(
                            "fields: {}",
                            ent.fields.iter().filter(|f| f.writable()).map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ")
                        ));
                        continue;
                    };
                    if !fi.writable() {
                        self.err(codes::E203, format!("{entity}.{} cannot be assigned", name.name), name.span).help =
                            Some("implicit, inverse and counter fields are maintained by the runtime".into());
                        continue;
                    }
                    let want = fi.ty.clone();
                    let got = match value {
                        Some(v) => self.expr(v, Some(&want)),
                        None => self.name(name, Some(&want)),
                    };
                    if !want.accepts(&got) {
                        self.err(
                            codes::E201,
                            format!("{entity}.{} is {want}, got {got}", name.name),
                            value.as_ref().map(|v| v.span).unwrap_or(name.span),
                        );
                    }
                    if given.contains(&name.name) {
                        self.err(codes::E101, format!("'{}' assigned twice", name.name), name.span);
                    }
                    given.push(name.name.clone());
                }
                FieldAssign::Spread(v) => {
                    let t = self.expr(v, None);
                    match &t {
                        Ty::Record(r) => {
                            let rec = self.m.records.get(r).copied();
                            if let Some(rec) = rec {
                                for rf in &rec.fields {
                                    match ent.field(&rf.name.name) {
                                        Some(fi) if fi.writable() => {
                                            let rt = self.shallow_type(&rf.ty);
                                            if !fi.ty.accepts(&rt) {
                                                self.err(
                                                    codes::E201,
                                                    format!("spread field '{}' is {rt}, but {entity}.{} is {}", rf.name.name, rf.name.name, fi.ty),
                                                    v.span,
                                                );
                                            }
                                            given.push(rf.name.name.clone());
                                        }
                                        _ => {
                                            self.err(
                                                codes::E103,
                                                format!("spread record {r} has field '{}' that {entity} cannot take", rf.name.name),
                                                v.span,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                        Ty::Unknown => {}
                        other => {
                            self.err(codes::E201, format!("only records can be spread, got {other}"), v.span);
                        }
                    }
                }
            }
        }
    }

    /// `set x.f = v` / `update ... set f = v`: returns (entity, field) if the target is writable.
    fn assign(&mut self, a: &Assign, row: Option<&Ty>) {
        let (owner, field) = match &a.target.kind {
            ExprKind::Field(base, f) => (self.expr(base, None), f.clone()),
            ExprKind::Name(f) if row.is_some() => (row.cloned().unwrap_or(Ty::Unknown), f.clone()),
            _ => {
                self.err(codes::E209, "assignment target must be a field, e.g. 'set order.status = PAID'", a.target.span);
                return;
            }
        };
        let owner = match owner {
            Ty::Coll(i) => *i,
            o => o,
        };
        let Some(ent_name) = owner.entity().map(|s| s.to_string()) else {
            if !owner.is_unknown() {
                self.err(codes::E209, format!("cannot assign a field of {owner}"), a.target.span);
            }
            return;
        };
        let Some(fi) = self.m.entity(&ent_name).and_then(|e| e.field(&field.name)).cloned() else {
            self.err(codes::E103, format!("{ent_name} has no field '{}'", field.name), field.span);
            return;
        };
        if !fi.writable() {
            self.err(codes::E203, format!("{ent_name}.{} cannot be assigned", field.name), field.span);
            return;
        }
        let got = self.expr(&a.value, Some(&fi.ty));
        if a.op != AssignOp::Set && !(fi.ty.is_numeric() || fi.ty.is_unknown()) {
            self.err(codes::E204, format!("'+='/'-=' need a numeric field; {ent_name}.{} is {}", field.name, fi.ty), a.span);
        }
        if matches!(got, Ty::Coll(_)) && !matches!(fi.ty, Ty::Coll(_)) {
            self.err(codes::E213, format!("value for {ent_name}.{} is multi-valued; aggregate it", field.name), a.value.span);
        } else if !fi.ty.accepts(&got) {
            self.err(codes::E201, format!("{ent_name}.{} is {}, got {got}", field.name, fi.ty), a.value.span);
        }
        if got == Ty::Null && !fi.optional {
            self.err(codes::E201, format!("{ent_name}.{} is not optional and cannot be set to null", field.name), a.value.span);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let(l) => {
                let t = self.expr(&l.value, None);
                self.bind(&l.name, t);
            }
            Stmt::Insert { entity, from, fields, bind, .. } => {
                let Some(ent) = self.entity_of(entity) else { return };
                self.push(None);
                if let Some(se) = from {
                    self.set_expr(se);
                }
                self.field_assigns(&ent, fields);
                self.pop();
                if let Some(b) = bind {
                    if from.is_some() {
                        self.err(codes::E212, "'insert ... from' creates several rows and cannot be bound with 'as'", b.span);
                    }
                    self.bind(b, Ty::Entity(ent));
                }
            }
            Stmt::Upsert { entity, fields, bind, .. } => {
                let Some(ent) = self.entity_of(entity) else { return };
                self.field_assigns(&ent, fields);
                if let Some(b) = bind {
                    self.bind(b, Ty::Entity(ent));
                }
            }
            Stmt::Update { target, via, assigns, .. } => {
                self.push(None);
                let row = self.set_expr(target);
                if let Some((path, alias)) = via {
                    let t = self.expr(path, None);
                    let el = match t {
                        Ty::Coll(i) => *i,
                        o => o,
                    };
                    self.bind(alias, el);
                }
                for a in assigns {
                    self.assign(a, Some(&row));
                }
                self.pop();
            }
            Stmt::Delete { target, .. } | Stmt::Purge { target, .. } => {
                self.push(None);
                let t = self.set_expr(target);
                if !(matches!(t, Ty::Entity(_) | Ty::Union(_)) || t.is_unknown()) {
                    self.err(codes::E201, format!("only entity rows can be deleted, got {t}"), target.span);
                }
                self.pop();
            }
            Stmt::Erase { target, .. } => {
                self.expr(target, None);
            }
            Stmt::Toggle { entity, fields, .. } => {
                let Some(ent) = self.entity_of(entity) else { return };
                self.field_assigns(&ent, fields);
            }
            Stmt::Set { assigns, .. } => {
                for a in assigns {
                    self.assign(a, None);
                }
            }
            Stmt::When { cond, body, .. } => {
                let t = self.expr(cond, Some(&Ty::Bool));
                if matches!(t, Ty::Coll(_)) {
                    self.err(codes::E213, "'when' condition is multi-valued", cond.span);
                }
                self.block(body);
            }
            Stmt::Each { source, body, .. } => {
                self.push(None);
                let t = self.expr(&source.source, None);
                if !matches!(t, Ty::Coll(_) | Ty::Unknown) {
                    self.err(codes::E206, format!("'each' needs a bounded collection input, got {t}"), source.span);
                }
                let el = match t {
                    Ty::Coll(i) => *i,
                    o => o,
                };
                match &source.alias {
                    Some(a) => self.bind(a, el),
                    None => {
                        self.err(codes::E206, "'each' needs an item name: each items x partial { ... }", source.span);
                    }
                }
                self.block(body);
                self.pop();
            }
            Stmt::Effect { call, bind, into, on_failure, .. } => {
                if call.callee.parts.len() < 2 {
                    self.err(
                        codes::E502,
                        format!("'{}' is not a statement; extension effects are written ns.effect(...)", call.callee.text()),
                        call.span,
                    );
                }
                let t = self.call(call, None);
                if let Some(target) = into {
                    let tt = self.expr(target, None);
                    let ok = match (&call.callee.text()[..], &tt) {
                        ("s3.put", Ty::Object) | (_, Ty::Unknown) | (_, Ty::Ext(_)) => true,
                        ("s3.put", _) => false,
                        _ => true,
                    };
                    if !ok {
                        self.err(codes::E201, format!("'into' target must be an s3.Object field, got {tt}"), target.span);
                    }
                    if !matches!(&target.kind, ExprKind::Field(..)) {
                        self.err(codes::E209, "'into' target must be a field of a row", target.span);
                    }
                }
                if let Some(b) = bind {
                    self.bind(b, t);
                }
                if let Some(blk) = on_failure {
                    self.block(blk);
                }
            }
            Stmt::Reserve { of, .. } => {
                self.expr(of, None);
                if !self.m.uses.contains("inventory") {
                    self.err(codes::E501, "reserve/confirm/release come from extension 'inventory'", s_span(s)).help =
                        Some("add 'use inventory'".into());
                }
            }
            Stmt::AtRun { at, intent, args, .. } => {
                self.expect_ty(at, &Ty::Time, "'at' time");
                if !self.m.intents.contains_key(&intent.name) {
                    self.err(codes::E104, format!("unknown intent '{}'", intent.name), intent.span);
                }
                for a in args {
                    self.expr(&a.value, None);
                }
            }
            Stmt::Notify(n) => self.notify(n),
            Stmt::ExportPersonalData { of, notify, .. } => {
                let t = self.expr(of, None);
                if t.entity() != self.m.actor_entity() && !t.is_unknown() {
                    self.err(codes::E201, "personal data export is for the actor entity", of.span);
                }
                if let Some(n) = notify {
                    self.expr(n, None);
                }
            }
        }
    }

    fn notify(&mut self, n: &Notify) {
        self.push(None);
        self.set_expr(&n.to);
        let ns = n.via.name.parts[0].name.clone();
        if !self.m.uses.contains(&ns) {
            self.err(codes::E501, format!("'{}' belongs to extension '{ns}', which is not declared", n.via.name.text()), n.via.name.span);
        }
        for fa in &n.fields {
            match fa {
                FieldAssign::Named { value: Some(v), .. } | FieldAssign::Spread(v) => {
                    self.expr(v, None);
                }
                FieldAssign::Named { name, value: None } => {
                    self.name(name, None);
                }
            }
        }
        self.pop();
    }

    // ---------- intents ----------

    /// A missing `allow` is a semantic rule (aip-ir `analyze`); the clause itself is only typed here.
    fn allow(&mut self, a: Option<&Allow>) {
        if let Some(a) = a {
            self.cond(&a.cond, "'allow' condition");
        }
    }

    fn emit(&mut self, e: &Emit) {
        let mut fields = Vec::new();
        for fa in &e.fields {
            match fa {
                FieldAssign::Named { name, value } => {
                    let t = match value {
                        Some(v) => self.expr(v, None),
                        None => self.name(name, None),
                    };
                    let t = match t {
                        Ty::Snapshot(x) => Ty::Entity(x),
                        o => o,
                    };
                    fields.push((name.name.clone(), t));
                }
                FieldAssign::Spread(v) => {
                    self.err(codes::E212, "events do not take spreads; name each field", v.span);
                }
            }
        }
        if let Some(t) = &e.to {
            if !self.m.uses.contains(&t.broker.name) {
                self.err(codes::E501, format!("broker '{}' is not a declared extension", t.broker.name), t.broker.span);
            }
            if let Some(k) = &t.key {
                self.expr(k, None);
            }
        }
        // the first shape is the event's shape; later emissions that differ are reported on the IR (aip-ir `analyze`)
        self.events.entry(e.event.name.clone()).or_insert(EventShape { fields, declared: false, span: e.span });
    }

    fn selection(&mut self, row: &Ty, sel: &Selection) {
        let mut seen: Vec<&str> = Vec::new();
        for it in &sel.items {
            if seen.contains(&it.name.name.as_str()) {
                self.err(codes::E101, format!("'{}' selected twice", it.name.name), it.name.span);
            }
            seen.push(&it.name.name);
            let t = match &it.value {
                Some(v) => self.expr(v, None),
                None => match self.field_ty(row, &it.name.name) {
                    Some(t) => t,
                    None => {
                        if !row.is_unknown() {
                            self.err(codes::E103, format!("{row} has no field '{}'", it.name.name), it.name.span);
                        }
                        Ty::Unknown
                    }
                },
            };
            let elem = match &t {
                Ty::Coll(i) => (**i).clone(),
                o => o.clone(),
            };
            let is_rel = matches!(elem, Ty::Entity(_) | Ty::Snapshot(_) | Ty::Record(_));
            match (&it.sub, is_rel) {
                (Some(sub), true) => {
                    self.push(None);
                    self.selection(&elem, sub);
                    self.pop();
                }
                (Some(_), false) if !elem.is_unknown() => {
                    self.err(codes::E220, format!("'{}' is {t}, not a relation; it cannot have a sub-selection", it.name.name), it.name.span);
                }
                (None, true) if it.value.is_none() && matches!(elem, Ty::Entity(_)) && matches!(t, Ty::Coll(_)) => {
                    self.err(codes::E220, format!("relation '{}' needs a sub-selection", it.name.name), it.name.span).help =
                        Some(format!("write '{} {{ id ... }}'", it.name.name));
                }
                _ => {}
            }
        }
    }

    fn query(&mut self, q: &QueryDecl) {
        self.ctx = Ctx::Query;
        self.search_aliases.clear();
        self.push(None);
        self.param_types(&q.params);
        for l in &q.lets {
            let t = self.expr(&l.value, None);
            self.bind(&l.name, t);
        }
        for (call, name) in &q.fetches {
            let t = self.call(call, None);
            self.bind(name, t);
        }
        let row = match &q.from {
            Some(FromClause::Entity { entity, alias }) => match self.entity_of(entity) {
                Some(e) => {
                    self.bind(alias, Ty::Entity(e.clone()));
                    Ty::Entity(e)
                }
                None => Ty::Unknown,
            },
            Some(FromClause::Param(p)) => match self.lookup(&p.name) {
                Some(t @ Ty::Entity(_)) => t,
                Some(other) => {
                    self.err(codes::E201, format!("'from {}' needs an entity parameter, got {other}", p.name), p.span);
                    Ty::Unknown
                }
                None => {
                    self.err(codes::E104, format!("'from {}': no such parameter; write 'from Entity alias'", p.name), p.span);
                    Ty::Unknown
                }
            },
            Some(FromClause::Call { call, alias }) => {
                let search = call.callee.parts.first().and_then(|p| self.m.searches.get(&p.name).copied());
                let el = match search {
                    Some(sd) => Ty::Entity(sd.entity.name.clone()),
                    None => {
                        self.err(codes::E104, format!("'{}' is not a search declaration", call.callee.text()), call.span);
                        Ty::Unknown
                    }
                };
                if search.is_some() {
                    if call.callee.parts.len() != 2 || call.callee.parts[1].name != "match" {
                        self.err(codes::E104, format!("a search is queried with '{}.match(text)'", call.callee.parts[0].name), call.span);
                    } else if call.args.len() != 1 {
                        self.err(codes::E211, format!("'match' takes the text to search for, got {} arguments", call.args.len()), call.span);
                    }
                    self.search_aliases.push(alias.name.clone());
                }
                for a in &call.args {
                    let t = self.expr(&a.value, None);
                    if search.is_some() && !(t.is_textual() || t.is_unknown()) {
                        self.err(codes::E201, format!("the text to search for must be Text, got {t}"), a.value.span);
                    }
                }
                self.bind(alias, el.clone());
                el
            }
            None => Ty::Unknown,
        };
        // `allow` may reference the row of a `from param` query
        self.allow(q.allow.as_ref());
        if let Some(f) = &q.filter {
            self.cond(f, "'where' condition");
        }
        for g in &q.group_by {
            self.expr(g, None);
        }
        match &q.sort {
            Some(Sort::Keys(keys)) => {
                for k in keys {
                    self.expr(&k.expr, None);
                }
            }
            Some(Sort::ByParam { param, cases }) => {
                let pt = self.lookup(&param.name);
                match pt {
                    Some(Ty::Enum(en)) => {
                        let values: Vec<String> =
                            self.m.enums.get(&en).map(|d| d.values.iter().map(|v| v.name.clone()).collect()).unwrap_or_default();
                        for (k, keys) in cases {
                            if !values.contains(&k.name) {
                                self.err(codes::E202, format!("'{}' is not a value of {en}", k.name), k.span);
                            }
                            for key in keys {
                                self.expr(&key.expr, None);
                            }
                        }
                    }
                    _ => {
                        self.err(codes::E201, format!("'sort by {} of {{...}}' needs an enum parameter", param.name), param.span);
                    }
                }
            }
            None => {}
        }
        let sel_row = if q.group_by.is_empty() { row } else { Ty::Unknown };
        self.push(Some(sel_row.clone()));
        self.selection(&sel_row, &q.select);
        self.pop();
        for t in &q.touches {
            if let ExprKind::Field(base, _) = &t.kind {
                self.expr(base, None);
            }
        }
        self.pop();
        self.search_aliases.clear();
    }

    fn command(&mut self, c: &CommandDecl) {
        self.ctx = Ctx::Command;
        self.push(None);
        self.param_types(&c.params);
        if let Some(Some(k)) = &c.idempotent {
            self.expr(k, None);
        }
        for l in &c.lets {
            let t = self.expr(&l.value, None);
            if matches!(t, Ty::Coll(_)) {
                self.err(codes::E213, format!("let {} is multi-valued; use same(...), the ... or an aggregate", l.name.name), l.value.span);
            }
            self.bind(&l.name, t);
        }
        self.allow(c.allow.as_ref());
        for r in &c.requires {
            if let Some(w) = &r.when {
                self.cond(w, "'require when' condition");
            }
            self.cond(&r.cond, "'require' condition");
        }
        if let Some(b) = &c.body {
            // bindings made inside `do` (insert ... as x) are visible to emit/returns
            self.stmts(&b.stmts);
        }
        for e in &c.emits {
            self.emit(e);
        }
        if let Some((r, sel)) = &c.returns {
            let t = self.expr(r, None);
            if let Some(sel) = sel {
                self.selection(&t, sel);
            }
        }
        self.pop();
    }

    // ---------- evolution declarations ----------

    /// `was` and `removed` say how a deployed database moves to this program; they must not contradict the program itself.
    fn evolution(&mut self, file: &File) {
        let declared: Vec<&str> = file
            .decls
            .iter()
            .filter_map(|d| match d {
                Decl::Entity(e) => Some(e.name.name.as_str()),
                _ => None,
            })
            .collect();
        for d in &file.decls {
            match d {
                Decl::Removed(r) if declared.contains(&r.name.name.as_str()) => {
                    self.err(codes::E111, format!("'removed entity {}' but {} is still declared", r.name.name, r.name.name), r.name.span);
                }
                Decl::Record(r) => {
                    for f in &r.fields {
                        for m in &f.mods {
                            if let FieldMod::Was(w) = m {
                                self.err(
                                    codes::E111,
                                    format!("'was' only applies to entity fields; {} is a field of record {}", f.name.name, r.name.name),
                                    w.span,
                                );
                            }
                        }
                    }
                }
                Decl::Entity(e) => {
                    if let Some(w) = &e.was
                        && (w.name == e.name.name || declared.contains(&w.name.as_str()))
                    {
                        self.err(codes::E111, format!("{} is 'was {}', but an entity of that name is still declared", e.name.name, w.name), w.span);
                    }
                    let Some(info) = self.m.entity(&e.name.name).cloned() else { continue };
                    let removed: Vec<&Ident> =
                        e.members.iter().filter_map(|m| if let EntityMember::RemovedField(x) = m { Some(x) } else { None }).collect();
                    for x in &removed {
                        if info.field(&x.name).is_some() {
                            self.err(codes::E111, format!("'removed field {}' but {}.{} is still declared", x.name, e.name.name, x.name), x.span);
                        }
                    }
                    let mut seen: Vec<&str> = Vec::new();
                    for m in &e.members {
                        let EntityMember::Field(f) = m else { continue };
                        for fm in &f.mods {
                            let FieldMod::Was(w) = fm else { continue };
                            let why = if w.name == f.name.name {
                                Some(format!("{}.{} is 'was' its own name", e.name.name, f.name.name))
                            } else if info.field(&w.name).is_some() {
                                Some(format!("{}.{} is 'was {}', but {}.{} is still declared", e.name.name, f.name.name, w.name, e.name.name, w.name))
                            } else if removed.iter().any(|x| x.name == w.name) {
                                Some(format!("{}.{} is 'was {}', and {} is also 'removed field'", e.name.name, f.name.name, w.name, w.name))
                            } else if seen.contains(&w.name.as_str()) {
                                Some(format!("two fields of {} are 'was {}'", e.name.name, w.name))
                            } else {
                                None
                            };
                            seen.push(&w.name);
                            if let Some(msg) = why {
                                self.err(codes::E111, msg, w.span);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // ---------- entities ----------

    fn entity(&mut self, e: &EntityDecl) {
        let ent_ty = Ty::Entity(e.name.name.clone());
        self.ctx = Ctx::Entity;
        let info = self.m.entity(&e.name.name).cloned();
        let Some(info) = info else { return };
        for f in &info.fields {
            let Some(decl) = f.decl else { continue };
            // Core IR holds one generator per field; a second declaration would be dropped silently
            let mut generators = decl.mods.iter().filter_map(|m| match m {
                FieldMod::Tree { span, .. } | FieldMod::Sequence { span, .. } | FieldMod::Slug { span, .. } | FieldMod::Position { span, .. } => {
                    Some(*span)
                }
                _ => None,
            });
            if let Some(extra) = generators.nth(1) {
                self.err(codes::E222, format!("a field can have at most one generator ({}.{})", e.name.name, f.name), extra);
            }
            for m in &decl.mods {
                match m {
                    FieldMod::OnDelete(p) | FieldMod::OnErase(p) => {
                        if !matches!(f.kind, FieldKind::Ref(_) | FieldKind::UnionRef(_)) {
                            self.err(codes::E201, format!("'on delete/erase' only applies to references; {} is {}", f.name, f.ty), decl.name.span);
                        }
                        if let RefPolicy::Reassign(x) = p {
                            self.push(Some(ent_ty.clone()));
                            self.expr(x, Some(&f.ty));
                            self.pop();
                        }
                    }
                    FieldMod::VisibleTo(x) => {
                        self.push(Some(ent_ty.clone()));
                        self.cond(x, "field visibility");
                        self.pop();
                    }
                    FieldMod::Masked { unless, with } => {
                        self.push(Some(ent_ty.clone()));
                        self.cond(unless, "mask condition");
                        self.pop();
                        let ns = &with.name.parts[0].name;
                        if with.name.parts.len() > 1 && !self.m.uses.contains(ns) && ns != "mask" {
                            self.err(codes::E501, format!("mask function '{}' comes from an undeclared extension", with.name.text()), with.name.span);
                        }
                    }
                    FieldMod::Via(v) => {
                        if let FieldKind::Inverse { target, .. } = &f.kind {
                            // resolving `via` to a field is name resolution; whether that field points back is a semantic rule (aip-ir `analyze`)
                            if self.m.entity(target).and_then(|t| t.field(&v.name)).is_none() {
                                self.err(
                                    codes::E106,
                                    format!(
                                        "{}.{} is 'via {}', but {target}.{} is not a reference to {}",
                                        e.name.name, f.name, v.name, v.name, e.name.name
                                    ),
                                    v.span,
                                )
                                .help = Some(format!("declare '{}: {}' on {target}", v.name, e.name.name));
                            }
                        } else {
                            self.err(codes::E106, "'via' is only valid on inverse relation fields ('Items[] via parent')", v.span);
                        }
                    }
                    FieldMod::Counter { via, .. } if !self.m.uses.contains(&via.name) => {
                        self.err(codes::E501, format!("counter via '{}' needs 'use {}'", via.name, via.name), via.span);
                    }
                    _ => {}
                }
            }
            if let TypeKind::JsonValidated(path) = &decl.ty.kind
                && info.field(&path.parts[0].name).is_none()
            {
                // resolving the head of the path is name resolution; what kind of field it is, is a semantic rule (aip-ir `analyze`)
                self.err(codes::E312, format!("'validated by {}' must name a Snapshot field of {}", path.text(), e.name.name), path.span);
            }
            if let Some(d) = &decl.default {
                self.push(Some(ent_ty.clone()));
                let t = self.expr(d, Some(&f.ty));
                if !f.ty.accepts(&t) {
                    self.err(codes::E201, format!("default of {} must be {}, got {t}", f.name, f.ty), d.span);
                }
                self.pop();
            }
        }
        for m in &e.members {
            self.push(Some(ent_ty.clone()));
            match m {
                EntityMember::Constraint(c) => self.constraint(e, &info, c),
                EntityMember::Lifecycle(l) => match info.field(&l.field.name).map(|f| f.ty.clone()) {
                    Some(Ty::Enum(en)) => {
                        let values: Vec<String> =
                            self.m.enums.get(&en).map(|d| d.values.iter().map(|v| v.name.clone()).collect()).unwrap_or_default();
                        for t in &l.transitions {
                            for v in t.from.iter().chain(&t.to) {
                                if !values.contains(&v.name) {
                                    self.err(codes::E202, format!("'{}' is not a value of {en}", v.name), v.span);
                                }
                            }
                        }
                    }
                    Some(other) => {
                        self.err(codes::E201, format!("lifecycle needs an enum field; {} is {other}", l.field.name), l.field.span);
                    }
                    None => {
                        self.err(codes::E103, format!("{} has no field '{}'", e.name.name, l.field.name), l.field.span);
                    }
                },
                EntityMember::Visibility(v) => self.cond(&v.cond, "visibility condition"),
                EntityMember::Predicate(p) => self.cond(&p.body, "predicate"),
                EntityMember::Trait(t) => match t {
                    EntityTrait::Publishable { by: Some(by), .. } => self.cond(by, "publish condition"),
                    EntityTrait::Tenant { via, .. } => {
                        let mut e_ty = ent_ty.clone();
                        for part in &via.parts {
                            e_ty = self.field_access(&e_ty, part);
                        }
                    }
                    EntityTrait::DynamicSchema { from, .. } if info.field(&from.name).is_none() => {
                        self.err(codes::E103, format!("{} has no field '{}'", e.name.name, from.name), from.span);
                    }
                    _ => {}
                },
                EntityMember::Field(_) | EntityMember::RemovedField(_) => {}
            }
            self.pop();
        }
    }

    fn constraint(&mut self, e: &EntityDecl, info: &crate::model::EntityInfo<'_>, c: &Constraint) {
        match c {
            Constraint::Unique { cols, filter, .. } => {
                for col in cols {
                    if info.field(&col.name).is_none() {
                        self.err(codes::E103, format!("{} has no field '{}'", e.name.name, col.name), col.span);
                    }
                }
                if let Some(f) = filter {
                    self.cond(f, "unique filter");
                }
            }
            Constraint::Cardinality { filter, per, repair, .. } => {
                self.cond(filter, "cardinality filter");
                let per_ty = info.field(&per.name).map(|f| f.ty.clone());
                match &per_ty {
                    Some(_) => {}
                    None => {
                        self.err(codes::E103, format!("{} has no field '{}'", e.name.name, per.name), per.span);
                    }
                }
                if let Some(b) = repair {
                    self.push(None);
                    self.bind(per, per_ty.unwrap_or(Ty::Unknown));
                    if let Some(s) = self.scopes.iter_mut().rev().find(|s| s.row.is_some()) {
                        s.row = None;
                    }
                    self.block(b);
                    self.pop();
                }
            }
            Constraint::NoOverlap { range, per, .. } => {
                if info.field(&range.name).is_none() {
                    self.err(codes::E103, format!("{} has no field '{}'", e.name.name, range.name), range.span);
                }
                if info.field(&per.name).is_none() {
                    self.err(codes::E103, format!("{} has no field '{}'", e.name.name, per.name), per.span);
                }
            }
            Constraint::Capacity { count, limit, .. } => {
                self.push(None);
                self.set_expr(count);
                self.pop();
                self.expect_ty(limit, &Ty::Int, "capacity limit");
            }
            Constraint::Invariant { cond, .. } => self.cond(cond, "invariant"),
        }
    }

    // ---------- top level ----------

    pub fn run(&mut self) {
        let file = self.m.file;
        if let Some(a) = self.m.actor {
            let ns = &a.via.name.parts[0].name;
            if !self.m.uses.contains(ns) {
                self.err(codes::E501, format!("actor provider '{}' needs 'use {ns}'", a.via.name.text()), a.via.name.span);
            }
            if !self.m.entities.contains_key(&a.name.name) {
                self.err(codes::E102, format!("actor entity '{}' is not declared", a.name.name), a.name.span);
            }
            if let Some(su) = &a.superuser {
                self.push(Some(Ty::Entity(a.name.name.clone())));
                self.cond(su, "superuser condition");
                self.pop();
            }
        }
        for d in &file.decls {
            if let Decl::Entity(e) = d {
                self.entity(e);
            }
        }
        self.evolution(file);
        for d in &file.decls {
            if let Decl::Record(r) = d {
                self.ctx = Ctx::Record;
                self.push(Some(Ty::Record(r.name.name.clone())));
                for f in &r.fields {
                    if self.shallow_type(&f.ty).is_unknown() {
                        self.err(codes::E102, format!("unknown type for field '{}'", f.name.name), f.ty.span);
                    }
                }
                for c in &r.checks {
                    if let Some(w) = &c.when {
                        self.cond(w, "check condition");
                    }
                    self.cond(&c.cond, "check");
                }
                self.pop();
            }
        }
        for d in &file.decls {
            if let Decl::Relation(r) = d {
                self.ctx = Ctx::Relation;
                self.push(None);
                self.param_types(&r.params);
                let t = self.expr(&r.body, None);
                match &r.result {
                    Some(res) => {
                        if t.entity() != Some(res.name.as_str()) && !t.is_unknown() {
                            self.err(
                                codes::E201,
                                format!("relation {} is declared to return {}, but its body is {t}", r.name.name, res.name),
                                r.body.span,
                            );
                        }
                    }
                    None => {
                        if !Ty::Bool.accepts(&t) {
                            self.err(
                                codes::E201,
                                format!("relation {} must be a condition (Bool) or declare its result type, got {t}", r.name.name),
                                r.body.span,
                            );
                        }
                    }
                }
                self.pop();
            }
        }
        // commands first so emitted event shapes are known to handlers
        for d in &file.decls {
            if let Decl::Command(c) = d {
                self.command(c);
            }
        }
        for d in &file.decls {
            self.ctx = Ctx::Command;
            match d {
                Decl::Query(q) => self.query(q),
                Decl::OnEvent(o) => {
                    self.push(None);
                    if !self.events.contains_key(&o.event.name) {
                        self.err(codes::E104, format!("event '{}' is never emitted or declared", o.event.name), o.event.span);
                    }
                    self.bind(&o.binding, Ty::Record(format!("event:{}", o.event.name)));
                    if let Some(w) = &o.when {
                        self.cond(w, "'when' condition");
                    }
                    self.block(&o.body);
                    self.pop();
                }
                Decl::Schedule(s) => {
                    self.push(None);
                    if let Some(se) = &s.for_each {
                        self.set_expr(se);
                    }
                    for st in &s.body {
                        self.stmt(st);
                    }
                    self.pop();
                }
                Decl::Retain(r) => {
                    let Some(ent) = self.entity_of(&r.entity) else { continue };
                    let mut t = Ty::Entity(ent);
                    for p in &r.after.parts {
                        t = self.field_access(&t, p);
                    }
                    if !(t.is_temporal() || t.is_unknown()) {
                        self.err(codes::E201, format!("retain ... after needs a time, got {t}"), r.after.span);
                    }
                }
                Decl::Rule(r) => {
                    self.push(None);
                    if let Some(ent) = self.entity_of(&r.entity) {
                        self.bind(&r.alias, Ty::Entity(ent));
                    }
                    self.cond(&r.when, "rule condition");
                    self.block(&r.body);
                    self.pop();
                }
                Decl::Subscribe(s) => {
                    self.push(None);
                    self.param_types(&s.params);
                    self.allow(Some(&s.allow));
                    if let FromClause::Entity { entity, alias } = &s.from
                        && let Some(e) = self.entity_of(entity)
                    {
                        self.bind(alias, Ty::Entity(e.clone()));
                        if let Some(f) = &s.filter {
                            self.cond(f, "'where' condition");
                        }
                        self.selection(&Ty::Entity(e), &s.select);
                    }
                    self.pop();
                }
                Decl::Job(j) => {
                    self.push(None);
                    self.param_types(&j.params);
                    self.cond(&j.allow.cond, "'allow' condition");
                    if let Some(p) = &j.progress {
                        self.set_expr(p);
                    }
                    if let Some(b) = &j.body {
                        self.block(b);
                    }
                    self.pop();
                }
                Decl::Verification(v) => {
                    self.push(None);
                    let subj = self.entity_of(&v.subject);
                    let tt = self.shallow_type(&v.target);
                    self.bind_str("target", tt.clone());
                    if let Some(w) = &v.target_where {
                        self.cond(w, "verification target condition");
                    }
                    let ns = &v.deliver.name.parts[0].name;
                    if !self.m.uses.contains(ns) {
                        self.err(codes::E501, format!("'{}' needs 'use {ns}'", v.deliver.name.text()), v.deliver.name.span);
                    }
                    let (a, b, blk) = &v.on_verified;
                    self.bind(a, subj.map(Ty::Entity).unwrap_or(Ty::Unknown));
                    self.bind(b, tt);
                    self.block(blk);
                    self.pop();
                }
                Decl::GrantLink(g) => {
                    self.push(None);
                    self.param_types(&g.scope);
                    self.param_types(&g.redeem_params);
                    self.cond(&g.issued_by, "'issued by' condition");
                    if let Some(t) = &g.to {
                        self.expr(t, self.actor_ty().as_ref());
                    }
                    if let Some(at) = self.actor_ty() {
                        self.bind_str("holder", at);
                    }
                    if let Some((rel, role)) = &g.grants {
                        let rt = self.expr(rel, None);
                        if rt.entity().is_none() && !rt.is_unknown() {
                            self.err(codes::E201, "'grants' must name a relation that returns a membership row", rel.span);
                        }
                        if self.m.enum_of_value(&role.name, None).is_none() {
                            self.err(codes::E202, format!("unknown role '{}'", role.name), role.span);
                        }
                    }
                    for r in &g.requires {
                        self.cond(&r.cond, "'require' condition");
                    }
                    if let Some(b) = &g.on_redeem {
                        self.block(b);
                    }
                    self.pop();
                }
                Decl::Approval(a) => {
                    self.push(None);
                    if let Some(e) = self.entity_of(&a.entity) {
                        self.bind(&a.alias, Ty::Entity(e));
                    }
                    self.push(None);
                    let elem = self.set_expr(&a.approvers);
                    self.pop();
                    let actor = self.actor_ty();
                    let links_actor = match (&elem, &actor) {
                        (Ty::Entity(e), Some(Ty::Entity(ae))) => {
                            e == ae
                                || self
                                    .m
                                    .entity(e)
                                    .is_some_and(|i| i.fields.iter().filter(|f| matches!(&f.kind, FieldKind::Ref(t) if t == ae)).count() == 1)
                        }
                        _ => elem.is_unknown(),
                    };
                    if !links_actor {
                        self.err(
                            codes::E201,
                            format!(
                                "approvers must be {} rows or rows with exactly one {} field",
                                actor.map(|t| t.to_string()).unwrap_or_else(|| "actor".into()),
                                "actor"
                            ),
                            a.approvers.span,
                        );
                    }
                    if a.approvers.alias.is_none() {
                        self.err(codes::E212, "approvers need an alias (e.g. 'ClubMember m where ...')", a.approvers.span);
                    }
                    if let Some(r) = &a.requested_by {
                        self.cond(r, "'requested by' condition");
                    }
                    self.block(&a.on_approved);
                    self.block(&a.on_rejected);
                    self.pop();
                }
                Decl::Expose(x) => {
                    let Some(ent) = self.entity_of(&x.entity) else { continue };
                    let ety = Ty::Entity(ent.clone());
                    self.push(Some(ety.clone()));
                    if let Some(Some(r)) = &x.read {
                        self.cond(r, "expose read condition");
                    }
                    for (op, spec) in [("create", &x.create), ("update", &x.update)] {
                        if let Some((cond, fields)) = spec {
                            self.cond(cond, "expose condition");
                            for f in fields {
                                match self.m.entity(&ent).and_then(|e| e.field(&f.name)) {
                                    Some(fi) if fi.writable() => {}
                                    Some(_) => {
                                        self.err(codes::E203, format!("{ent}.{} cannot be written through expose", f.name), f.span);
                                    }
                                    None => {
                                        self.err(codes::E103, format!("{ent} has no field '{}'", f.name), f.span);
                                    }
                                }
                            }
                            if op == "create" {
                                let missing: Vec<String> = self
                                    .m
                                    .entity(&ent)
                                    .map(|e| {
                                        e.fields
                                            .iter()
                                            .filter(|fi| fi.required() && !fields.iter().any(|f| f.name == fi.name))
                                            .map(|fi| fi.name.clone())
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                if !missing.is_empty() {
                                    self.err(
                                        codes::E205,
                                        format!("expose create for {ent} does not accept required field(s) {}", missing.join(", ")),
                                        x.span,
                                    );
                                }
                            }
                        }
                    }
                    if let Some(dc) = &x.delete {
                        self.cond(dc, "expose delete condition");
                    }
                    self.pop();
                }
                Decl::Webhook(w) => {
                    let via = w.via.name.text();
                    const SOURCES: &[&str] = &["http.webhook", "payments.stripe.webhook"];
                    if !SOURCES.contains(&via.as_str()) {
                        self.err(codes::E104, format!("unknown webhook source '{via}' (known: {})", SOURCES.join(", ")), w.via.name.span);
                    } else if !self.m.uses.contains(&w.via.name.parts[0].name) {
                        self.err(codes::E501, format!("'{via}' needs 'use {}'", w.via.name.parts[0].name), w.via.name.span);
                    }
                    // option shape and duplicate handlers are semantic rules (aip-ir `analyze`)
                    for h in &w.handlers {
                        self.push(None);
                        let ty = h.ty.as_ref().map(|t| self.shallow_type(t)).unwrap_or(Ty::Json);
                        if !matches!(ty, Ty::Record(_) | Ty::Json | Ty::Unknown) {
                            self.err(codes::E201, "a webhook payload is a record or Json", h.binding.span);
                        }
                        self.bind(&h.binding, ty);
                        self.block(&h.body);
                        self.pop();
                    }
                }
                Decl::Migration(mg) => self.block(&mg.body),
                Decl::OutboundWebhooks(o) => {
                    let Some(entity) = self.entity_of(&o.entity) else { continue };
                    for ev in &o.events {
                        if !self.events.contains_key(&ev.name) {
                            self.err(codes::E104, format!("event '{}' is never emitted or declared", ev.name), ev.span);
                        }
                    }
                    if let Some(f) = &o.filter {
                        // the condition sees the endpoint row and the event; every listed event has to have the fields it reads
                        let mut seen: Vec<(String, (u32, u32))> = Vec::new();
                        for ev in &o.events {
                            let before = self.diags.len();
                            self.push(None);
                            self.bind(&o.alias, Ty::Entity(entity.clone()));
                            self.bind_str("event", Ty::Record(format!("event:{}", ev.name)));
                            self.cond(f, "'where' condition");
                            self.pop();
                            // the same mistake in two events is one diagnostic
                            let mut keep = self.diags.split_off(before);
                            keep.retain(|d| {
                                let k = (d.message.clone(), (d.span.start, d.span.end));
                                if seen.contains(&k) {
                                    false
                                } else {
                                    seen.push(k);
                                    true
                                }
                            });
                            self.diags.extend(keep);
                        }
                    }
                }
                Decl::Search(sd) => {
                    if let Some(e) = self.entity_of(&sd.entity) {
                        for (f, _) in &sd.fields {
                            if self.field_ty(&Ty::Entity(e.clone()), &f.name).is_none() {
                                self.err(codes::E103, format!("{e} has no field '{}'", f.name), f.span);
                            }
                        }
                    }
                }
                Decl::Impersonate(i) => {
                    self.entity_of(&i.entity);
                    // the condition is about the caller: there is no row in scope
                    self.push(None);
                    self.cond(&i.by, "impersonation condition");
                    self.pop();
                }
                Decl::Consent(c) => {
                    for i in &c.intents {
                        if !self.m.intents.contains_key(&i.name) {
                            self.err(codes::E104, format!("unknown intent '{}'", i.name), i.span);
                        }
                    }
                }
                _ => {}
            }
        }
        let _ = self.ctx;
    }
}

fn s_span(s: &Stmt) -> Span {
    match s {
        Stmt::Reserve { span, .. } => *span,
        _ => Span::default(),
    }
}
