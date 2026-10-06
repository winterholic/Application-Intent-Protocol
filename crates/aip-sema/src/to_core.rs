//! AST + `Analysis` → Canonical Semantic IR (`aip-ir`).
//!
//! Name resolution here follows `check.rs` step by step (scope stack, row
//! fallback, enum values) so the IR says exactly what the checker decided.
//! Where the IR has no node for a construct, the closest existing form is used
//! and a `LowerNote` records it; nothing is silently dropped.

use crate::Analysis;
use crate::model::{EntityInfo, FieldInfo, FieldKind as MFieldKind, Model};
use crate::ty::Ty;
use aip_ir as ir;
use aip_syntax::Span;
use aip_syntax::ast;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    /// A name that no scope, row, enum or config explains.
    Unresolved,
    /// A construct lowered to the nearest existing IR form.
    Approx,
    /// The IR lacks a node for this; a change is proposed.
    Proposal,
    /// Observation only.
    Info,
}

#[derive(Debug, Clone)]
pub struct LowerNote {
    pub kind: NoteKind,
    pub line: u32,
    pub col: u32,
    pub message: String,
}

pub fn to_core(a: &Analysis<'_>) -> (ir::Program, ir::SourceMap) {
    let (p, sm, _) = to_core_with_notes(a);
    (p, sm)
}

pub fn to_core_with_notes(a: &Analysis<'_>) -> (ir::Program, ir::SourceMap, Vec<LowerNote>) {
    let mut l = Lower {
        a,
        m: &a.model,
        scopes: Vec::new(),
        sm: ir::SourceMap::new(),
        notes: Vec::new(),
        decl: String::new(),
        list: String::new(),
        here: String::new(),
        search_aliases: Vec::new(),
    };
    let p = l.program();
    (p, l.sm, l.notes)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BindKind {
    Param,
    Local,
}

struct Scope {
    vars: HashMap<String, (BindKind, ir::Type)>,
    /// Bare names fall back to fields of this row (entity or record).
    row: Option<ir::Type>,
}

struct Lower<'x, 'a> {
    a: &'x Analysis<'a>,
    m: &'x Model<'a>,
    scopes: Vec<Scope>,
    sm: ir::SourceMap,
    notes: Vec<LowerNote>,
    /// IR path of the declaration being lowered (`intents.Rename`, `reactions[2]`); parameters and statement lists hang off it.
    decl: String,
    /// IR path of the statement list being lowered (`intents.Rename.body`); statement `i` is recorded as `list[i]`.
    list: String,
    /// IR path of the statement being lowered; its nested blocks extend it (`.body`, `.on_failure`).
    here: String,
    /// Aliases the current query's `from Search.match(..) alias` binds; `alias.rank` lowers to the match score.
    search_aliases: Vec<String>,
}

fn is_upper_name(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_uppercase()) && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

// same factors as aip-pg `duration_seconds` (month = 30 days, year = 365 days)
fn dur_secs(d: &ast::Duration) -> i64 {
    use aip_syntax::lexer::DurUnit::*;
    d.n.max(0)
        * match d.unit {
            Sec => 1,
            Min => 60,
            Hour => 3600,
            Day => 86_400,
            Week => 604_800,
            Month => 2_592_000,
            Year => 31_536_000,
        }
}

fn dur(d: &ast::Duration) -> ir::Dur {
    use aip_syntax::lexer::DurUnit::*;
    let unit = match d.unit {
        Sec => ir::DurUnit::Second,
        Min => ir::DurUnit::Minute,
        Hour => ir::DurUnit::Hour,
        Day => ir::DurUnit::Day,
        Week => ir::DurUnit::Week,
        Month => ir::DurUnit::Month,
        Year => ir::DurUnit::Year,
    };
    ir::Dur { n: d.n, unit }
}

fn dur_opt(d: &Option<ast::Duration>) -> Option<i64> {
    d.as_ref().map(dur_secs)
}

pub fn ty_to_ir(t: &Ty) -> ir::Type {
    use ir::Type as T;
    match t {
        Ty::Bool => T::Bool,
        Ty::Int => T::Int { min: None, max: None },
        Ty::Decimal => T::Decimal { precision: None, scale: None },
        Ty::Text => T::Text { min: None, max: None, trim: false, lower: false, pattern: None },
        Ty::RichText => T::RichText { policy: None },
        Ty::Email => T::Email,
        Ty::Url => T::Url,
        Ty::Phone => T::Phone { region: None },
        Ty::Time => T::Time,
        Ty::Date => T::Date,
        Ty::Duration => T::Duration,
        Ty::Size => T::Size,
        Ty::Uuid => T::Uuid,
        Ty::Money(c) => T::Money { currency: c.clone() },
        Ty::Json => T::Json { schema: None, validated_by: None },
        Ty::Enum(n) => T::Enum { name: n.clone() },
        // the checker spells an event payload binding as record `event:<Name>`
        Ty::Record(n) => match n.strip_prefix("event:") {
            Some(ev) => T::Event { name: ev.to_string() },
            None => T::Record { name: n.clone() },
        },
        Ty::Entity(e) => T::Ref { entity: e.clone() },
        Ty::Union(v) => T::RefUnion { entities: v.clone() },
        Ty::Snapshot(e) => T::Snapshot { entity: e.clone() },
        Ty::Range(t) => T::Range { of: Box::new(ty_to_ir(t)) },
        Ty::Localized(t) => T::Localized { of: Box::new(ty_to_ir(t)) },
        Ty::Coll(t) => T::Many { of: Box::new(ty_to_ir(t)) },
        Ty::Object => T::Object,
        Ty::Upload => T::Upload { max_bytes: None, types: Vec::new() },
        Ty::Credential => T::Credential { kind: String::new() },
        Ty::Recurrence => T::Recurrence,
        Ty::Ext(n) => T::Ext { name: n.clone() },
        Ty::Null => T::Null,
        Ty::Unknown => T::Unknown,
    }
}

fn entity_of_type(t: &ir::Type) -> Option<&str> {
    match t {
        ir::Type::Ref { entity } | ir::Type::Snapshot { entity } => Some(entity),
        ir::Type::Many { of } => entity_of_type(of),
        _ => None,
    }
}

fn many_inner(t: ir::Type) -> ir::Type {
    match t {
        ir::Type::Many { of } => *of,
        o => o,
    }
}

/// The type the checker would give an expression of declared type `t`: refinements
/// (ranges, bounds, patterns) are not part of expression types, so `x` and `x: x` lower alike.
fn strip_refine(t: &ir::Type) -> ir::Type {
    use ir::Type as T;
    match t {
        T::Int { .. } => int_type(),
        T::RichText { .. } => T::RichText { policy: None },
        T::Decimal { .. } => T::Decimal { precision: None, scale: None },
        T::Text { .. } => T::Text { min: None, max: None, trim: false, lower: false, pattern: None },
        T::Phone { .. } => T::Phone { region: None },
        T::Json { .. } => T::Json { schema: None, validated_by: None },
        T::Upload { .. } => T::Upload { max_bytes: None, types: Vec::new() },
        T::Credential { .. } => T::Credential { kind: String::new() },
        T::Set { of, .. } | T::List { of, .. } | T::Many { of } => T::Many { of: Box::new(strip_refine(of)) },
        T::Range { of } => T::Range { of: Box::new(strip_refine(of)) },
        T::Localized { of } => T::Localized { of: Box::new(strip_refine(of)) },
        other => other.clone(),
    }
}

fn int_type() -> ir::Type {
    ir::Type::Int { min: None, max: None }
}

impl<'x, 'a> Lower<'x, 'a> {
    // ---------- infrastructure ----------

    fn note(&mut self, kind: NoteKind, span: Span, message: impl Into<String>) {
        self.notes.push(LowerNote { kind, line: span.line, col: span.col, message: message.into() });
    }

    fn mark(&mut self, key: String, span: Span) {
        self.sm.insert(key, ir::Pos { line: span.line, col: span.col });
    }

    fn push(&mut self, row: Option<ir::Type>) {
        self.scopes.push(Scope { vars: HashMap::new(), row });
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn bind(&mut self, name: &str, kind: BindKind, ty: ir::Type) {
        if let Some(s) = self.scopes.last_mut() {
            s.vars.insert(name.to_string(), (kind, ty));
        }
    }

    fn set_row(&mut self, row: ir::Type) {
        if let Some(s) = self.scopes.last_mut() {
            s.row = Some(row);
        }
    }

    fn lookup(&self, name: &str) -> Option<(BindKind, ir::Type)> {
        self.scopes.iter().rev().find_map(|s| s.vars.get(name).cloned())
    }

    /// Type of `name` on a row type, with the declaring entity/record.
    fn field_of(&self, row: &ir::Type, name: &str) -> Option<(String, ir::Type)> {
        match row {
            ir::Type::Ref { entity } | ir::Type::Snapshot { entity } => {
                let f = self.m.entity(entity)?.field(name)?;
                Some((entity.clone(), ty_to_ir(&f.ty)))
            }
            ir::Type::Record { name: r } => {
                let rec = self.m.records.get(r)?;
                let f = rec.fields.iter().find(|f| f.name.name == name)?;
                Some((r.clone(), strip_refine(&self.type_expr(&f.ty))))
            }
            _ => None,
        }
    }

    fn row_field(&self, name: &str) -> Option<(String, ir::Type)> {
        self.scopes.iter().rev().filter_map(|s| s.row.as_ref()).find_map(|row| self.field_of(row, name))
    }

    fn actor_type(&self) -> ir::Type {
        match self.m.actor_entity() {
            Some(e) => ir::Type::Ref { entity: e.to_string() },
            None => ir::Type::Unknown,
        }
    }

    /// Declared field type of `field` reached through `base`.
    fn field_type_through(&self, base: &ir::Type, field: &str) -> ir::Type {
        match base {
            ir::Type::Many { of } => ir::Type::Many { of: Box::new(self.field_type_through(of, field)) },
            other => self.field_of(other, field).map(|(_, t)| t).unwrap_or(ir::Type::Unknown),
        }
    }

    // ---------- types ----------

    fn type_name(&self, n: &str) -> ir::Type {
        use ir::Type as T;
        match n {
            "Bool" => T::Bool,
            "Int" => int_type(),
            "Decimal" => T::Decimal { precision: None, scale: None },
            "Text" => T::Text { min: None, max: None, trim: false, lower: false, pattern: None },
            "RichText" => T::RichText { policy: None },
            "Email" => T::Email,
            "Url" => T::Url,
            "Phone" => T::Phone { region: None },
            "Time" => T::Time,
            "Date" => T::Date,
            "Duration" => T::Duration,
            "Uuid" => T::Uuid,
            "Json" => T::Json { schema: None, validated_by: None },
            "Upload" => T::Upload { max_bytes: None, types: Vec::new() },
            "Recurrence" => T::Recurrence,
            n if self.m.enums.contains_key(n) => T::Enum { name: n.to_string() },
            n if self.m.records.contains_key(n) => T::Record { name: n.to_string() },
            n if self.m.entities.contains_key(n) => T::Ref { entity: n.to_string() },
            _ => T::Unknown,
        }
    }

    fn type_arg_text(t: &ast::TypeExpr) -> Option<String> {
        match &t.kind {
            ast::TypeKind::Name(q) => Some(q.text()),
            _ => None,
        }
    }

    /// Surface type → IR type, keeping refinements (ranges, lengths, patterns).
    fn type_expr(&self, t: &ast::TypeExpr) -> ir::Type {
        use ir::Type as T;
        let range = |opts: &[ast::RefineOpt]| {
            opts.iter().find_map(|o| if let ast::RefineOpt::Range(a, b) = o { Some((*a, *b)) } else { None }).unwrap_or((None, None))
        };
        let word = |opts: &[ast::RefineOpt], w: &str| opts.iter().any(|o| matches!(o, ast::RefineOpt::Word(x) if x.name == w));
        match &t.kind {
            ast::TypeKind::Name(q) if q.parts.len() > 1 => {
                if q.text() == "s3.Object" {
                    T::Object
                } else {
                    T::Ext { name: q.text() }
                }
            }
            ast::TypeKind::Name(q) => self.type_name(&q.parts[0].name),
            ast::TypeKind::Refined { base, opts } => match base.name.as_str() {
                "Int" => {
                    let (min, max) = range(opts);
                    T::Int { min, max }
                }
                "Text" => {
                    let (min, max) = range(opts);
                    let pattern = opts.iter().find_map(|o| if let ast::RefineOpt::Matches(r) = o { Some(r.clone()) } else { None });
                    T::Text { min, max, trim: word(opts, "trim"), lower: word(opts, "lower"), pattern }
                }
                "RichText" => T::RichText {
                    policy: opts.iter().find_map(|o| {
                        if let ast::RefineOpt::KeyValue(k, v) = o
                            && k.name == "policy"
                        {
                            Some(v.name.clone())
                        } else {
                            None
                        }
                    }),
                },
                "Decimal" => {
                    let mut ints = opts.iter().filter_map(|o| if let ast::RefineOpt::Int(n) = o { u32::try_from(*n).ok() } else { None });
                    let precision = ints.next();
                    let scale = ints.next();
                    T::Decimal { precision, scale }
                }
                "Phone" => T::Phone { region: opts.iter().find_map(|o| if let ast::RefineOpt::Word(w) = o { Some(w.name.clone()) } else { None }) },
                "Money" => T::Money {
                    currency: opts.iter().find_map(|o| if let ast::RefineOpt::Word(w) = o { Some(w.name.clone()) } else { None }).unwrap_or_default(),
                },
                "Upload" => T::Upload {
                    max_bytes: opts.iter().find_map(|o| if let ast::RefineOpt::Max(n) = o { Some(*n) } else { None }),
                    types: opts
                        .iter()
                        .find_map(|o| if let ast::RefineOpt::Types(ts) = o { Some(ts.iter().map(|t| t.name.clone()).collect()) } else { None })
                        .unwrap_or_default(),
                },
                other => self.type_name(other),
            },
            ast::TypeKind::Generic { base, arg, max } => {
                let inner = self.type_expr(arg);
                match base.name.as_str() {
                    "Set" => T::Set { of: Box::new(inner), max: max.unwrap_or(0) },
                    "List" => T::List { of: Box::new(inner), max: max.unwrap_or(0) },
                    "Range" => T::Range { of: Box::new(inner) },
                    "Localized" => T::Localized { of: Box::new(inner) },
                    "Json" => T::Json { schema: Self::type_arg_text(arg), validated_by: None },
                    "Credential" => T::Credential { kind: Self::type_arg_text(arg).unwrap_or_default() },
                    "Snapshot" => match inner {
                        T::Ref { entity } => T::Snapshot { entity },
                        _ => T::Unknown,
                    },
                    _ => T::Unknown,
                }
            }
            ast::TypeKind::JsonValidated(q) => T::Json { schema: None, validated_by: Some(q.text()) },
            ast::TypeKind::Many(n) => T::Many { of: Box::new(self.type_name(&n.name)) },
            ast::TypeKind::Union(alts) => T::RefUnion { entities: alts.iter().map(|a| a.name.clone()).collect() },
        }
    }

    /// `type_expr` for a declaration site: reports refinement options that have no IR field.
    fn declared_type(&mut self, t: &ast::TypeExpr) -> ir::Type {
        self.note_type_loss(t);
        self.type_expr(t)
    }

    fn note_type_loss(&mut self, t: &ast::TypeExpr) {
        use ast::RefineOpt as O;
        match &t.kind {
            ast::TypeKind::Refined { base, opts } => {
                for o in opts {
                    let consumed = match (base.name.as_str(), o) {
                        ("Int", O::Range(..)) | ("Text", O::Range(..) | O::Matches(_)) | ("Decimal", O::Int(_)) => true,
                        ("RichText", O::KeyValue(k, _)) => k.name == "policy",
                        ("Text", O::Word(w)) => w.name == "trim" || w.name == "lower",
                        ("Phone" | "Money", O::Word(_)) | ("Upload", O::Max(_) | O::Types(_)) => true,
                        _ => false,
                    };
                    if !consumed {
                        self.note(
                            NoteKind::Proposal,
                            t.span,
                            format!("IR change proposal: refinement option of {}(...) has no IR field and is dropped", base.name),
                        );
                    }
                }
            }
            ast::TypeKind::Generic { arg, .. } => self.note_type_loss(arg),
            _ => {}
        }
    }

    // ---------- program ----------

    fn program(&mut self) -> ir::Program {
        let m = self.m;
        let mut p = ir::Program {
            aip_core: ir::CORE_IR_VERSION.to_string(),
            uses: m.uses.iter().cloned().collect(),
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
        for d in &m.file.decls {
            match d {
                ast::Decl::Removed(x) => p.removed_entities.push(x.name.name.clone()),
                ast::Decl::Use(q) => {
                    if let Some(i) = m.uses.iter().position(|u| *u == q.text()) {
                        self.mark(format!("uses[{i}]"), q.span);
                    }
                }
                ast::Decl::Actor(x) => p.actor = Some(self.actor(x)),
                ast::Decl::Enum(x) => {
                    self.mark(format!("enums.{}", x.name.name), x.span);
                    p.enums
                        .insert(x.name.name.clone(), ir::EnumDef { ordered: x.ordered, values: x.values.iter().map(|v| v.name.clone()).collect() });
                }
                ast::Decl::Record(x) => {
                    self.mark(format!("records.{}", x.name.name), x.span);
                    let def = self.record(x);
                    p.records.insert(x.name.name.clone(), def);
                }
                ast::Decl::Entity(x) => {
                    self.mark(format!("entities.{}", x.name.name), x.span);
                    if let Some(info) = m.entities.get(&x.name.name) {
                        let e = self.entity(info);
                        p.entities.insert(x.name.name.clone(), e);
                    }
                }
                ast::Decl::Relation(x) => {
                    self.mark(format!("relations.{}", x.name.name), x.span);
                    self.decl = format!("relations.{}", x.name.name);
                    let r = self.relation(x);
                    p.relations.insert(x.name.name.clone(), r);
                }
                ast::Decl::Fn(x) => {
                    self.mark(format!("fns.{}", x.name.name), x.span);
                    let f = self.fn_def(x);
                    p.fns.insert(x.name.name.clone(), f);
                }
                ast::Decl::Event(x) => self.mark(format!("events.{}", x.name.name), x.span),
                ast::Decl::Config(x) => {
                    self.mark(format!("configs.{}", x.name.name), x.span);
                    let c = self.config(x);
                    p.configs.insert(x.name.name.clone(), c);
                }
                ast::Decl::Flag(x) => {
                    self.mark(format!("flags.{}", x.name.name), x.span);
                    p.flags.insert(x.name.name.clone(), ir::FlagDef { default_on: x.default_on, rollout: x.rollout });
                }
                ast::Decl::Query(x) => {
                    self.mark(format!("intents.{}", x.name.name), x.span);
                    self.mark(format!("intents.{}.name", x.name.name), x.name.span);
                    self.decl = format!("intents.{}", x.name.name);
                    let q = self.query(x);
                    p.intents.insert(x.name.name.clone(), ir::Intent::Query(q));
                }
                ast::Decl::Command(x) => {
                    self.mark(format!("intents.{}", x.name.name), x.span);
                    self.mark(format!("intents.{}.name", x.name.name), x.name.span);
                    self.decl = format!("intents.{}", x.name.name);
                    let c = self.command(x);
                    p.intents.insert(x.name.name.clone(), ir::Intent::Command(c));
                }
                ast::Decl::Expose(x) => {
                    self.note(
                        NoteKind::Info,
                        x.span,
                        format!(
                            "expose {} stays in the AST after lower::expand; its generated intents are lowered, the decl itself is skipped",
                            x.entity.name
                        ),
                    );
                }
                ast::Decl::OnEvent(_)
                | ast::Decl::Schedule(_)
                | ast::Decl::Retain(_)
                | ast::Decl::Rule(_)
                | ast::Decl::Webhook(_)
                | ast::Decl::Consume(_) => {
                    let idx = p.reactions.len();
                    self.mark(format!("reactions[{idx}]"), decl_span(d));
                    self.decl = format!("reactions[{idx}]");
                    if let Some(n) = d.name() {
                        self.mark(format!("reactions[{idx}].name"), n.span);
                    }
                    if let Some(r) = self.reaction(d) {
                        p.reactions.push(r);
                    }
                }
                ast::Decl::Subscribe(_)
                | ast::Decl::Projection(_)
                | ast::Decl::Search(_)
                | ast::Decl::Job(_)
                | ast::Decl::Verification(_)
                | ast::Decl::GrantLink(_)
                | ast::Decl::Approval(_)
                | ast::Decl::OutboundWebhooks(_)
                | ast::Decl::Consent(_)
                | ast::Decl::Impersonate(_)
                | ast::Decl::Migration(_)
                | ast::Decl::Upcast(_) => {
                    let idx = p.forms.len();
                    self.mark(format!("forms[{idx}]"), decl_span(d));
                    self.decl = format!("forms[{idx}]");
                    if let Some(n) = d.name() {
                        self.mark(format!("forms[{idx}].name"), n.span);
                    }
                    if let Some(f) = self.form(d) {
                        p.forms.push(f);
                    }
                }
            }
        }
        p.events = self.events();
        p
    }

    fn events(&mut self) -> BTreeMap<String, ir::EventDef> {
        let m = self.m;
        let mut out = BTreeMap::new();
        for (name, shape) in &m.events {
            let decl = m.file.decls.iter().find_map(|d| match d {
                ast::Decl::Event(e) if e.name.name == *name => Some(e),
                _ => None,
            });
            let def = match decl {
                Some(e) => ir::EventDef {
                    version: e.version,
                    fields: e.fields.iter().map(|(n, t)| (n.name.clone(), self.declared_type(t))).collect(),
                    declared: true,
                },
                None => ir::EventDef {
                    version: None,
                    fields: shape.fields.iter().map(|(n, t)| (n.clone(), ty_to_ir(t))).collect(),
                    declared: shape.declared,
                },
            };
            out.insert(name.clone(), def);
        }
        out
    }

    // ---------- declarations ----------

    fn actor(&mut self, x: &ast::ActorDecl) -> ir::Actor {
        self.push(None);
        let provider = self.call_target(&x.via);
        self.pop();
        self.push(Some(ir::Type::Ref { entity: x.name.name.clone() }));
        let superuser = x.superuser.as_ref().map(|e| self.expr(e));
        self.pop();
        ir::Actor { entity: x.name.name.clone(), provider, scopes: x.scopes.iter().map(|q| q.text()).collect(), superuser }
    }

    fn record(&mut self, x: &ast::RecordDecl) -> ir::RecordDef {
        let row = ir::Type::Record { name: x.name.name.clone() };
        let fields = x.fields.iter().map(|f| self.decl_field(f, ir::FieldKind::Stored, &row)).collect();
        self.push(Some(row));
        let checks = x
            .checks
            .iter()
            .map(|c| ir::Require { when: c.when.as_ref().map(|w| self.expr(w)), cond: self.expr(&c.cond), code: c.code.name.clone() })
            .collect();
        self.pop();
        ir::RecordDef { fields, checks }
    }

    /// Field with its modifiers; `kind` is decided by the caller (entity fields use the model's classification).
    fn decl_field(&mut self, f: &ast::FieldDecl, kind: ir::FieldKind, row: &ir::Type) -> ir::Field {
        let ty = self.declared_type(&f.ty);
        self.push(Some(row.clone()));
        let default = f.default.as_ref().map(|d| self.expr(d));
        let mut field = ir::Field {
            name: f.name.name.clone(),
            was: None,
            ty,
            optional: f.ty.optional,
            default,
            kind,
            personal: false,
            encrypted: false,
            visible_to: None,
            masked: None,
            generated: None,
            variants: Vec::new(),
        };
        for m in &f.mods {
            match m {
                ast::FieldMod::Was(old) => field.was = Some(old.name.clone()),
                ast::FieldMod::Personal(_) => field.personal = true,
                ast::FieldMod::Encrypted(_) => field.encrypted = true,
                ast::FieldMod::VisibleTo(e) => field.visible_to = Some(self.expr(e)),
                ast::FieldMod::Masked { unless, with } => {
                    let unless = self.expr(unless);
                    let with = self.call_target(with);
                    field.masked = Some(ir::Masked { unless, with });
                }
                ast::FieldMod::Tree { max_depth, .. } => field.generated = Some(ir::Generated::Tree { max_depth: *max_depth }),
                ast::FieldMod::Sequence { per, format, .. } => {
                    field.generated =
                        Some(ir::Generated::Sequence { per: per.parts.iter().map(|p| p.name.clone()).collect(), format: format.clone() })
                }
                ast::FieldMod::Slug { from, per, .. } => {
                    field.generated = Some(ir::Generated::Slug {
                        from: from.name.clone(),
                        per: per.as_ref().map(|q| q.parts.iter().map(|p| p.name.clone()).collect()).unwrap_or_default(),
                    })
                }
                ast::FieldMod::Position { within, .. } => {
                    field.generated = Some(ir::Generated::Position { within: within.parts.iter().map(|p| p.name.clone()).collect() })
                }
                ast::FieldMod::Variants(vs) => field.variants = vs.iter().map(|(k, v)| (k.name.clone(), v.clone())).collect(),
                // consumed by the field kind
                ast::FieldMod::OnDelete(_) | ast::FieldMod::OnErase(_) | ast::FieldMod::Via(_) | ast::FieldMod::Counter { .. } => {}
            }
        }
        self.pop();
        field
    }

    fn ref_policy(&mut self, p: &ast::RefPolicy, row: &ir::Type) -> ir::RefPolicy {
        match p {
            ast::RefPolicy::Cascade(_) => ir::RefPolicy::Cascade,
            ast::RefPolicy::Restrict(_) => ir::RefPolicy::Restrict,
            ast::RefPolicy::SetNull(_) => ir::RefPolicy::SetNull,
            ast::RefPolicy::Anonymize(_) => ir::RefPolicy::Anonymize,
            ast::RefPolicy::Reassign(e) => {
                self.push(Some(row.clone()));
                let to = self.expr(e);
                self.pop();
                ir::RefPolicy::Reassign { to }
            }
        }
    }

    fn entity_field(&mut self, entity: &str, f: &FieldInfo<'_>, row: &ir::Type) -> ir::Field {
        let Some(decl) = f.decl else {
            return ir::Field {
                name: f.name.clone(),
                was: None,
                ty: ty_to_ir(&f.ty),
                optional: f.optional,
                default: None,
                kind: ir::FieldKind::Implicit,
                personal: false,
                encrypted: false,
                visible_to: None,
                masked: None,
                generated: None,
                variants: Vec::new(),
            };
        };
        let on_delete = decl.mods.iter().find_map(|m| if let ast::FieldMod::OnDelete(p) = m { Some(p) } else { None });
        let on_erase = decl.mods.iter().find_map(|m| if let ast::FieldMod::OnErase(p) = m { Some(p) } else { None });
        let kind = match &f.kind {
            MFieldKind::Stored => ir::FieldKind::Stored,
            MFieldKind::Implicit => ir::FieldKind::Implicit,
            MFieldKind::Ref(t) => ir::FieldKind::Ref {
                target: t.clone(),
                on_delete: on_delete.map(|p| self.ref_policy(p, row)),
                on_erase: on_erase.map(|p| self.ref_policy(p, row)),
            },
            MFieldKind::UnionRef(ts) => ir::FieldKind::RefUnion {
                targets: ts.clone(),
                on_delete: on_delete.map(|p| self.ref_policy(p, row)),
                on_erase: on_erase.map(|p| self.ref_policy(p, row)),
            },
            MFieldKind::Inverse { target, via } => ir::FieldKind::Inverse { target: target.clone(), via: via.clone() },
            MFieldKind::Counter { via } => {
                let opts = decl.mods.iter().find_map(|m| if let ast::FieldMod::Counter { opts, .. } = m { Some(opts) } else { None });
                let mut dedupe = None;
                let mut window_seconds = None;
                let mut sharded = None;
                for o in opts.into_iter().flatten() {
                    match o {
                        ast::CounterOpt::Dedupe { by, within } => {
                            dedupe = Some(ir::CounterDedupe { by: by.name.clone(), within_seconds: dur_secs(within) })
                        }
                        ast::CounterOpt::Window(d) => window_seconds = Some(dur_secs(d)),
                        ast::CounterOpt::Sharded(n) => sharded = Some(*n),
                    }
                }
                ir::FieldKind::Counter { store: via.clone(), dedupe, window_seconds, sharded }
            }
        };
        let mut field = self.decl_field(decl, kind, row);
        field.optional = f.optional;
        let fpath = format!("entities.{entity}.fields.{}", f.name);
        self.mark(fpath.clone(), decl.span);
        self.mark_field_parts(&fpath, decl);
        if let Some(span) = decl.mods.iter().find_map(|m| if let ast::FieldMod::Encrypted(s) = m { Some(*s) } else { None }) {
            self.mark(format!("entities.{entity}.fields.{}.encrypted", f.name), span);
        }
        field
    }

    /// Positions of the parts of a field a semantic rule can point at (the IR paths `analyze` reports).
    fn mark_field_parts(&mut self, fpath: &str, decl: &ast::FieldDecl) {
        let policy_span = |p: &ast::RefPolicy| match p {
            ast::RefPolicy::Cascade(s) | ast::RefPolicy::Restrict(s) | ast::RefPolicy::SetNull(s) | ast::RefPolicy::Anonymize(s) => *s,
            ast::RefPolicy::Reassign(e) => e.span,
        };
        for m in &decl.mods {
            match m {
                ast::FieldMod::OnDelete(p) => self.mark(format!("{fpath}.kind.on_delete"), policy_span(p)),
                ast::FieldMod::OnErase(p) => self.mark(format!("{fpath}.kind.on_erase"), policy_span(p)),
                ast::FieldMod::Via(v) => self.mark(format!("{fpath}.kind"), v.span),
                ast::FieldMod::Counter { span, .. } => self.mark(format!("{fpath}.kind"), *span),
                ast::FieldMod::Tree { span, .. }
                | ast::FieldMod::Sequence { span, .. }
                | ast::FieldMod::Slug { span, .. }
                | ast::FieldMod::Position { span, .. } => self.mark(format!("{fpath}.generated"), *span),
                ast::FieldMod::VisibleTo(x) => self.mark(format!("{fpath}.visible_to"), x.span),
                ast::FieldMod::Masked { unless, .. } => self.mark(format!("{fpath}.masked.unless"), unless.span),
                _ => {}
            }
        }
        if let ast::TypeKind::JsonValidated(q) = &decl.ty.kind {
            self.mark(format!("{fpath}.ty"), q.span);
        }
        if let Some(d) = &decl.default {
            self.mark(format!("{fpath}.default"), d.span);
        }
    }

    fn entity(&mut self, info: &EntityInfo<'_>) -> ir::Entity {
        let decl = info.decl;
        let name = decl.name.name.clone();
        let row = ir::Type::Ref { entity: name.clone() };
        let fields: Vec<ir::Field> = info.fields.iter().map(|f| self.entity_field(&name, f, &row)).collect();
        let mut e = ir::Entity {
            was: decl.was.as_ref().map(|w| w.name.clone()),
            removed_fields: Vec::new(),
            personal: decl.personal,
            access_audited: decl.access_audited,
            fields,
            constraints: Vec::new(),
            lifecycles: Vec::new(),
            visibility: None,
            predicates: BTreeMap::new(),
            traits: ir::Traits::default(),
        };
        for (ordinal, m) in decl.members.iter().enumerate() {
            match m {
                ast::EntityMember::Field(_) => {}
                ast::EntityMember::RemovedField(x) => e.removed_fields.push(x.name.clone()),
                ast::EntityMember::Constraint(c) => {
                    let cpath = format!("entities.{name}.constraints[{}]", e.constraints.len());
                    self.mark(cpath.clone(), constraint_span(c));
                    match c {
                        ast::Constraint::Unique { filter: Some(f), .. } => self.mark(format!("{cpath}.filter"), f.span),
                        ast::Constraint::Cardinality { filter, .. } => self.mark(format!("{cpath}.filter"), filter.span),
                        ast::Constraint::Invariant { cond, .. } => self.mark(format!("{cpath}.cond"), cond.span),
                        _ => {}
                    }
                    self.push(Some(row.clone()));
                    let c = self.constraint(c, ordinal as u32, &cpath);
                    self.pop();
                    e.constraints.push(c);
                }
                ast::EntityMember::Lifecycle(l) => {
                    self.mark(format!("entities.{name}.lifecycles[{}]", e.lifecycles.len()), l.span);
                    e.lifecycles.push(ir::Lifecycle {
                        field: l.field.name.clone(),
                        transitions: l
                            .transitions
                            .iter()
                            .map(|t| ir::Transition {
                                from: t.from.iter().map(|v| v.name.clone()).collect(),
                                to: t.to.iter().map(|v| v.name.clone()).collect(),
                            })
                            .collect(),
                    });
                }
                ast::EntityMember::Visibility(v) => {
                    if e.visibility.is_none() {
                        self.mark(format!("entities.{name}.visibility"), v.span);
                    }
                    self.push(Some(row.clone()));
                    let cond = self.expr(&v.cond);
                    self.pop();
                    if e.visibility.is_some() {
                        self.note(NoteKind::Approx, v.span, format!("{name} has several visibility rules; only the first is kept"));
                    } else {
                        e.visibility = Some(ir::Visibility { unless: v.unless, cond });
                    }
                }
                ast::EntityMember::Predicate(p) => {
                    self.mark(format!("entities.{name}.predicates.{}", p.name.name), p.body.span);
                    self.push(Some(row.clone()));
                    let body = self.expr(&p.body);
                    self.pop();
                    e.predicates.insert(p.name.name.clone(), body);
                }
                ast::EntityMember::Trait(t) => match t {
                    ast::EntityTrait::Track { created, updated, by_actor, .. } => {
                        e.traits.track_created |= created;
                        e.traits.track_updated |= updated;
                        e.traits.track_by_actor |= by_actor;
                    }
                    ast::EntityTrait::History(_) => e.traits.history = true,
                    ast::EntityTrait::SoftDelete { retain, .. } => e.traits.soft_delete = Some(dur_opt(retain)),
                    ast::EntityTrait::Versioned(_) => e.traits.versioned = true,
                    ast::EntityTrait::Publishable { by, span } => {
                        e.traits.publishable = true;
                        self.mark(format!("entities.{name}.traits.publishable"), *span);
                        if let Some(b) = by {
                            self.mark(format!("entities.{name}.traits.publish_by"), b.span);
                            self.push(Some(row.clone()));
                            e.traits.publish_by = Some(self.expr(b));
                            self.pop();
                        }
                    }
                    ast::EntityTrait::Tenant { via, span } => {
                        e.traits.tenant = Some(via.parts.iter().map(|p| p.name.clone()).collect());
                        self.mark(format!("entities.{name}.traits.tenant"), *span);
                    }
                    ast::EntityTrait::DynamicSchema { from, span } => {
                        e.traits.dynamic_schema = Some(from.name.clone());
                        self.mark(format!("entities.{name}.traits.dynamic_schema"), *span);
                    }
                },
            }
        }
        e
    }

    fn constraint(&mut self, c: &ast::Constraint, ordinal: u32, path: &str) -> ir::Constraint {
        match c {
            ast::Constraint::Unique { cols, filter, code, .. } => ir::Constraint::Unique {
                ordinal,
                fields: cols.iter().map(|c| c.name.clone()).collect(),
                filter: filter.as_ref().map(|f| self.expr(f)),
                code: code.as_ref().map(|c| c.name.clone()),
            },
            ast::Constraint::Cardinality { kind, filter, per, repair, .. } => {
                let filter = self.expr(filter);
                let repair = repair.as_ref().map(|b| {
                    // inside the repair block `per` is a plain binding and the entity row is out of scope
                    let per_ty = self.scopes.iter().rev().filter_map(|s| s.row.as_ref()).find_map(|r| self.field_of(r, &per.name)).map(|(_, t)| t);
                    let idx = self.scopes.iter().rposition(|s| s.row.is_some());
                    let saved = idx.and_then(|i| self.scopes[i].row.take());
                    self.push(None);
                    self.bind(&per.name, BindKind::Local, per_ty.unwrap_or(ir::Type::Unknown));
                    let body = self.block_at(format!("{path}.repair"), b);
                    self.pop();
                    if let (Some(i), Some(r)) = (idx, saved) {
                        self.scopes[i].row = Some(r);
                    }
                    body
                });
                ir::Constraint::Cardinality {
                    ordinal,
                    kind: match kind {
                        ast::CardKind::ExactlyOne => ir::Cardinality::ExactlyOne,
                        ast::CardKind::AtMostOne => ir::Cardinality::AtMostOne,
                        ast::CardKind::AtLeastOne => ir::Cardinality::AtLeastOne,
                    },
                    filter,
                    per: per.name.clone(),
                    repair,
                }
            }
            ast::Constraint::NoOverlap { range, per, code, .. } => {
                ir::Constraint::NoOverlap { ordinal, range: range.name.clone(), per: per.name.clone(), code: code.as_ref().map(|c| c.name.clone()) }
            }
            ast::Constraint::Capacity { count, limit, code, .. } => {
                self.push(None);
                let (count, _) = self.set_expr(count);
                self.pop();
                ir::Constraint::Capacity { ordinal, count, limit: self.expr(limit), code: code.name.clone() }
            }
            ast::Constraint::Invariant { name, cond, .. } => ir::Constraint::Invariant { ordinal, name: name.name.clone(), cond: self.expr(cond) },
        }
    }

    fn params(&mut self, ps: &[ast::Param]) -> Vec<ir::Param> {
        let mut out = Vec::new();
        for p in ps {
            self.mark(format!("{}.params.{}", self.decl, p.name.name), p.ty.span);
            let ty = self.declared_type(&p.ty);
            let default = p.default.as_ref().map(|d| self.expr(d));
            self.bind(&p.name.name, BindKind::Param, strip_refine(&ty));
            out.push(ir::Param { name: p.name.name.clone(), ty, optional: p.ty.optional, default });
        }
        out
    }

    fn relation(&mut self, x: &ast::RelationDecl) -> ir::Relation {
        self.push(None);
        let params = self.params(&x.params);
        self.mark(format!("{}.body", self.decl), x.body.span);
        let body = self.expr(&x.body);
        self.pop();
        ir::Relation { params, result: x.result.as_ref().map(|r| r.name.clone()), body }
    }

    fn fn_def(&mut self, x: &ast::FnDecl) -> ir::FnDef {
        self.push(None);
        let params = self.params(&x.params);
        let body = match &x.body {
            ast::FnBody::Expr(e) => {
                self.note(
                    NoteKind::Info,
                    x.span,
                    format!("fn {}: check.rs does not visit fn bodies, so expression types may be Unknown", x.name.name),
                );
                ir::FnBody::Expr { expr: self.expr(e) }
            }
            ast::FnBody::Wasm(m) => ir::FnBody::Wasm { module: m.clone() },
        };
        self.pop();
        ir::FnDef { params, ret: self.declared_type(&x.ret), body }
    }

    fn config(&mut self, x: &ast::ConfigDecl) -> ir::ConfigDef {
        self.push(None);
        let default = x.default.as_ref().map(|d| self.expr(d));
        self.pop();
        ir::ConfigDef { ty: self.declared_type(&x.ty), optional: x.ty.optional, default }
    }

    // ---------- calls ----------

    fn callee(&self, q: &ast::QualName) -> ir::Callee {
        let name = q.text();
        if q.parts.len() > 1 {
            return ir::Callee::Ext { name };
        }
        if self.m.relations.contains_key(&name) {
            ir::Callee::Relation { name }
        } else if self.m.fns.contains_key(&name) {
            ir::Callee::Fn { name }
        } else if self.m.intents.contains_key(&name) {
            ir::Callee::Intent { name }
        } else {
            ir::Callee::Builtin { name }
        }
    }

    /// Arguments of a `via` target: a bare identifier such as `kakao` in
    /// `auth.oidc(kakao)` names a provider variant, not a value in scope.
    fn args(&mut self, args: &[ast::Arg], symbolic: bool) -> Vec<ir::Arg> {
        args.iter()
            .map(|a| {
                let value = match &a.value.kind {
                    ast::ExprKind::Name(id) if symbolic && self.try_name(id, None).is_none() => {
                        ir::Expr { ty: ir::Type::Symbol, node: ir::Node::Symbol { name: id.name.clone() } }
                    }
                    _ => self.expr(&a.value),
                };
                ir::Arg { name: a.name.as_ref().map(|n| n.name.clone()), value }
            })
            .collect()
    }

    fn call_target(&mut self, t: &ast::CallTarget) -> ir::Call {
        ir::Call { callee: ir::Callee::Ext { name: t.name.text() }, args: self.args(t.args.as_deref().unwrap_or(&[]), true) }
    }

    fn call_expr(&mut self, c: &ast::CallExpr) -> ir::Call {
        let callee = self.callee(&c.callee);
        if matches!(&callee, ir::Callee::Builtin { name } if name == "flag")
            && let Some(ast::Arg { value: ast::Expr { kind: ast::ExprKind::Name(n), .. }, .. }) = c.args.first()
            && self.m.flags.contains(&n.name)
        {
            let arg = ir::Arg { name: None, value: ir::Expr { ty: ir::Type::Bool, node: ir::Node::Config { name: n.name.clone() } } };
            return ir::Call { callee, args: vec![arg] };
        }
        ir::Call { callee, args: self.args(&c.args, false) }
    }

    // ---------- names ----------

    /// Resolution order of check.rs `name()`: bindings, row fields, enum values; then configs and flags.
    fn try_name(&self, id: &ast::Ident, sema: Option<&Ty>) -> Option<(ir::Node, ir::Type)> {
        if let Some((kind, ty)) = self.lookup(&id.name) {
            let node = match kind {
                BindKind::Param => ir::Node::Param { name: id.name.clone() },
                BindKind::Local => ir::Node::Local { name: id.name.clone() },
            };
            return Some((node, ty));
        }
        if let Some((owner, ty)) = self.row_field(&id.name) {
            let node = if self.m.entities.contains_key(&owner) {
                ir::Node::RowField { entity: owner, field: id.name.clone() }
            } else {
                ir::Node::RecordField { record: owner, field: id.name.clone() }
            };
            return Some((node, ty));
        }
        let from_sema = match sema {
            Some(Ty::Enum(en)) if self.m.enums.get(en).is_some_and(|d| d.values.iter().any(|v| v.name == id.name)) => Some(en.clone()),
            _ => None,
        };
        let en = from_sema.or_else(|| if is_upper_name(&id.name) { self.m.enum_of_value(&id.name, None) } else { None });
        if let Some(en) = en {
            return Some((ir::Node::EnumValue { enum_name: en.clone(), value: id.name.clone() }, ir::Type::Enum { name: en }));
        }
        if let Some(c) = self.m.configs.get(&id.name) {
            return Some((ir::Node::Config { name: id.name.clone() }, strip_refine(&self.type_expr(&c.ty))));
        }
        if self.m.flags.contains(&id.name) {
            return Some((ir::Node::Config { name: id.name.clone() }, ir::Type::Bool));
        }
        None
    }

    fn name_node(&mut self, id: &ast::Ident, sema: Option<&Ty>) -> (ir::Node, ir::Type) {
        match self.try_name(id, sema) {
            Some(r) => r,
            None => {
                self.note(NoteKind::Unresolved, id.span, format!("unresolved name '{}'", id.name));
                (ir::Node::Local { name: id.name.clone() }, ir::Type::Unknown)
            }
        }
    }

    /// `name` in `{ name }` / `insert ... { name }`: the same-named binding.
    fn shorthand(&mut self, id: &ast::Ident) -> ir::Expr {
        let (node, ty) = self.name_node(id, None);
        ir::Expr { ty, node }
    }

    // ---------- expressions ----------

    fn expr(&mut self, e: &ast::Expr) -> ir::Expr {
        let sema = self.a.ty_of(e);
        let (node, hint) = self.node(e, &sema);
        let mut ty = ty_to_ir(&sema);
        if ty == ir::Type::Unknown {
            ty = match hint {
                Some(t) if t != ir::Type::Unknown => t,
                _ => self.infer(&node),
            };
        }
        ir::Expr { ty, node }
    }

    /// `row.signingSecret` of an endpoint entity that has no field of that name.
    fn is_endpoint_secret(&self, base: &ast::Expr) -> bool {
        match self.a.ty_of(base) {
            Ty::Entity(e) => self.m.endpoints.contains(&e) && self.m.entity(&e).is_some_and(|x| x.field("signingSecret").is_none()),
            _ => false,
        }
    }

    fn search_alias(&self, base: &ast::Expr) -> Option<String> {
        match &base.kind {
            ast::ExprKind::Name(n) if self.search_aliases.contains(&n.name) => Some(n.name.clone()),
            _ => None,
        }
    }

    fn bx(&mut self, e: &ast::Expr) -> Box<ir::Expr> {
        Box::new(self.expr(e))
    }

    /// Type for nodes the checker never visited (fn bodies, consume handlers, ...).
    fn infer(&self, n: &ir::Node) -> ir::Type {
        use ir::Node as N;
        let text = ir::Type::Text { min: None, max: None, trim: false, lower: false, pattern: None };
        match n {
            N::Lit { lit } => match lit {
                ir::Literal::Int(_) => int_type(),
                ir::Literal::Decimal(_) => ir::Type::Decimal { precision: None, scale: None },
                ir::Literal::Text(_) => text,
                ir::Literal::Bool(_) => ir::Type::Bool,
                ir::Literal::Null => ir::Type::Null,
                ir::Literal::Duration(_) => ir::Type::Duration,
                ir::Literal::SizeBytes(_) => ir::Type::Size,
                ir::Literal::TimeOfDay(..) => ir::Type::Time,
            },
            N::Actor => self.actor_type(),
            N::Now => ir::Type::Time,
            N::Today => ir::Type::Date,
            N::Public | N::Authenticated | N::SelfRow => ir::Type::Bool,
            N::Unary { op: ir::UnOp::Not, .. }
            | N::InList { .. }
            | N::InRange { .. }
            | N::InSet { .. }
            | N::InRangeValue { .. }
            | N::TypeTest { .. }
            | N::Pred { .. }
            | N::HasScope { .. }
            | N::Exists { .. }
            | N::Quant { .. } => ir::Type::Bool,
            N::Unary { arg, .. } => arg.ty.clone(),
            N::Binary { op, l, .. } => match op {
                ir::BinOp::Add | ir::BinOp::Sub | ir::BinOp::Mul | ir::BinOp::Div => l.ty.clone(),
                _ => ir::Type::Bool,
            },
            N::Field { base, field, .. } => self.field_type_through(&base.ty, field),
            N::If { then, .. } => then.ty.clone(),
            N::Call(c) => match &c.callee {
                ir::Callee::Ext { name } => ir::Type::Ext { name: name.clone() },
                _ => ir::Type::Unknown,
            },
            _ => ir::Type::Unknown,
        }
    }

    fn node(&mut self, e: &ast::Expr, sema: &Ty) -> (ir::Node, Option<ir::Type>) {
        use ast::ExprKind as K;
        use ir::Node as N;
        let lit = |l: ir::Literal| (N::Lit { lit: l }, None);
        match &e.kind {
            K::Int(n) => lit(ir::Literal::Int(*n)),
            K::Decimal(s) => lit(ir::Literal::Decimal(s.clone())),
            K::Str(s) => lit(ir::Literal::Text(s.clone())),
            K::Bool(b) => lit(ir::Literal::Bool(*b)),
            K::Null => lit(ir::Literal::Null),
            K::Duration(d) => lit(ir::Literal::Duration(dur(d))),
            K::Size(n) => lit(ir::Literal::SizeBytes(*n)),
            K::TimeOfDay(h, m) => lit(ir::Literal::TimeOfDay(*h, *m)),
            K::Name(id) => {
                let (n, t) = self.name_node(id, Some(sema));
                (n, Some(t))
            }
            K::Kw(k) => (
                match k {
                    ast::Kw::Actor => N::Actor,
                    ast::Kw::SelfRow => N::SelfRow,
                    ast::Kw::This => N::This,
                    ast::Kw::Now => N::Now,
                    ast::Kw::Today => N::Today,
                    ast::Kw::Public => N::Public,
                    ast::Kw::Authenticated => N::Authenticated,
                },
                None,
            ),
            K::Field(base, f) if f.name == "rank" && self.search_alias(base).is_some() => {
                (N::SearchRank { alias: self.search_alias(base).unwrap_or_default() }, None)
            }
            K::Field(base, f) if f.name == "signingSecret" && self.is_endpoint_secret(base) => {
                (N::SigningSecret { base: self.bx(base) }, Some(ir::Type::Text { min: None, max: None, trim: false, lower: false, pattern: None }))
            }
            K::Field(base, f) => {
                let base = self.bx(base);
                let entity = entity_of_type(&base.ty).map(str::to_string);
                (N::Field { base, entity, field: f.name.clone() }, None)
            }
            K::Call(c) => (N::Call(self.call_expr(c)), None),
            K::Neg(x) => (N::Unary { op: ir::UnOp::Neg, arg: self.bx(x) }, None),
            K::Not(x) => (N::Unary { op: ir::UnOp::Not, arg: self.bx(x) }, None),
            K::Binary(op, l, r) => (N::Binary { op: bin_op(*op), l: self.bx(l), r: self.bx(r) }, None),
            K::InList(x, items) => (N::InList { value: self.bx(x), list: items.iter().map(|i| self.expr(i)).collect() }, None),
            K::InRange(x, lo, hi) => (N::InRange { value: self.bx(x), lo: self.bx(lo), hi: self.bx(hi) }, None),
            K::InExpr(x, r) => {
                let value = self.bx(x);
                let rhs = self.bx(r);
                let node =
                    if matches!(rhs.ty, ir::Type::Range { .. }) { N::InRangeValue { value, range: rhs } } else { N::InSet { value, set: rhs } };
                (node, None)
            }
            K::Is(x, name) => (self.is_node(e.span, x, name), None),
            K::HasScope(x, scope) => (N::HasScope { subject: self.bx(x), scope: scope.text() }, None),
            K::Exists(se) => {
                self.push(None);
                let (set, _) = self.set_expr(se);
                self.pop();
                (N::Exists { set: Box::new(set) }, None)
            }
            K::The(se) => {
                self.push(None);
                let (set, el) = self.set_expr(se);
                self.pop();
                (N::The { set: Box::new(set) }, Some(el))
            }
            K::Latest(se, by) => {
                self.push(None);
                let (set, el) = self.set_expr(se);
                let by = self.bx(by);
                self.pop();
                (N::Latest { set: Box::new(set), by }, Some(el))
            }
            K::First(se, keys) => {
                self.push(None);
                let (set, el) = self.set_expr(se);
                let order = keys.iter().map(|k| self.sort_key(k)).collect();
                self.pop();
                (N::First { set: Box::new(set), order }, Some(el))
            }
            K::Agg { func, set, body } => match set {
                // check.rs returns Int for a bare aggregate without visiting anything else
                None => (N::Agg { func: agg_fn(*func), set: None, value: None }, None),
                Some(se) => {
                    self.push(None);
                    let (set, _) = self.set_expr(se);
                    let value = body.as_ref().map(|b| self.bx(b));
                    self.pop();
                    (N::Agg { func: agg_fn(*func), set: Some(Box::new(set)), value }, None)
                }
            },
            K::Quant { all, set, body } => {
                self.push(None);
                let (set, _) = self.set_expr(set);
                let body = self.bx(body);
                self.pop();
                (N::Quant { all: *all, set: Box::new(set), body }, None)
            }
            K::RunningSum { value, over, order } => (N::RunningSum { value: self.bx(value), over: over.name.clone(), order: self.bx(order) }, None),
            K::If(c, t, f) => (N::If { cond: self.bx(c), then: self.bx(t), otherwise: self.bx(f) }, None),
            K::List(items) => (N::List { items: items.iter().map(|i| self.expr(i)).collect() }, None),
            K::Binder(se, body) => {
                self.push(None);
                let (set, _) = self.set_expr(se);
                let body = self.bx(body);
                self.pop();
                (N::Binder { set: Box::new(set), body }, None)
            }
        }
    }

    fn is_node(&mut self, span: Span, x: &ast::Expr, name: &ast::Ident) -> ir::Node {
        let row = self.bx(x);
        match row.ty.clone() {
            ir::Type::Ref { entity } => {
                let is_pred = self.m.entity(&entity).and_then(|i| i.predicate(&name.name)).is_some();
                if !is_pred && self.m.entities.contains_key(&name.name) {
                    return ir::Node::TypeTest { value: row, entity: name.name.clone() };
                }
                ir::Node::Pred { entity, name: name.name.clone(), row }
            }
            ir::Type::Enum { name: en } => {
                let value =
                    ir::Expr { ty: ir::Type::Enum { name: en.clone() }, node: ir::Node::EnumValue { enum_name: en, value: name.name.clone() } };
                ir::Node::Binary { op: ir::BinOp::Eq, l: row, r: Box::new(value) }
            }
            _ => {
                self.note(NoteKind::Unresolved, span, format!("'is {}' on a value that is neither an entity row nor an enum", name.name));
                ir::Node::Pred { entity: String::new(), name: name.name.clone(), row }
            }
        }
    }

    fn sort_key(&mut self, k: &ast::SortKey) -> ir::SortKey {
        ir::SortKey { expr: self.expr(&k.expr), desc: k.desc }
    }

    /// Source, alias and filter of a set. Binds the alias (or sets the row) in the innermost scope, like check.rs `set_expr`.
    fn set_expr(&mut self, se: &ast::SetExpr) -> (ir::SetExpr, ir::Type) {
        let (source, elem) = match &se.source.kind {
            ast::ExprKind::Name(id) if self.m.entities.contains_key(&id.name) && self.lookup(&id.name).is_none() => {
                (ir::SetSource::Entity { entity: id.name.clone() }, ir::Type::Ref { entity: id.name.clone() })
            }
            _ => {
                let e = self.expr(&se.source);
                let el = many_inner(e.ty.clone());
                (ir::SetSource::Expr { expr: e }, el)
            }
        };
        match &se.alias {
            Some(a) => self.bind(&a.name, BindKind::Local, elem.clone()),
            None => self.set_row(elem.clone()),
        }
        let filter = se.filter.as_ref().map(|f| self.expr(f));
        (ir::SetExpr { source, alias: se.alias.as_ref().map(|a| a.name.clone()), filter }, elem)
    }

    // ---------- statements ----------

    /// A block of statements at IR path `path`, in its own scope.
    fn block_at(&mut self, path: String, b: &ast::Block) -> Vec<ir::Stmt> {
        self.push(None);
        let out = self.stmts_at(path, &b.stmts);
        self.pop();
        out
    }

    /// Statements at IR path `path`; statement `i` is recorded as `path[i]` so rules on the IR can point at its line.
    fn stmts_at(&mut self, path: String, v: &[ast::Stmt]) -> Vec<ir::Stmt> {
        let saved = (std::mem::replace(&mut self.list, path), std::mem::take(&mut self.here));
        let mut out = Vec::new();
        for (i, s) in v.iter().enumerate() {
            self.here = format!("{}[{i}]", self.list);
            self.mark(self.here.clone(), stmt_span(s));
            self.stmt(s, &mut out);
        }
        (self.list, self.here) = saved;
        out
    }

    fn let_(&mut self, l: &ast::Let) -> ir::Let {
        let value = self.expr(&l.value);
        self.bind(&l.name.name, BindKind::Local, value.ty.clone());
        ir::Let { name: l.name.name.clone(), value, code: l.code.as_ref().map(|c| c.name.clone()) }
    }

    fn field_assigns(&mut self, fields: &[ast::FieldAssign], allow_spread: bool) -> Vec<(String, ir::Expr)> {
        let mut out = Vec::new();
        for fa in fields {
            match fa {
                ast::FieldAssign::Named { name, value: Some(v) } => out.push((name.name.clone(), self.expr(v))),
                ast::FieldAssign::Named { name, value: None } => out.push((name.name.clone(), self.shorthand(name))),
                ast::FieldAssign::Spread(v) => {
                    let base = self.expr(v);
                    match (&base.ty, allow_spread) {
                        (ir::Type::Record { name }, true) if self.m.records.contains_key(name) => {
                            if let Some(rec) = self.m.records.get(name).copied() {
                                for rf in &rec.fields {
                                    let ty = self.type_expr(&rf.ty);
                                    out.push((
                                        rf.name.name.clone(),
                                        ir::Expr {
                                            ty,
                                            node: ir::Node::Field { base: Box::new(base.clone()), entity: None, field: rf.name.name.clone() },
                                        },
                                    ));
                                }
                            }
                        }
                        _ => self.note(
                            NoteKind::Proposal,
                            v.span,
                            "IR change proposal: Spread (only a record value can be spread into named fields; this spread was skipped)",
                        ),
                    }
                }
            }
        }
        out
    }

    /// `set` target: a field path, or a bare field of the row being updated.
    fn assign(&mut self, a: &ast::Assign, row: Option<(&ir::Type, Option<&str>)>) -> ir::Assign {
        let target = match (&a.target.kind, row) {
            (ast::ExprKind::Name(f), Some((rt, alias))) => {
                let entity = entity_of_type(rt).unwrap_or_default().to_string();
                let ty = self.field_of(rt, &f.name).map(|(_, t)| t).unwrap_or(ir::Type::Unknown);
                let node = match alias {
                    Some(al) => ir::Node::Field {
                        base: Box::new(ir::Expr { ty: rt.clone(), node: ir::Node::Local { name: al.to_string() } }),
                        entity: Some(entity),
                        field: f.name.clone(),
                    },
                    None => ir::Node::RowField { entity, field: f.name.clone() },
                };
                ir::Expr { ty, node }
            }
            _ => self.expr(&a.target),
        };
        let value = self.expr(&a.value);
        ir::Assign {
            target,
            op: match a.op {
                ast::AssignOp::Set => ir::AssignOp::Set,
                ast::AssignOp::Add => ir::AssignOp::Add,
                ast::AssignOp::Sub => ir::AssignOp::Sub,
            },
            value,
        }
    }

    /// `where` of a statement's set, so a rule that fires inside it points at that line.
    fn mark_filter(&mut self, here: &str, se: &ast::SetExpr) {
        if let Some(f) = &se.filter {
            self.mark(format!("{here}.filter"), f.span);
        }
    }

    fn stmt(&mut self, s: &ast::Stmt, out: &mut Vec<ir::Stmt>) {
        use ast::Stmt as S;
        let here = self.here.clone();
        match s {
            S::Let(l) => out.push(ir::Stmt::Let(self.let_(l))),
            S::Insert { entity, from, fields, bind, .. } => {
                self.push(None);
                let from = from.as_ref().map(|se| self.set_expr(se).0);
                let values = self.field_assigns(fields, true);
                self.pop();
                if let Some(b) = bind {
                    self.bind(&b.name, BindKind::Local, ir::Type::Ref { entity: entity.name.clone() });
                }
                out.push(ir::Stmt::Insert { entity: entity.name.clone(), from, values, bind: bind.as_ref().map(|b| b.name.clone()) });
            }
            S::Upsert { entity, keys, fields, bind, .. } => {
                let values = self.field_assigns(fields, true);
                if let Some(b) = bind {
                    self.bind(&b.name, BindKind::Local, ir::Type::Ref { entity: entity.name.clone() });
                }
                out.push(ir::Stmt::Upsert {
                    entity: entity.name.clone(),
                    keys: keys.iter().map(|k| k.name.clone()).collect(),
                    values,
                    bind: bind.as_ref().map(|b| b.name.clone()),
                });
            }
            S::Update { target, via, assigns, .. } => {
                self.mark_filter(&here, target);
                self.push(None);
                let (set, row) = self.set_expr(target);
                let via = via.as_ref().map(|(path, alias)| {
                    let p = self.expr(path);
                    let el = many_inner(p.ty.clone());
                    self.bind(&alias.name, BindKind::Local, el);
                    ir::UpdateVia { path: p, alias: alias.name.clone() }
                });
                let alias = target.alias.as_ref().map(|a| a.name.as_str());
                let assigns = assigns.iter().map(|a| self.assign(a, Some((&row, alias)))).collect();
                self.pop();
                out.push(ir::Stmt::Update { target: set, via, assigns });
            }
            S::Delete { target, .. } | S::Purge { target, .. } => {
                self.mark_filter(&here, target);
                self.push(None);
                let (set, _) = self.set_expr(target);
                self.pop();
                out.push(if matches!(s, S::Delete { .. }) { ir::Stmt::Delete { target: set } } else { ir::Stmt::Purge { target: set } });
            }
            S::Erase { target, .. } => {
                self.mark(format!("{here}.target"), target.span);
                out.push(ir::Stmt::Erase { target: self.expr(target) });
            }
            S::Toggle { entity, fields, .. } => {
                let values = self.field_assigns(fields, true);
                out.push(ir::Stmt::Toggle { entity: entity.name.clone(), values });
            }
            S::Set { assigns, .. } => {
                let assigns = assigns.iter().map(|a| self.assign(a, None)).collect();
                out.push(ir::Stmt::Set { assigns });
            }
            S::When { cond, body, .. } => {
                self.mark(format!("{here}.cond"), cond.span);
                let cond = self.expr(cond);
                out.push(ir::Stmt::When { cond, body: self.block_at(format!("{here}.body"), body) });
            }
            S::Each { source, body, .. } => {
                self.mark_filter(&here, source);
                self.push(None);
                let (set, _) = self.set_expr(source);
                let body = self.block_at(format!("{here}.body"), body);
                self.pop();
                out.push(ir::Stmt::Each { source: set, body });
            }
            S::Effect { call, bind, into, on_failure, .. } => {
                let c = self.call_expr(call);
                let into = into.as_ref().map(|i| self.expr(i));
                if let Some(b) = bind {
                    self.bind(&b.name, BindKind::Local, ir::Type::Ext { name: call.callee.text() });
                }
                let on_failure = on_failure.as_ref().map(|b| self.block_at(format!("{here}.on_failure"), b));
                out.push(ir::Stmt::Effect { call: c, bind: bind.as_ref().map(|b| b.name.clone()), into, on_failure });
            }
            S::Reserve { op, what, of, duration, code, .. } => out.push(ir::Stmt::Reserve {
                op: op.name.clone(),
                what: what.name.clone(),
                of: self.expr(of),
                seconds: dur_opt(duration),
                code: code.as_ref().map(|c| c.name.clone()),
            }),
            S::AtRun { at, intent, args, .. } => {
                let at = self.expr(at);
                out.push(ir::Stmt::AtRun { at, intent: intent.name.clone(), args: self.args(args, false) });
            }
            S::Notify(n) => {
                self.mark(format!("{here}.to"), n.to.span);
                out.push(ir::Stmt::Notify(self.notify(n)));
            }
            S::ExportPersonalData { of, to, notify, .. } => {
                let of = self.expr(of);
                out.push(ir::Stmt::ExportPersonalData { of, to: to.name.clone(), notify: notify.as_ref().map(|n| self.expr(n)) });
            }
        }
    }

    fn notify(&mut self, n: &ast::Notify) -> ir::Notify {
        self.push(None);
        let (to, _) = self.set_expr(&n.to);
        let via = self.call_target(&n.via);
        let fields = self.field_assigns(&n.fields, true);
        self.pop();
        ir::Notify { to, via, fields, category: n.category.as_ref().map(|c| c.name.clone()), digest_seconds: dur_opt(&n.digest) }
    }

    // ---------- intents ----------

    fn rate_limits(&self, limits: &[ast::Rate]) -> Vec<ir::RateLimit> {
        limits
            .iter()
            .map(|r| ir::RateLimit {
                n: r.n,
                per_seconds: dur_secs(&r.per),
                key: match &r.key {
                    ast::RateKey::Actor => ir::RateKey::Actor,
                    ast::RateKey::Client => ir::RateKey::Client,
                    ast::RateKey::Path(q) => ir::RateKey::Path { path: q.parts.iter().map(|p| p.name.clone()).collect() },
                },
            })
            .collect()
    }

    fn policy(&mut self, a: &ast::Allow) -> ir::Policy {
        ir::Policy { cond: self.expr(&a.cond), code: a.code.as_ref().map(|c| c.name.clone()) }
    }

    fn require(&mut self, r: &ast::Require) -> ir::Require {
        ir::Require { when: r.when.as_ref().map(|w| self.expr(w)), cond: self.expr(&r.cond), code: r.code.name.clone() }
    }

    fn selection(&mut self, row: &ir::Type, sel: &ast::Selection) -> ir::Selection {
        let mut items = Vec::new();
        for it in &sel.items {
            let value = it.value.as_ref().map(|v| self.expr(v));
            let sub = match &it.sub {
                Some(sub) => {
                    let t = match &value {
                        Some(v) => v.ty.clone(),
                        None => self.field_of(row, &it.name.name).map(|(_, t)| t).unwrap_or(ir::Type::Unknown),
                    };
                    let elem = many_inner(t);
                    self.push(None);
                    let s = self.selection(&elem, sub);
                    self.pop();
                    Some(s)
                }
                None => None,
            };
            items.push(ir::SelItem { name: it.name.name.clone(), value, sub });
        }
        ir::Selection { items }
    }

    fn sort(&mut self, s: &ast::Sort) -> ir::Sort {
        match s {
            ast::Sort::Keys(keys) => ir::Sort::Keys { keys: keys.iter().map(|k| self.sort_key(k)).collect() },
            ast::Sort::ByParam { param, cases } => ir::Sort::ByParam {
                param: param.name.clone(),
                cases: cases.iter().map(|(k, keys)| (k.name.clone(), keys.iter().map(|x| self.sort_key(x)).collect())).collect(),
            },
        }
    }

    fn query(&mut self, q: &ast::QueryDecl) -> ir::Query {
        self.search_aliases.clear();
        self.push(None);
        let params = self.params(&q.params);
        let base = self.decl.clone();
        for (i, l) in q.lets.iter().enumerate() {
            self.mark(format!("{base}.lets[{i}]"), l.value.span);
        }
        let lets: Vec<ir::Let> = q.lets.iter().map(|l| self.let_(l)).collect();
        let mut fetches = Vec::new();
        for (call, name) in &q.fetches {
            let c = self.call_expr(call);
            self.bind(&name.name, BindKind::Local, ir::Type::Ext { name: call.callee.text() });
            fetches.push((c, name.name.clone()));
        }
        let (source, row) = match &q.from {
            Some(ast::FromClause::Entity { entity, alias }) => {
                let t = ir::Type::Ref { entity: entity.name.clone() };
                self.bind(&alias.name, BindKind::Local, t.clone());
                (Some(ir::QuerySource::Entity { entity: entity.name.clone(), alias: alias.name.clone() }), t)
            }
            Some(ast::FromClause::Param(p)) => {
                let t = self.lookup(&p.name).map(|(_, t)| t).unwrap_or(ir::Type::Unknown);
                let entity = entity_of_type(&t).unwrap_or_default().to_string();
                (Some(ir::QuerySource::Param { param: p.name.clone(), entity }), t)
            }
            Some(ast::FromClause::Call { call, alias }) => {
                let search = call.callee.parts.first().and_then(|p| self.m.searches.get(&p.name).copied());
                let t = search.map(|s| ir::Type::Ref { entity: s.entity.name.clone() }).unwrap_or(ir::Type::Unknown);
                self.bind(&alias.name, BindKind::Local, t.clone());
                match (search, call.args.first()) {
                    (Some(sd), Some(arg)) => {
                        self.search_aliases.push(alias.name.clone());
                        let query = Box::new(self.expr(&arg.value));
                        (Some(ir::QuerySource::Search { search: sd.name.name.clone(), query, alias: alias.name.clone() }), t)
                    }
                    _ => {
                        let c = self.call_expr(call);
                        (Some(ir::QuerySource::Call { call: c, alias: alias.name.clone() }), t)
                    }
                }
            }
            None => (None, ir::Type::Unknown),
        };
        if let Some(a) = &q.allow {
            self.mark(format!("{base}.allow"), a.cond.span);
        }
        let allow = q.allow.as_ref().map(|a| self.policy(a));
        if let Some(f) = &q.filter {
            self.mark(format!("{base}.filter"), f.span);
        }
        let filter = q.filter.as_ref().map(|f| self.expr(f));
        let group_by = q.group_by.iter().map(|g| self.expr(g)).collect();
        match &q.sort {
            Some(ast::Sort::ByParam { param, .. }) => self.mark(format!("{base}.sort"), param.span),
            Some(ast::Sort::Keys(keys)) => {
                if let Some(k) = keys.first() {
                    self.mark(format!("{base}.sort"), k.expr.span);
                }
            }
            None => {}
        }
        let sort = q.sort.as_ref().map(|s| self.sort(s));
        let sel_row = if q.group_by.is_empty() { row } else { ir::Type::Unknown };
        self.push(Some(sel_row.clone()));
        let select = self.selection(&sel_row, &q.select);
        self.pop();
        for (i, t) in q.touches.iter().enumerate() {
            self.mark(format!("{base}.touches[{i}]"), t.span);
        }
        let touches = q.touches.iter().map(|t| self.expr(t)).collect();
        self.pop();
        ir::Query {
            internal: q.internal,
            drafts: q.drafts,
            cross_tenant: q.cross_tenant,
            params,
            cache: q.cached.as_ref().map(|(d, per)| ir::Cache { seconds: dur_secs(d), per: per.as_ref().map(|p| p.name.clone()) }),
            rate_limits: self.rate_limits(&q.limits),
            lets,
            allow,
            fetches,
            source,
            filter,
            group_by,
            sort,
            page: q.page.as_ref().map(|p| ir::Page { size: p.size, offset_max_page: p.offset_max_page }),
            plan: q.plan.as_ref().map(|p| p.name.clone()),
            consistency: q.consistency.as_ref().map(|c| c.name.clone()),
            select,
            touches,
            output: self.query_shape(q),
        }
    }

    fn command(&mut self, c: &ast::CommandDecl) -> ir::Command {
        self.push(None);
        let params = self.params(&c.params);
        let base = self.decl.clone();
        let idempotency = match &c.idempotent {
            None => ir::Idempotency::None,
            Some(None) => ir::Idempotency::CallerKey,
            Some(Some(k)) => ir::Idempotency::Derived { key: self.expr(k) },
        };
        for (i, l) in c.lets.iter().enumerate() {
            self.mark(format!("{base}.lets[{i}]"), l.value.span);
        }
        let lets: Vec<ir::Let> = c.lets.iter().map(|l| self.let_(l)).collect();
        if let Some(a) = &c.allow {
            self.mark(format!("{base}.allow"), a.cond.span);
        }
        let allow = c.allow.as_ref().map(|a| self.policy(a));
        for (i, r) in c.requires.iter().enumerate() {
            self.mark(format!("{base}.requires[{i}]"), r.span);
            self.mark(format!("{base}.requires[{i}].cond"), r.cond.span);
            if let Some(w) = &r.when {
                self.mark(format!("{base}.requires[{i}].when"), w.span);
            }
        }
        let requires = c.requires.iter().map(|r| self.require(r)).collect();
        // bindings made in `do` (insert ... as x) stay visible to emit and returns
        let body = c.body.as_ref().map(|b| self.stmts_at(format!("{base}.body"), &b.stmts)).unwrap_or_default();
        for (i, e) in c.emits.iter().enumerate() {
            self.mark(format!("{base}.emits[{i}]"), e.span);
        }
        let emits = c.emits.iter().map(|e| self.emit(e)).collect();
        if let Some((value, _)) = &c.returns {
            self.mark(format!("{base}.returns"), value.span);
        }
        let returns = c.returns.as_ref().map(|(value, sel)| {
            let value = self.expr(value);
            let select = sel.as_ref().map(|s| self.selection(&value.ty.clone(), s));
            ir::Returns { value, select }
        });
        self.pop();
        ir::Command {
            internal: c.internal,
            cross_tenant: c.cross_tenant,
            params,
            idempotency,
            audited: c.audited,
            rate_limits: self.rate_limits(&c.limits),
            lets,
            allow,
            requires,
            body,
            emits,
            returns,
            output: self.command_shape(c),
        }
    }

    fn emit(&mut self, e: &ast::Emit) -> ir::Emit {
        let fields = self.field_assigns(&e.fields, false);
        let to = e.to.as_ref().map(|t| ir::EmitTarget {
            broker: t.broker.name.clone(),
            topic: t.topic.clone(),
            key: t.key.as_ref().map(|k| self.expr(k)),
        });
        ir::Emit { event: e.event.name.clone(), fields, to }
    }

    // ---------- reactions ----------

    fn set_opt(&mut self, se: &Option<ast::SetExpr>) -> Option<ir::SetExpr> {
        se.as_ref().map(|s| self.set_expr(s).0)
    }

    fn reaction(&mut self, d: &ast::Decl) -> Option<ir::Reaction> {
        Some(match d {
            ast::Decl::OnEvent(o) => {
                self.push(None);
                self.bind(&o.binding.name, BindKind::Local, ir::Type::Event { name: o.event.name.clone() });
                let when = o.when.as_ref().map(|w| self.expr(w));
                let body = self.block_at(format!("{}.body", self.decl), &o.body);
                self.pop();
                ir::Reaction::Event { cross_tenant: o.cross_tenant, event: o.event.name.clone(), binding: o.binding.name.clone(), when, body }
            }
            ast::Decl::Schedule(s) => {
                self.push(None);
                let for_each = self.set_opt(&s.for_each);
                let body = self.stmts_at(format!("{}.body", self.decl), &s.body);
                self.pop();
                ir::Reaction::Schedule(ir::Schedule {
                    cross_tenant: s.cross_tenant,
                    name: s.name.name.clone(),
                    every: match &s.every {
                        ast::Every::Day => ir::Every::Day,
                        ast::Every::Week(d) => ir::Every::Week { day: d.name.clone() },
                        ast::Every::MonthDay(n) => ir::Every::MonthDay { day: *n },
                        ast::Every::Interval(d) => ir::Every::Interval { seconds: dur_secs(d) },
                    },
                    at: s.at,
                    tz: s.tz.clone(),
                    catch_up_once: s.catch_up_once,
                    for_each,
                    body,
                })
            }
            ast::Decl::Retain(r) => {
                self.push(None);
                let notify = r.notify.as_ref().map(|(path, before, via)| ir::RetainNotify {
                    path: path.parts.iter().map(|p| p.name.clone()).collect(),
                    before: dur(before),
                    via: self.call_target(via),
                });
                self.pop();
                ir::Reaction::Retain {
                    entity: r.entity.name.clone(),
                    keep: dur(&r.keep),
                    after: r.after.parts.iter().map(|p| p.name.clone()).collect(),
                    anonymize: r.anonymize,
                    notify,
                }
            }
            ast::Decl::Rule(r) => {
                self.push(None);
                self.bind(&r.alias.name, BindKind::Local, ir::Type::Ref { entity: r.entity.name.clone() });
                let when = self.expr(&r.when);
                let body = self.block_at(format!("{}.body", self.decl), &r.body);
                self.pop();
                ir::Reaction::Rule {
                    cross_tenant: r.cross_tenant,
                    name: r.name.name.clone(),
                    entity: r.entity.name.clone(),
                    alias: r.alias.name.clone(),
                    when,
                    body,
                }
            }
            ast::Decl::Webhook(w) => {
                self.push(None);
                let dp = self.decl.clone();
                self.mark(format!("{dp}.via"), w.via.name.span);
                for (j, a) in w.via.args.iter().flatten().enumerate() {
                    self.mark(format!("{dp}.via.args[{j}]"), a.name.as_ref().map(|n| n.span).unwrap_or(a.value.span));
                }
                let via = self.call_target(&w.via);
                self.pop();
                let mut handlers = Vec::new();
                for (j, h) in w.handlers.iter().enumerate() {
                    self.mark(format!("{dp}.handlers[{j}]"), h.event_span);
                    self.push(None);
                    let ty = h.ty.as_ref().map(|t| self.declared_type(t));
                    self.bind(
                        &h.binding.name,
                        BindKind::Local,
                        ty.as_ref().map(strip_refine).unwrap_or(ir::Type::Json { schema: None, validated_by: None }),
                    );
                    let body = self.block_at(format!("{dp}.handlers[{j}].body"), &h.body);
                    self.pop();
                    handlers.push(ir::WebhookOn { cross_tenant: h.cross_tenant, event: h.event.clone(), binding: h.binding.name.clone(), ty, body });
                }
                ir::Reaction::Webhook(ir::Webhook { name: w.name.name.clone(), via, handlers })
            }
            ast::Decl::Consume(c) => {
                self.note(
                    NoteKind::Info,
                    c.span,
                    format!("consume {}: check.rs does not visit consume bodies, so names and types are lowered unchecked", c.name.name),
                );
                self.push(None);
                let body = self.block_at(format!("{}.body", self.decl), &c.body);
                self.pop();
                ir::Reaction::Consume {
                    cross_tenant: c.cross_tenant,
                    name: c.name.name.clone(),
                    broker: c.broker.name.clone(),
                    topic: c.topic.clone(),
                    key: c.key.name.clone(),
                    dedupe: c.dedupe.as_ref().map(|d| d.name.clone()),
                    body,
                }
            }
            _ => return None,
        })
    }

    // ---------- L3 forms ----------

    fn form(&mut self, d: &ast::Decl) -> Option<ir::Form> {
        use ir::Form as F;
        Some(match d {
            ast::Decl::Subscribe(s) => {
                self.push(None);
                let params = self.params(&s.params);
                let allow = self.policy(&s.allow);
                let (source, row) = match &s.from {
                    ast::FromClause::Entity { entity, alias } => {
                        let t = ir::Type::Ref { entity: entity.name.clone() };
                        self.bind(&alias.name, BindKind::Local, t.clone());
                        (ir::QuerySource::Entity { entity: entity.name.clone(), alias: alias.name.clone() }, t)
                    }
                    ast::FromClause::Param(p) => {
                        let t = self.lookup(&p.name).map(|(_, t)| t).unwrap_or(ir::Type::Unknown);
                        (ir::QuerySource::Param { param: p.name.clone(), entity: entity_of_type(&t).unwrap_or_default().to_string() }, t)
                    }
                    ast::FromClause::Call { call, alias } => {
                        let c = self.call_expr(call);
                        self.bind(&alias.name, BindKind::Local, ir::Type::Unknown);
                        (ir::QuerySource::Call { call: c, alias: alias.name.clone() }, ir::Type::Unknown)
                    }
                };
                let filter = s.filter.as_ref().map(|f| self.expr(f));
                let select = self.selection(&row, &s.select);
                self.pop();
                let output = self.subscribe_shape(s);
                F::Subscribe(ir::Subscribe { name: s.name.name.clone(), params, allow, source, filter, select, output })
            }
            ast::Decl::Projection(p) => {
                self.note(
                    NoteKind::Info,
                    p.span,
                    format!("projection {}: check.rs does not visit projections, so names and types are lowered unchecked", p.name.name),
                );
                let mut handlers = Vec::new();
                for (event, binding, stmt) in &p.handlers {
                    self.push(None);
                    self.bind(&binding.name, BindKind::Local, ir::Type::Event { name: event.name.clone() });
                    let mut v = Vec::new();
                    self.here = self.decl.clone();
                    self.stmt(stmt, &mut v);
                    self.pop();
                    match v.len() {
                        1 => {
                            if let Some(st) = v.pop() {
                                handlers.push((event.name.clone(), binding.name.clone(), st));
                            }
                        }
                        _ => self.note(NoteKind::Approx, p.span, "projection handler lowered to several statements; handler skipped"),
                    }
                }
                F::Projection(ir::Projection {
                    name: p.name.name.clone(),
                    events: p.events.iter().map(|e| e.name.clone()).collect(),
                    key: p.key.name.clone(),
                    handlers,
                })
            }
            ast::Decl::Search(s) => F::Search(ir::Search {
                name: s.name.name.clone(),
                entity: s.entity.name.clone(),
                fields: s.fields.iter().map(|(f, w)| (f.name.clone(), w.as_ref().map(|w| w.name.clone()))).collect(),
                language: s.language.as_ref().map(|l| l.name.clone()),
            }),
            ast::Decl::Job(j) => {
                self.push(None);
                let params = self.params(&j.params);
                let allow = self.policy(&j.allow);
                let progress = self.set_opt(&j.progress);
                let body = j.body.as_ref().map(|b| self.block_at(format!("{}.body", self.decl), b));
                let notify = j.notify.as_ref().map(|(e, via)| (self.expr(e), self.call_target(via)));
                self.pop();
                F::Job(ir::Job {
                    name: j.name.name.clone(),
                    params,
                    allow,
                    progress,
                    produce: j.produce.as_ref().map(|(format, store, bucket, exp)| ir::JobProduce {
                        format: format.name.clone(),
                        store: store.name.clone(),
                        bucket: bucket.clone(),
                        expires_seconds: dur_opt(exp),
                    }),
                    body,
                    notify,
                })
            }
            ast::Decl::Verification(v) => {
                self.push(None);
                let target = self.declared_type(&v.target);
                self.bind("target", BindKind::Local, strip_refine(&target));
                let target_where = v.target_where.as_ref().map(|w| self.expr(w));
                let deliver = self.call_target(&v.deliver);
                let (a, b, blk) = &v.on_verified;
                self.bind(&a.name, BindKind::Local, ir::Type::Ref { entity: v.subject.name.clone() });
                self.bind(&b.name, BindKind::Local, strip_refine(&target));
                let body = self.block_at(format!("{}.on_verified", self.decl), blk);
                self.pop();
                F::Verification(ir::Verification {
                    name: v.name.name.clone(),
                    subject: v.subject.name.clone(),
                    target,
                    target_where,
                    alnum: v.alnum,
                    length: v.length,
                    ttl_seconds: dur_secs(&v.ttl),
                    attempts: v.attempts,
                    resend_after_seconds: dur_opt(&v.resend_after),
                    deliver,
                    on_verified: (a.name.clone(), b.name.clone(), body),
                })
            }
            ast::Decl::GrantLink(g) => {
                self.push(None);
                let scope = self.params(&g.scope);
                let redeem_params = self.params(&g.redeem_params);
                let issued_by = self.expr(&g.issued_by);
                let to = g.to.as_ref().map(|t| self.expr(t));
                let actor = self.actor_type();
                self.bind("holder", BindKind::Local, actor);
                let grants = g.grants.as_ref().map(|(rel, role)| (self.expr(rel), role.name.clone()));
                let requires = g.requires.iter().map(|r| self.require(r)).collect();
                let on_redeem = g.on_redeem.as_ref().map(|b| self.block_at(format!("{}.on_redeem", self.decl), b));
                self.pop();
                F::GrantLink(ir::GrantLink {
                    name: g.name.name.clone(),
                    grants,
                    scope,
                    redeem_params,
                    issued_by,
                    to,
                    expires_seconds: dur_secs(&g.expires),
                    uses: g.uses,
                    requires,
                    on_redeem,
                })
            }
            ast::Decl::Approval(a) => {
                self.push(None);
                self.bind(&a.alias.name, BindKind::Local, ir::Type::Ref { entity: a.entity.name.clone() });
                self.push(None);
                let (approvers, _) = self.set_expr(&a.approvers);
                self.pop();
                let requested_by = a.requested_by.as_ref().map(|r| self.expr(r));
                let on_approved = self.block_at(format!("{}.on_approved", self.decl), &a.on_approved);
                let on_rejected = self.block_at(format!("{}.on_rejected", self.decl), &a.on_rejected);
                self.pop();
                F::Approval(ir::Approval {
                    name: a.name.name.clone(),
                    entity: a.entity.name.clone(),
                    alias: a.alias.name.clone(),
                    approvers,
                    requested_by,
                    required: a.required,
                    no_self: a.no_self,
                    on_approved,
                    on_rejected,
                    expires_seconds: dur_opt(&a.expires),
                })
            }
            ast::Decl::OutboundWebhooks(o) => {
                self.push(None);
                self.bind(&o.alias.name, BindKind::Local, ir::Type::Ref { entity: o.entity.name.clone() });
                let first = o.events.first().map(|e| e.name.clone()).unwrap_or_default();
                self.bind("event", BindKind::Local, ir::Type::Event { name: first });
                let filter = o.filter.as_ref().map(|f| self.expr(f));
                self.pop();
                F::OutboundWebhooks(ir::OutboundWebhooks {
                    entity: o.entity.name.clone(),
                    alias: o.alias.name.clone(),
                    events: o.events.iter().map(|e| e.name.clone()).collect(),
                    filter,
                    sign: o.sign.name.clone(),
                    retry: o.retry,
                    over_seconds: dur_secs(&o.over),
                    disable_after_seconds: dur_opt(&o.disable_after),
                })
            }
            ast::Decl::Consent(c) => {
                F::Consent(ir::Consent { name: c.name.name.clone(), version: c.version, intents: c.intents.iter().map(|i| i.name.clone()).collect() })
            }
            ast::Decl::Impersonate(i) => {
                self.push(None);
                let by = self.expr(&i.by);
                self.pop();
                F::Impersonate(ir::Impersonate { entity: i.entity.name.clone(), by, ttl_seconds: dur_secs(&i.ttl) })
            }
            ast::Decl::Migration(m) => {
                let body = self.block_at(format!("{}.body", self.decl), &m.body);
                F::Migration(ir::Migration { name: m.name.name.clone(), body })
            }
            ast::Decl::Upcast(u) => {
                self.note(NoteKind::Info, u.span, "upcast: check.rs does not visit this form, so names and types are lowered unchecked");
                self.push(None);
                let with = self.field_assigns(&u.with, false);
                self.pop();
                F::Upcast(ir::Upcast { event: u.event.name.clone(), from: u.from, to: u.to, with })
            }
            _ => return None,
        })
    }

    // ---------- output shapes (port of aip-pg/src/shape.rs) ----------

    fn query_shape(&self, q: &ast::QueryDecl) -> ir::Shape {
        self.rows_shape(&q.params, q.from.as_ref(), &q.group_by, &q.select, false)
    }

    /// One update of a subscription is the list of its rows, as a list query would answer.
    fn subscribe_shape(&self, s: &ast::SubscribeDecl) -> ir::Shape {
        self.rows_shape(&s.params, Some(&s.from), &[], &s.select, true)
    }

    fn rows_shape(
        &self,
        params: &[ast::Param],
        from: Option<&ast::FromClause>,
        group_by: &[ast::Expr],
        select: &ast::Selection,
        always_list: bool,
    ) -> ir::Shape {
        let a = self.a;
        let entity = match from {
            Some(ast::FromClause::Entity { entity, .. }) => Some(entity.name.clone()),
            Some(ast::FromClause::Param(p)) => {
                params.iter().find(|x| x.name.name == p.name).and_then(|x| entity_of_type(&self.type_expr(&x.ty)).map(str::to_string))
            }
            // aip-pg yields no output shape for search-backed queries; the search's entity is the row here
            Some(ast::FromClause::Call { call, .. }) => {
                call.callee.parts.first().and_then(|p| a.model.searches.get(&p.name)).map(|s| s.entity.name.clone())
            }
            None => None,
        };
        let Some(entity) = entity else { return ir::Shape::None };
        let obj = if group_by.is_empty() {
            sel_shape(a, &entity, select)
        } else {
            let fields = select
                .items
                .iter()
                .map(|it| {
                    let s = match &it.value {
                        Some(e) => ty_shape(&a.ty_of(e), false),
                        None => ir::Shape::Value { ty: ir::Type::Unknown, nullable: false },
                    };
                    (it.name.name.clone(), s)
                })
                .collect();
            ir::Shape::Object { fields, nullable: false }
        };
        if !always_list && matches!(from, Some(ast::FromClause::Param(_))) { obj } else { ir::Shape::List { nullable: false, of: Box::new(obj) } }
    }

    fn command_shape(&self, c: &ast::CommandDecl) -> ir::Shape {
        let a = self.a;
        match &c.returns {
            Some((e, Some(sel))) => match a.ty_of(e) {
                Ty::Entity(ent) => sel_shape(a, &ent, sel),
                _ => ir::Shape::Value { ty: ir::Type::Unknown, nullable: false },
            },
            Some((e, None)) => ty_shape(&a.ty_of(e), false),
            None => ir::Shape::None,
        }
    }
}

fn stmt_span(s: &ast::Stmt) -> Span {
    use ast::Stmt as S;
    match s {
        S::Let(l) => l.value.span,
        S::Notify(n) => n.span,
        S::Insert { span, .. }
        | S::Upsert { span, .. }
        | S::Update { span, .. }
        | S::Delete { span, .. }
        | S::Purge { span, .. }
        | S::Erase { span, .. }
        | S::Toggle { span, .. }
        | S::Set { span, .. }
        | S::When { span, .. }
        | S::Each { span, .. }
        | S::Effect { span, .. }
        | S::Reserve { span, .. }
        | S::AtRun { span, .. }
        | S::ExportPersonalData { span, .. } => *span,
    }
}

fn constraint_span(c: &ast::Constraint) -> Span {
    match c {
        ast::Constraint::Unique { span, .. }
        | ast::Constraint::Cardinality { span, .. }
        | ast::Constraint::NoOverlap { span, .. }
        | ast::Constraint::Capacity { span, .. }
        | ast::Constraint::Invariant { span, .. } => *span,
    }
}

fn decl_span(d: &ast::Decl) -> Span {
    match d {
        ast::Decl::Use(q) => q.span,
        ast::Decl::Actor(x) => x.span,
        ast::Decl::Enum(x) => x.span,
        ast::Decl::Record(x) => x.span,
        ast::Decl::Entity(x) => x.span,
        ast::Decl::Relation(x) => x.span,
        ast::Decl::Fn(x) => x.span,
        ast::Decl::Event(x) => x.span,
        ast::Decl::Upcast(x) => x.span,
        ast::Decl::Query(x) => x.span,
        ast::Decl::Command(x) => x.span,
        ast::Decl::Subscribe(x) => x.span,
        ast::Decl::Webhook(x) => x.span,
        ast::Decl::Consume(x) => x.span,
        ast::Decl::OnEvent(x) => x.span,
        ast::Decl::Schedule(x) => x.span,
        ast::Decl::Retain(x) => x.span,
        ast::Decl::Rule(x) => x.span,
        ast::Decl::Projection(x) => x.span,
        ast::Decl::Search(x) => x.span,
        ast::Decl::Job(x) => x.span,
        ast::Decl::Verification(x) => x.span,
        ast::Decl::GrantLink(x) => x.span,
        ast::Decl::Approval(x) => x.span,
        ast::Decl::Expose(x) => x.span,
        ast::Decl::OutboundWebhooks(x) => x.span,
        ast::Decl::Consent(x) => x.span,
        ast::Decl::Config(x) => x.span,
        ast::Decl::Flag(x) => x.span,
        ast::Decl::Impersonate(x) => x.span,
        ast::Decl::Migration(x) => x.span,
        ast::Decl::Removed(x) => x.span,
    }
}

fn bin_op(op: ast::BinOp) -> ir::BinOp {
    match op {
        ast::BinOp::Or => ir::BinOp::Or,
        ast::BinOp::And => ir::BinOp::And,
        ast::BinOp::Eq => ir::BinOp::Eq,
        ast::BinOp::Ne => ir::BinOp::Ne,
        ast::BinOp::Lt => ir::BinOp::Lt,
        ast::BinOp::Le => ir::BinOp::Le,
        ast::BinOp::Gt => ir::BinOp::Gt,
        ast::BinOp::Ge => ir::BinOp::Ge,
        ast::BinOp::Add => ir::BinOp::Add,
        ast::BinOp::Sub => ir::BinOp::Sub,
        ast::BinOp::Mul => ir::BinOp::Mul,
        ast::BinOp::Div => ir::BinOp::Div,
    }
}

fn agg_fn(f: ast::AggFn) -> ir::AggFn {
    match f {
        ast::AggFn::Count => ir::AggFn::Count,
        ast::AggFn::Sum => ir::AggFn::Sum,
        ast::AggFn::Min => ir::AggFn::Min,
        ast::AggFn::Max => ir::AggFn::Max,
        ast::AggFn::Avg => ir::AggFn::Avg,
    }
}

// ---------- shapes ----------

fn with_nullable(s: ir::Shape, yes: bool) -> ir::Shape {
    match s {
        ir::Shape::Value { ty, nullable } => ir::Shape::Value { ty, nullable: nullable || yes },
        ir::Shape::Object { fields, nullable } => ir::Shape::Object { fields, nullable: nullable || yes },
        ir::Shape::List { of, nullable } => ir::Shape::List { of, nullable: nullable || yes },
        other => other,
    }
}

fn ty_shape(t: &Ty, nullable: bool) -> ir::Shape {
    match t {
        Ty::Coll(inner) => ir::Shape::List { nullable: false, of: Box::new(ty_shape(inner, false)) },
        other => ir::Shape::Value { ty: ty_to_ir(other), nullable },
    }
}

fn sel_shape(a: &Analysis<'_>, entity: &str, sel: &ast::Selection) -> ir::Shape {
    let info = a.model.entity(entity);
    let mut fields = Vec::new();
    for it in &sel.items {
        let s = match &it.value {
            Some(e) => with_nullable(derived_shape(a, &a.ty_of(e), it.sub.as_ref()), may_be_null(a, e)),
            None => match info.and_then(|i| i.field(&it.name.name)) {
                Some(f) => field_shape(a, f, it.sub.as_ref()),
                None => ir::Shape::Value { ty: ir::Type::Unknown, nullable: false },
            },
        };
        fields.push((it.name.name.clone(), s));
    }
    ir::Shape::Object { fields, nullable: false }
}

fn field_shape(a: &Analysis<'_>, f: &FieldInfo<'_>, sub: Option<&ast::Selection>) -> ir::Shape {
    let guarded = f.decl.is_some_and(|d| d.mods.iter().any(|m| matches!(m, ast::FieldMod::VisibleTo(_))));
    let s = match (&f.kind, sub) {
        (MFieldKind::Ref(t), Some(s)) => with_nullable(sel_shape(a, t, s), true), // related row may be invisible
        (MFieldKind::Inverse { target, .. }, Some(s)) => ir::Shape::List { nullable: false, of: Box::new(sel_shape(a, target, s)) },
        (MFieldKind::Counter { .. }, _) => ir::Shape::Value { ty: int_type(), nullable: false },
        (_, _) => match &f.ty {
            Ty::Object => ir::Shape::Value { ty: ir::Type::Url, nullable: false },
            t => ty_shape(t, false),
        },
    };
    with_nullable(s, f.optional || guarded)
}

fn derived_shape(a: &Analysis<'_>, t: &Ty, sub: Option<&ast::Selection>) -> ir::Shape {
    match (t, sub) {
        (Ty::Entity(e), Some(s)) => with_nullable(sel_shape(a, e, s), true),
        (Ty::Coll(inner), Some(s)) => match inner.as_ref() {
            Ty::Entity(e) => ir::Shape::List { nullable: false, of: Box::new(sel_shape(a, e, s)) },
            other => ir::Shape::List { nullable: false, of: Box::new(ty_shape(other, false)) },
        },
        (Ty::Entity(_), None) => ir::Shape::Value { ty: ir::Type::Uuid, nullable: true },
        (t, _) => ty_shape(t, false),
    }
}

/// Values read through `the/latest/first`, an entity-returning relation or an
/// optional field can be missing.
fn may_be_null(a: &Analysis<'_>, e: &ast::Expr) -> bool {
    match &e.kind {
        ast::ExprKind::The(_) | ast::ExprKind::Latest(..) | ast::ExprKind::First(..) => true,
        ast::ExprKind::Call(c) => a.model.relations.get(&c.callee.text()).is_some_and(|r| r.result.is_some()),
        ast::ExprKind::Field(b, _) => may_be_null(a, b),
        ast::ExprKind::Agg { func, .. } => !matches!(func, ast::AggFn::Count | ast::AggFn::Sum),
        _ => false,
    }
}
