//! Wire types of the client contract. Refinements a caller must respect
//! (ranges, lengths, patterns) are kept for declared types; types inferred for
//! computed values (event payloads, outputs) carry the kind only.

use aip_ir as ir;
use serde::Serialize;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeSpec {
    Bool,
    Int { min: Option<i64>, max: Option<i64> },
    Decimal,
    Text { min: Option<i64>, max: Option<i64>, trim: bool, lower: bool, pattern: Option<String> },
    RichText,
    Email,
    Url,
    Phone,
    Time,
    Date,
    Duration,
    Uuid,
    Money { currency: String },
    Json,
    Enum { name: String },
    Record { name: String },
    Entity { entity: String },
    Union { entities: Vec<String> },
    Snapshot { entity: String },
    Set { of: Box<TypeSpec>, max: u64 },
    List { of: Box<TypeSpec>, max: u64 },
    Range { of: Box<TypeSpec> },
    Upload { max_bytes: Option<u64>, types: Vec<String> },
    Object,
    Other { name: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct ParamSpec {
    pub name: String,
    pub ty: TypeSpec,
    pub optional: bool,
    pub default: Option<serde_json::Value>,
}

/// A value's type as the contract's inference sees it.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Bool,
    Int,
    Decimal,
    Text,
    RichText,
    Email,
    Url,
    Phone,
    Time,
    Date,
    Duration,
    Size,
    Uuid,
    Money(String),
    Json,
    Enum(String),
    Record(String),
    Entity(String),
    Union(Vec<String>),
    Snapshot(String),
    Range(Box<Ty>),
    Localized(Box<Ty>),
    Coll(Box<Ty>),
    Object,
    Upload,
    Credential,
    Recurrence,
    Ext(String),
    Null,
    Unknown,
}

impl Ty {
    pub fn from_ir(t: &ir::Type) -> Ty {
        use ir::Type as T;
        match t {
            T::Bool => Ty::Bool,
            T::Int { .. } => Ty::Int,
            T::Decimal { .. } => Ty::Decimal,
            T::Text { .. } => Ty::Text,
            T::RichText { .. } => Ty::RichText,
            T::Email => Ty::Email,
            T::Url => Ty::Url,
            T::Phone { .. } => Ty::Phone,
            T::Time => Ty::Time,
            T::Date => Ty::Date,
            T::Duration => Ty::Duration,
            T::Size => Ty::Size,
            T::Uuid => Ty::Uuid,
            T::Money { currency } => Ty::Money(currency.clone()),
            T::Json { .. } => Ty::Json,
            T::Enum { name } => Ty::Enum(name.clone()),
            T::Record { name } => Ty::Record(name.clone()),
            // an event payload binding is typed as the record `event:<Name>`
            T::Event { name } => Ty::Record(format!("event:{name}")),
            T::Ref { entity } => Ty::Entity(entity.clone()),
            T::RefUnion { entities } => Ty::Union(entities.clone()),
            T::Snapshot { entity } => Ty::Snapshot(entity.clone()),
            T::Set { of, .. } | T::List { of, .. } | T::Many { of } => Ty::Coll(Box::new(Ty::from_ir(of))),
            T::Range { of } => Ty::Range(Box::new(Ty::from_ir(of))),
            T::Localized { of } => Ty::Localized(Box::new(Ty::from_ir(of))),
            T::Upload { .. } => Ty::Upload,
            T::Object => Ty::Object,
            T::Credential { .. } => Ty::Credential,
            T::Recurrence => Ty::Recurrence,
            T::Ext { name } => Ty::Ext(name.clone()),
            T::Null => Ty::Null,
            T::Symbol | T::Unknown => Ty::Unknown,
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Ty::*;
        match self {
            Money(c) => write!(f, "Money({c})"),
            Enum(n) | Record(n) | Entity(n) => write!(f, "{n}"),
            Union(v) => write!(f, "ref {}", v.join(" | ")),
            Snapshot(e) => write!(f, "Snapshot<{e}>"),
            Range(t) => write!(f, "Range<{t}>"),
            Localized(t) => write!(f, "Localized<{t}>"),
            Coll(t) => write!(f, "{t}[]"),
            Ext(n) => write!(f, "{n}"),
            other => write!(f, "{other:?}"),
        }
    }
}

fn ty_spec(ty: &Ty) -> TypeSpec {
    match ty {
        Ty::Bool => TypeSpec::Bool,
        Ty::Int | Ty::Size => TypeSpec::Int { min: None, max: None },
        Ty::Decimal => TypeSpec::Decimal,
        Ty::Money(c) => TypeSpec::Money { currency: c.clone() },
        Ty::Text | Ty::Recurrence => TypeSpec::Text { min: None, max: None, trim: false, lower: false, pattern: None },
        Ty::RichText => TypeSpec::RichText,
        Ty::Email => TypeSpec::Email,
        Ty::Url => TypeSpec::Url,
        Ty::Phone => TypeSpec::Phone,
        Ty::Time => TypeSpec::Time,
        Ty::Date => TypeSpec::Date,
        Ty::Duration => TypeSpec::Duration,
        Ty::Uuid => TypeSpec::Uuid,
        Ty::Enum(n) => TypeSpec::Enum { name: n.clone() },
        Ty::Record(n) => TypeSpec::Record { name: n.clone() },
        Ty::Entity(e) => TypeSpec::Entity { entity: e.clone() },
        Ty::Union(v) => TypeSpec::Union { entities: v.clone() },
        Ty::Snapshot(e) => TypeSpec::Snapshot { entity: e.clone() },
        Ty::Range(t) => TypeSpec::Range { of: Box::new(ty_spec(t)) },
        Ty::Coll(t) => TypeSpec::List { of: Box::new(ty_spec(t)), max: 0 },
        Ty::Object => TypeSpec::Object,
        Ty::Upload => TypeSpec::Upload { max_bytes: None, types: Vec::new() },
        other => TypeSpec::Other { name: other.to_string() },
    }
}

/// Contract type of a computed value of IR type `t` (kind only).
pub fn inferred(t: &ir::Type) -> TypeSpec {
    ty_spec(&Ty::from_ir(t))
}

/// Contract type of a declared IR type, refinements included.
pub fn declared(t: &ir::Type) -> TypeSpec {
    use ir::Type as T;
    match t {
        T::Bool => TypeSpec::Bool,
        T::Int { min, max } => TypeSpec::Int { min: *min, max: *max },
        T::Decimal { .. } => TypeSpec::Decimal,
        T::Text { min, max, trim, lower, pattern } => TypeSpec::Text { min: *min, max: *max, trim: *trim, lower: *lower, pattern: pattern.clone() },
        T::RichText { .. } => TypeSpec::RichText,
        T::Email => TypeSpec::Email,
        T::Url => TypeSpec::Url,
        T::Phone { .. } => TypeSpec::Phone,
        T::Time => TypeSpec::Time,
        T::Date => TypeSpec::Date,
        T::Duration => TypeSpec::Duration,
        T::Uuid => TypeSpec::Uuid,
        T::Money { currency } => TypeSpec::Money { currency: currency.clone() },
        T::Json { .. } | T::Localized { .. } | T::Credential { .. } => TypeSpec::Json,
        T::Enum { name } => TypeSpec::Enum { name: name.clone() },
        T::Record { name } => TypeSpec::Record { name: name.clone() },
        T::Ref { entity } => TypeSpec::Entity { entity: entity.clone() },
        T::RefUnion { entities } => TypeSpec::Union { entities: entities.clone() },
        T::Snapshot { entity } => TypeSpec::Snapshot { entity: entity.clone() },
        T::Set { of, max } => TypeSpec::Set { of: Box::new(declared(of)), max: *max },
        T::List { of, max } => TypeSpec::List { of: Box::new(declared(of)), max: *max },
        T::Many { of } => TypeSpec::Set { of: Box::new(declared(of)), max: 0 },
        T::Range { of } => TypeSpec::Range { of: Box::new(declared(of)) },
        T::Upload { max_bytes, types } => TypeSpec::Upload { max_bytes: *max_bytes, types: types.clone() },
        T::Object => TypeSpec::Object,
        T::Recurrence => TypeSpec::Other { name: "Recurrence".into() },
        T::Ext { name } => TypeSpec::Other { name: name.clone() },
        other => inferred(other),
    }
}
