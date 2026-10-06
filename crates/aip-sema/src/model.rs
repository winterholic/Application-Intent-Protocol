//! Symbol tables built from the AST: every declared name, every entity field
//! with its resolved type. Built before any expression is checked.

use crate::ty::Ty;
use aip_ir::codes;
use aip_syntax::ast::*;
use aip_syntax::{Diagnostic, Span};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Stored,
    Ref(String),
    UnionRef(Vec<String>),
    Inverse {
        target: String,
        via: String,
    },
    Counter {
        via: String,
    },
    /// id, createdAt, updatedAt, createdBy, updatedBy, deletedAt, version
    Implicit,
}

#[derive(Debug, Clone)]
pub struct FieldInfo<'a> {
    pub name: String,
    pub ty: Ty,
    pub optional: bool,
    pub has_default: bool,
    pub kind: FieldKind,
    pub personal: bool,
    pub decl: Option<&'a FieldDecl>,
}

impl FieldInfo<'_> {
    pub fn writable(&self) -> bool {
        matches!(self.kind, FieldKind::Stored | FieldKind::Ref(_) | FieldKind::UnionRef(_))
    }

    /// Must be given on insert: stored, not optional, no default.
    pub fn required(&self) -> bool {
        self.writable() && !self.optional && !self.has_default
    }
}

#[derive(Debug, Clone, Default)]
pub struct Traits {
    pub created: bool,
    pub updated: bool,
    pub by_actor: bool,
    pub history: bool,
    pub soft_delete: bool,
    pub versioned: bool,
    pub publishable: bool,
}

#[derive(Debug, Clone)]
pub struct EntityInfo<'a> {
    pub decl: &'a EntityDecl,
    pub fields: Vec<FieldInfo<'a>>,
    pub traits: Traits,
}

impl<'a> EntityInfo<'a> {
    pub fn field(&self, name: &str) -> Option<&FieldInfo<'a>> {
        self.fields.iter().find(|f| f.name == name)
    }

    pub fn predicate(&self, name: &str) -> Option<&'a PredicateDecl> {
        self.decl.members.iter().find_map(|m| match m {
            EntityMember::Predicate(p) if p.name.name == name => Some(p),
            _ => None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct EventShape {
    pub fields: Vec<(String, Ty)>,
    pub declared: bool,
    pub span: Span,
}

pub struct Model<'a> {
    pub file: &'a File,
    pub uses: BTreeSet<String>,
    pub actor: Option<&'a ActorDecl>,
    pub enums: BTreeMap<String, &'a EnumDecl>,
    pub records: BTreeMap<String, &'a RecordDecl>,
    pub entities: BTreeMap<String, EntityInfo<'a>>,
    pub relations: BTreeMap<String, &'a RelationDecl>,
    pub fns: BTreeMap<String, &'a FnDecl>,
    pub intents: BTreeMap<String, Span>,
    pub events: BTreeMap<String, EventShape>,
    pub flags: BTreeSet<String>,
    pub configs: BTreeMap<String, &'a ConfigDecl>,
    pub searches: BTreeMap<String, &'a SearchDecl>,
    /// Entities whose rows are the endpoints of an `outbound webhooks` form.
    pub endpoints: BTreeSet<String>,
    pub diags: Vec<Diagnostic>,
}

const SCALARS: &[&str] = &[
    "Bool",
    "Int",
    "Decimal",
    "Text",
    "RichText",
    "Email",
    "Url",
    "Phone",
    "Time",
    "Date",
    "Duration",
    "Uuid",
    "Money",
    "Json",
    "Upload",
    "Recurrence",
];

impl<'a> Model<'a> {
    pub fn actor_entity(&self) -> Option<&str> {
        self.actor.map(|a| a.name.name.as_str())
    }

    pub fn entity(&self, name: &str) -> Option<&EntityInfo<'a>> {
        self.entities.get(name)
    }

    pub fn err(&mut self, code: &str, msg: impl Into<String>, span: Span) -> &mut Diagnostic {
        self.diags.push(Diagnostic::error(code, msg, span));
        self.diags.last_mut().expect("just pushed")
    }

    pub fn warn(&mut self, code: &str, msg: impl Into<String>, span: Span) -> &mut Diagnostic {
        self.diags.push(Diagnostic::warning(code, msg, span));
        self.diags.last_mut().expect("just pushed")
    }

    /// Which enum declares this value? `expected` disambiguates.
    pub fn enum_of_value(&self, value: &str, expected: Option<&Ty>) -> Option<String> {
        if let Some(Ty::Enum(e)) = expected
            && self.enums.get(e).is_some_and(|d| d.values.iter().any(|v| v.name == value))
        {
            return Some(e.clone());
        }
        let owners: Vec<&String> = self.enums.iter().filter(|(_, d)| d.values.iter().any(|v| v.name == value)).map(|(n, _)| n).collect();
        if owners.len() == 1 { Some(owners[0].clone()) } else { None }
    }

    pub fn build(file: &'a File) -> Model<'a> {
        let mut m = Model {
            file,
            uses: BTreeSet::new(),
            actor: None,
            enums: BTreeMap::new(),
            records: BTreeMap::new(),
            entities: BTreeMap::new(),
            relations: BTreeMap::new(),
            fns: BTreeMap::new(),
            intents: BTreeMap::new(),
            events: BTreeMap::new(),
            flags: BTreeSet::new(),
            configs: BTreeMap::new(),
            searches: BTreeMap::new(),
            endpoints: BTreeSet::new(),
            diags: Vec::new(),
        };
        m.collect();
        m.resolve_entities();
        m
    }

    fn collect(&mut self) {
        let mut types: BTreeMap<String, Span> = BTreeMap::new();
        for d in &self.file.decls {
            let type_name = match d {
                Decl::Enum(e) => Some(&e.name),
                Decl::Record(r) => Some(&r.name),
                Decl::Entity(e) => Some(&e.name),
                _ => None,
            };
            if let Some(n) = type_name {
                if SCALARS.contains(&n.name.as_str()) {
                    self.err(codes::E101, format!("'{}' is a built-in type name", n.name), n.span);
                } else if let Some(prev) = types.insert(n.name.clone(), n.span) {
                    self.err(codes::E101, format!("duplicate type name '{}' (first declared at line {})", n.name, prev.line), n.span);
                }
            }
            match d {
                Decl::Use(q) => {
                    self.uses.insert(q.text());
                }
                Decl::Actor(a) => {
                    if self.actor.is_some() {
                        self.err(codes::E107, "only one actor declaration is supported", a.span);
                    }
                    self.actor = Some(a);
                }
                Decl::Enum(e) => {
                    let mut seen = BTreeSet::new();
                    for v in &e.values {
                        if !seen.insert(&v.name) {
                            self.err(codes::E101, format!("duplicate value '{}' in enum {}", v.name, e.name.name), v.span);
                        }
                    }
                    self.enums.insert(e.name.name.clone(), e);
                }
                Decl::Record(r) => {
                    self.records.insert(r.name.name.clone(), r);
                }
                Decl::Relation(r) => {
                    if self.relations.insert(r.name.name.clone(), r).is_some() {
                        self.err(codes::E101, format!("duplicate relation '{}'", r.name.name), r.name.span);
                    }
                }
                Decl::Fn(f) => {
                    if self.fns.insert(f.name.name.clone(), f).is_some() {
                        self.err(codes::E101, format!("duplicate function '{}'", f.name.name), f.name.span);
                    }
                }
                Decl::Event(e) => {
                    let fields = Vec::new(); // resolved below, after types are known
                    self.events.insert(e.name.name.clone(), EventShape { fields, declared: true, span: e.span });
                }
                Decl::Flag(f) => {
                    self.flags.insert(f.name.name.clone());
                }
                Decl::Config(c) => {
                    self.configs.insert(c.name.name.clone(), c);
                }
                Decl::Search(s) => {
                    self.searches.insert(s.name.name.clone(), s);
                }
                Decl::OutboundWebhooks(o) => {
                    self.endpoints.insert(o.entity.name.clone());
                }
                _ => {}
            }
            let intent_name = match d {
                Decl::Query(q) => Some(&q.name),
                Decl::Command(c) => Some(&c.name),
                Decl::Subscribe(s) => Some(&s.name),
                Decl::Job(j) => Some(&j.name),
                Decl::Webhook(w) => Some(&w.name),
                Decl::Verification(v) => Some(&v.name),
                Decl::GrantLink(g) => Some(&g.name),
                Decl::Approval(a) => Some(&a.name),
                _ => None,
            };
            if let Some(n) = intent_name
                && let Some(prev) = self.intents.insert(n.name.clone(), n.span)
            {
                self.err(codes::E101, format!("duplicate operation name '{}' (first declared at line {})", n.name, prev.line), n.span).help =
                    Some("queries, commands and other intents share one namespace".into());
            }
        }
        for d in &self.file.decls {
            if let Decl::Event(e) = d {
                let fields: Vec<(String, Ty)> = e.fields.iter().map(|(n, t)| (n.name.clone(), self.resolve_type(t))).collect();
                if let Some(shape) = self.events.get_mut(&e.name.name) {
                    shape.fields = fields;
                }
            }
        }
    }

    /// Resolves a surface type. Unknown names are reported once and become `Unknown`.
    pub fn resolve_type(&mut self, t: &TypeExpr) -> Ty {
        match &t.kind {
            TypeKind::Name(q) => {
                if q.parts.len() > 1 {
                    let ns = &q.parts[0].name;
                    if !self.uses.contains(ns) {
                        self.err(codes::E501, format!("type '{}' comes from extension '{ns}', which is not declared", q.text()), q.span).help =
                            Some(format!("add 'use {ns}' at the top of the file"));
                    }
                    return if q.text() == "s3.Object" { Ty::Object } else { Ty::Ext(q.text()) };
                }
                self.named_type(&q.parts[0])
            }
            TypeKind::Refined { base, opts } => match base.name.as_str() {
                "Int" => Ty::Int,
                "Text" => Ty::Text,
                "Decimal" => Ty::Decimal,
                "Phone" => Ty::Phone,
                "RichText" => Ty::RichText,
                "Upload" => Ty::Upload,
                "Money" => {
                    let cur = opts.iter().find_map(|o| match o {
                        RefineOpt::Word(w) => Some(w.name.clone()),
                        _ => None,
                    });
                    match cur {
                        Some(c) => Ty::Money(c),
                        None => {
                            self.err(codes::E108, "Money needs a currency: Money(KRW)", base.span);
                            Ty::Unknown
                        }
                    }
                }
                other => {
                    self.err(codes::E108, format!("type '{other}' takes no options"), base.span);
                    Ty::Unknown
                }
            },
            TypeKind::Generic { base, arg, .. } => {
                let inner = self.resolve_type(arg);
                match base.name.as_str() {
                    "Set" | "List" => Ty::Coll(Box::new(inner)),
                    "Range" => {
                        if !(inner.is_temporal() || inner.is_numeric() || inner.is_unknown()) {
                            self.err(codes::E201, format!("Range<{inner}> is not supported; ranges are over time, date or numbers"), t.span);
                        }
                        Ty::Range(Box::new(inner))
                    }
                    "Json" => Ty::Json,
                    "Localized" => Ty::Localized(Box::new(inner)),
                    "Credential" => Ty::Credential,
                    "Snapshot" => match inner {
                        Ty::Entity(e) => Ty::Snapshot(e),
                        Ty::Unknown => Ty::Unknown,
                        other => {
                            self.err(codes::E201, format!("Snapshot<{other}>: only entities can be snapshotted"), t.span);
                            Ty::Unknown
                        }
                    },
                    other => {
                        self.err(codes::E102, format!("unknown generic type '{other}'"), base.span).help =
                            Some("generic types: Set, List, Range, Json, Localized, Credential, Snapshot".into());
                        Ty::Unknown
                    }
                }
            }
            TypeKind::JsonValidated(_) => Ty::Json,
            TypeKind::Many(target) => match self.named_type(target) {
                Ty::Entity(e) => Ty::Coll(Box::new(Ty::Entity(e))),
                Ty::Unknown => Ty::Unknown,
                other => {
                    self.err(codes::E201, format!("'{other}[]' is only valid for entities (inverse relations)"), t.span);
                    Ty::Unknown
                }
            },
            TypeKind::Union(alts) => {
                for a in alts {
                    if !self.entity_names().contains(&a.name) {
                        self.err(codes::E102, format!("unknown entity '{}' in union reference", a.name), a.span);
                    }
                }
                Ty::Union(alts.iter().map(|a| a.name.clone()).collect())
            }
        }
    }

    fn entity_names(&self) -> BTreeSet<String> {
        self.file
            .decls
            .iter()
            .filter_map(|d| match d {
                Decl::Entity(e) => Some(e.name.name.clone()),
                _ => None,
            })
            .collect()
    }

    fn named_type(&mut self, id: &Ident) -> Ty {
        match id.name.as_str() {
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
            "Recurrence" => Ty::Recurrence,
            n if self.enums.contains_key(n) => Ty::Enum(n.to_string()),
            n if self.records.contains_key(n) => Ty::Record(n.to_string()),
            n if self.entity_names().contains(n) => Ty::Entity(n.to_string()),
            n => {
                let mut d = Diagnostic::error(codes::E102, format!("unknown type '{n}'"), id.span);
                d.help = Some("declare it as an entity, enum or record, or use a built-in type (Text, Int, Time, ...)".into());
                self.diags.push(d);
                Ty::Unknown
            }
        }
    }

    fn resolve_entities(&mut self) {
        let decls: Vec<&'a EntityDecl> = self
            .file
            .decls
            .iter()
            .filter_map(|d| match d {
                Decl::Entity(e) => Some(e),
                _ => None,
            })
            .collect();
        for e in decls {
            let mut traits = Traits::default();
            for m in &e.members {
                if let EntityMember::Trait(t) = m {
                    match t {
                        EntityTrait::Track { created, updated, by_actor, .. } => {
                            traits.created |= created;
                            traits.updated |= updated;
                            traits.by_actor |= by_actor;
                        }
                        EntityTrait::History(_) => traits.history = true,
                        EntityTrait::SoftDelete { .. } => traits.soft_delete = true,
                        EntityTrait::Versioned(_) => traits.versioned = true,
                        EntityTrait::Publishable { .. } => traits.publishable = true,
                        _ => {}
                    }
                }
            }
            let implicit = |name: &str, ty: Ty, optional: bool| FieldInfo {
                name: name.to_string(),
                ty,
                optional,
                has_default: true,
                kind: FieldKind::Implicit,
                personal: false,
                decl: None,
            };
            let mut fields = vec![implicit("id", Ty::Uuid, false)];
            if traits.created {
                fields.push(implicit("createdAt", Ty::Time, false));
            }
            if traits.updated {
                fields.push(implicit("updatedAt", Ty::Time, false));
            }
            if traits.by_actor {
                let a = self.actor_entity().map(|s| s.to_string());
                if let Some(a) = a {
                    if traits.created {
                        fields.push(implicit("createdBy", Ty::Entity(a.clone()), true));
                    }
                    if traits.updated {
                        fields.push(implicit("updatedBy", Ty::Entity(a), true));
                    }
                }
            }
            if traits.soft_delete {
                fields.push(implicit("deletedAt", Ty::Time, true));
            }
            // history keeps one row per version, so it needs a version counter too
            if traits.versioned || traits.history {
                fields.push(implicit("version", Ty::Int, false));
            }
            for m in &e.members {
                let EntityMember::Field(f) = m else { continue };
                if fields.iter().any(|x| x.name == f.name.name) {
                    let implicit_clash = fields.iter().any(|x| x.name == f.name.name && x.kind == FieldKind::Implicit);
                    let msg = if implicit_clash {
                        format!("field '{}' is implicit on {} and cannot be declared", f.name.name, e.name.name)
                    } else {
                        format!("duplicate field '{}' in {}", f.name.name, e.name.name)
                    };
                    self.err(if implicit_clash { codes::E105 } else { codes::E101 }, msg, f.name.span);
                    continue;
                }
                let ty = self.resolve_type(&f.ty);
                let via = f.mods.iter().find_map(|m| match m {
                    FieldMod::Via(v) => Some(v.name.clone()),
                    _ => None,
                });
                let counter = f.mods.iter().find_map(|m| match m {
                    FieldMod::Counter { via, .. } => Some(via.name.clone()),
                    _ => None,
                });
                let kind = match (&ty, via, counter) {
                    (_, _, Some(via)) => FieldKind::Counter { via },
                    (Ty::Coll(inner), Some(via), None) => match inner.as_ref() {
                        Ty::Entity(t) => FieldKind::Inverse { target: t.clone(), via },
                        _ => FieldKind::Stored,
                    },
                    (Ty::Coll(inner), None, None) if matches!(inner.as_ref(), Ty::Entity(_)) && matches!(f.ty.kind, TypeKind::Many(_)) => {
                        self.err(codes::E106, format!("{}.{} needs 'via <backref field>'", e.name.name, f.name.name), f.span);
                        FieldKind::Stored
                    }
                    (Ty::Entity(t), _, None) => FieldKind::Ref(t.clone()),
                    (Ty::Union(alts), _, None) => FieldKind::UnionRef(alts.clone()),
                    _ => FieldKind::Stored,
                };
                let personal = f.mods.iter().any(|m| matches!(m, FieldMod::Personal(_)));
                fields.push(FieldInfo {
                    name: f.name.name.clone(),
                    ty,
                    optional: f.ty.optional,
                    // sequence/slug/position values are generated by the runtime
                    has_default: f.default.is_some()
                        || f.mods.iter().any(|m| matches!(m, FieldMod::Sequence { .. } | FieldMod::Slug { .. } | FieldMod::Position { .. })),
                    kind,
                    personal,
                    decl: Some(f),
                });
            }
            self.entities.insert(e.name.name.clone(), EntityInfo { decl: e, fields, traits });
        }
    }
}
