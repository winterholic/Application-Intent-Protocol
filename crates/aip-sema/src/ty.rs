use serde::Serialize;
use std::fmt;

/// Semantic types. `Coll` marks a multi-valued expression (a set parameter, an
/// inverse relation, a path through one); scalar contexts reject it.
#[derive(Debug, Clone, PartialEq, Serialize)]
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
    /// Result of an extension call whose type is opaque to Core.
    Ext(String),
    Null,
    /// Error recovery: already reported, never report again.
    Unknown,
}

impl Ty {
    pub fn is_unknown(&self) -> bool {
        matches!(self, Ty::Unknown)
    }

    pub fn entity(&self) -> Option<&str> {
        match self {
            Ty::Entity(e) | Ty::Snapshot(e) => Some(e),
            _ => None,
        }
    }

    pub fn is_textual(&self) -> bool {
        matches!(self, Ty::Text | Ty::RichText | Ty::Email | Ty::Url | Ty::Phone)
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Ty::Int | Ty::Decimal | Ty::Money(_))
    }

    pub fn is_temporal(&self) -> bool {
        matches!(self, Ty::Time | Ty::Date)
    }

    /// Can a value of `other` be used where `self` is expected (or be compared with it)?
    pub fn accepts(&self, other: &Ty) -> bool {
        use Ty::*;
        match (self, other) {
            (Unknown, _) | (_, Unknown) | (_, Null) | (Null, _) => true,
            (Localized(a), b) | (b, Localized(a)) => a.accepts(b),
            (a, b) if a.is_textual() && b.is_textual() => true,
            (a, b) if a.is_temporal() && b.is_temporal() => true,
            (Int, Decimal) | (Decimal, Int) => true,
            (Money(_), Int) | (Int, Money(_)) | (Money(_), Decimal) => true,
            (Entity(a), Snapshot(b)) | (Snapshot(a), Entity(b)) => a == b,
            (Union(alts), Entity(e)) | (Entity(e), Union(alts)) => alts.contains(e),
            (Entity(_), Uuid) | (Uuid, Entity(_)) => true,
            (Ext(_), _) | (_, Ext(_)) => true,
            (Object, Upload) => true,
            (Json, Record(_)) | (Record(_), Json) => true,
            (Coll(a), Coll(b)) => a.accepts(b),
            (a, b) => a == b,
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
