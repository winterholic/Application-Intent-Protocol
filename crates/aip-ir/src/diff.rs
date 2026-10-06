//! What changed between two versions of a program, in terms of meaning: which entities and fields appeared,
//! disappeared, were renamed or changed type, which enum values moved, which constraints and traits differ.
//! It knows nothing about SQL; a backend turns these changes into whatever it needs to carry stored data
//! from the old version to the new one (`aip-pg` `evolve`).
//!
//! The only intent the programs themselves can state is `was` (a rename) and `removed` (a removal). A name that is
//! gone without either is reported as a removal that was not declared, and the backend refuses it: dropping a
//! column destroys data, a rename keeps it, and the diff cannot tell which one was meant.

use crate::{Constraint, Entity, Field, FieldKind, Form, Program, Type};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum Change {
    EntityAdded {
        entity: String,
    },
    EntityRemoved {
        entity: String,
        declared: bool,
    },
    EntityRenamed {
        from: String,
        to: String,
    },
    FieldAdded {
        entity: String,
        field: String,
    },
    FieldRemoved {
        entity: String,
        field: String,
        declared: bool,
    },
    FieldRenamed {
        entity: String,
        from: String,
        to: String,
    },
    /// The type of a stored field changed; `relation` says how the two types compare.
    FieldType {
        entity: String,
        field: String,
        from: Type,
        to: Type,
        relation: TypeRelation,
    },
    /// Required became optional (`optional: true`) or the reverse.
    FieldOptional {
        entity: String,
        field: String,
        optional: bool,
    },
    FieldDefault {
        entity: String,
        field: String,
    },
    /// Stored, reference, inverse, counter, ...: what the field is changed, not just its type.
    FieldKind {
        entity: String,
        field: String,
    },
    /// The field became `encrypted` (`now`) or stopped being: the stored values are in the other form.
    FieldEncryption {
        entity: String,
        field: String,
        now: bool,
    },
    /// Delete policy of a reference, or another property that changes the objects the backend creates for the field.
    FieldShape {
        entity: String,
        field: String,
    },
    EnumValueAdded {
        name: String,
        value: String,
        /// Not at the end of the list: for an `ordered` enum every comparison involving the new value reads differently.
        inside: bool,
        ordered: bool,
    },
    EnumValueRemoved {
        name: String,
        value: String,
    },
    /// The same values in another order, and the order is meaningful (`ordered`).
    EnumReordered {
        name: String,
    },
    ConstraintAdded {
        entity: String,
        kind: String,
    },
    ConstraintRemoved {
        entity: String,
        kind: String,
    },
    /// An entity trait that owns tables or triggers of its own (`history`, `publishable`, `tenant via`, `dynamic schema from`).
    TraitChanged {
        entity: String,
        name: String,
        now: bool,
    },
    SearchChanged {
        name: String,
        entity: String,
    },
}

/// How a new type compares to the one it replaces, as far as the values already stored are concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeRelation {
    /// Every value of the old type is a value of the new one.
    Widens,
    /// The new type refuses values the old one allowed; stored rows may break it.
    Narrows,
    /// Different kind of value, or a change this module cannot compare.
    Incompatible,
}

/// The entity of `old` that `name` (an entity of `new`) continues, if any.
pub fn old_entity_name<'a>(old: &'a Program, new: &'a Program, name: &'a str) -> Option<&'a str> {
    let e = new.entities.get(name)?;
    if old.entities.contains_key(name) {
        return Some(name);
    }
    e.was.as_deref().filter(|w| old.entities.contains_key(*w))
}

/// The field of `old` that `f` (a field of `new`'s entity) continues, if any.
pub fn old_field_name<'a>(old: &'a Entity, f: &'a Field) -> Option<&'a str> {
    if old.fields.iter().any(|o| o.name == f.name) {
        return Some(&f.name);
    }
    f.was.as_deref().filter(|w| old.fields.iter().any(|o| o.name == *w))
}

pub fn diff(old: &Program, new: &Program) -> Vec<Change> {
    let mut out = Vec::new();
    enums(old, new, &mut out);
    let mut matched: Vec<&str> = Vec::new();
    for (name, e) in &new.entities {
        match old_entity_name(old, new, name) {
            None => out.push(Change::EntityAdded { entity: name.clone() }),
            Some(o) => {
                matched.push(o);
                if o != name {
                    out.push(Change::EntityRenamed { from: o.to_string(), to: name.clone() });
                }
                entity(name, &old.entities[o], e, &mut out);
            }
        }
    }
    for name in old.entities.keys() {
        if !matched.contains(&name.as_str()) {
            out.push(Change::EntityRemoved { entity: name.clone(), declared: new.removed_entities.contains(name) });
        }
    }
    searches(old, new, &mut out);
    out
}

fn entity(name: &str, old: &Entity, new: &Entity, out: &mut Vec<Change>) {
    let mut matched: Vec<&str> = Vec::new();
    for f in &new.fields {
        match old_field_name(old, f) {
            None => out.push(Change::FieldAdded { entity: name.into(), field: f.name.clone() }),
            Some(o) => {
                matched.push(o);
                if o != f.name {
                    out.push(Change::FieldRenamed { entity: name.into(), from: o.to_string(), to: f.name.clone() });
                }
                if let Some(of) = old.fields.iter().find(|x| x.name == o) {
                    field(name, of, f, out);
                }
            }
        }
    }
    for f in &old.fields {
        if !matched.contains(&f.name.as_str()) {
            out.push(Change::FieldRemoved { entity: name.into(), field: f.name.clone(), declared: new.removed_fields.contains(&f.name) });
        }
    }
    constraints(name, old, new, out);
    for (t, a, b) in [
        ("history", old.traits.history, new.traits.history),
        ("publishable", old.traits.publishable, new.traits.publishable),
        ("tenant", old.traits.tenant.is_some(), new.traits.tenant.is_some()),
        ("dynamic_schema", old.traits.dynamic_schema.is_some(), new.traits.dynamic_schema.is_some()),
    ] {
        if a != b {
            out.push(Change::TraitChanged { entity: name.into(), name: t.into(), now: b });
        }
    }
}

fn field(entity: &str, old: &Field, new: &Field, out: &mut Vec<Change>) {
    let (e, n) = (entity.to_string(), new.name.clone());
    if kind_class(&old.kind) != kind_class(&new.kind) {
        out.push(Change::FieldKind { entity: e, field: n });
        return;
    }
    if old.ty != new.ty {
        out.push(Change::FieldType {
            entity: e.clone(),
            field: n.clone(),
            from: old.ty.clone(),
            to: new.ty.clone(),
            relation: relation(&old.ty, &new.ty),
        });
    }
    if old.encrypted != new.encrypted {
        out.push(Change::FieldEncryption { entity: e.clone(), field: n.clone(), now: new.encrypted });
    }
    if old.optional != new.optional {
        out.push(Change::FieldOptional { entity: e.clone(), field: n.clone(), optional: new.optional });
    }
    if old.default != new.default {
        out.push(Change::FieldDefault { entity: e.clone(), field: n.clone() });
    }
    if old.kind != new.kind || old.generated != new.generated {
        out.push(Change::FieldShape { entity: e, field: n });
    }
}

/// Kinds that are different things; the parameters of one kind (a delete policy) are not part of this.
fn kind_class(k: &FieldKind) -> (u8, Vec<String>) {
    match k {
        FieldKind::Stored => (0, Vec::new()),
        FieldKind::Ref { target, .. } => (1, vec![target.clone()]),
        FieldKind::RefUnion { targets, .. } => (2, targets.clone()),
        FieldKind::Inverse { target, via } => (3, vec![target.clone(), via.clone()]),
        FieldKind::Counter { .. } => (4, Vec::new()),
        FieldKind::Implicit => (5, Vec::new()),
    }
}

/// How `new` compares to `old` for the values stored under `old`.
pub fn relation(old: &Type, new: &Type) -> TypeRelation {
    use TypeRelation::*;
    let fold = |rs: &[TypeRelation]| {
        if rs.contains(&Incompatible) {
            Incompatible
        } else if rs.contains(&Narrows) {
            Narrows
        } else {
            Widens
        }
    };
    // a missing bound is the widest one
    let low = |a: Option<i64>, b: Option<i64>| match (a, b) {
        (x, y) if x == y => Widens,
        (_, None) => Widens,
        (None, Some(_)) => Narrows,
        (Some(x), Some(y)) => {
            if y <= x {
                Widens
            } else {
                Narrows
            }
        }
    };
    let high = |a: Option<i64>, b: Option<i64>| match (a, b) {
        (x, y) if x == y => Widens,
        (_, None) => Widens,
        (None, Some(_)) => Narrows,
        (Some(x), Some(y)) => {
            if y >= x {
                Widens
            } else {
                Narrows
            }
        }
    };
    match (old, new) {
        (a, b) if a == b => Widens,
        (Type::Int { min: a0, max: a1 }, Type::Int { min: b0, max: b1 }) => fold(&[low(*a0, *b0), high(*a1, *b1)]),
        (Type::Text { min: a0, max: a1, trim: at, lower: al, pattern: ap }, Type::Text { min: b0, max: b1, trim: bt, lower: bl, pattern: bp }) => {
            let flag = |a: bool, b: bool| if a == b || !b { Widens } else { Narrows };
            let pattern = match (ap, bp) {
                (a, b) if a == b => Widens,
                (_, None) => Widens,
                (None, Some(_)) => Narrows,
                // two different patterns cannot be compared
                _ => Incompatible,
            };
            fold(&[low(*a0, *b0), high(*a1, *b1), flag(*at, *bt), flag(*al, *bl), pattern])
        }
        (Type::Decimal { precision: ap, scale: asc }, Type::Decimal { precision: bp, scale: bsc }) => {
            if asc == bsc && (bp.is_none() || (ap.is_some() && bp >= ap)) { Widens } else { Incompatible }
        }
        (Type::List { of: a, max: am }, Type::List { of: b, max: bm }) | (Type::Set { of: a, max: am }, Type::Set { of: b, max: bm }) => {
            match (relation(a, b), bm.cmp(am)) {
                (Incompatible, _) => Incompatible,
                (Narrows, _) | (_, std::cmp::Ordering::Less) => Narrows,
                _ => Widens,
            }
        }
        // a text type that accepts more than the one that held the value
        (
            Type::Email | Type::Url | Type::Phone { .. } | Type::RichText { .. } | Type::Enum { .. },
            Type::Text { min: None, max: None, pattern: None, .. },
        ) => Widens,
        _ => Incompatible,
    }
}

fn enums(old: &Program, new: &Program, out: &mut Vec<Change>) {
    for (name, n) in &new.enums {
        let Some(o) = old.enums.get(name) else { continue };
        for (i, v) in n.values.iter().enumerate() {
            if !o.values.contains(v) {
                // appended: everything before it is exactly the old list
                let inside = n.values[..i].iter().any(|x| !o.values.contains(x)) || n.values[i + 1..].iter().any(|x| o.values.contains(x));
                out.push(Change::EnumValueAdded { name: name.clone(), value: v.clone(), inside, ordered: n.ordered });
            }
        }
        for v in &o.values {
            if !n.values.contains(v) {
                out.push(Change::EnumValueRemoved { name: name.clone(), value: v.clone() });
            }
        }
        let kept = |vs: &[String], other: &[String]| vs.iter().filter(|v| other.contains(v)).cloned().collect::<Vec<_>>();
        if (o.ordered || n.ordered) && kept(&o.values, &n.values) != kept(&n.values, &o.values) {
            out.push(Change::EnumReordered { name: name.clone() });
        }
    }
}

/// Constraints are compared by what they say; the ordinal only says where they were written.
fn constraints(entity: &str, old: &Entity, new: &Entity, out: &mut Vec<Change>) {
    let key = |c: &Constraint| -> String {
        let mut v = serde_json::to_value(c).unwrap_or_default();
        if let Some(m) = v.as_object_mut() {
            m.remove("ordinal");
        }
        v.to_string()
    };
    let kind = |c: &Constraint| -> String {
        match c {
            Constraint::Unique { fields, .. } => format!("unique({})", fields.join(", ")),
            Constraint::Cardinality { .. } => "cardinality".into(),
            Constraint::NoOverlap { range, .. } => format!("no overlap({range})"),
            Constraint::Capacity { .. } => "capacity".into(),
            Constraint::Invariant { name, .. } => format!("invariant {name}"),
        }
    };
    let mut old_keys: BTreeMap<String, &Constraint> = BTreeMap::new();
    for c in &old.constraints {
        old_keys.insert(key(c), c);
    }
    let mut new_keys: BTreeMap<String, &Constraint> = BTreeMap::new();
    for c in &new.constraints {
        new_keys.insert(key(c), c);
    }
    for (k, c) in &new_keys {
        if !old_keys.contains_key(k) {
            out.push(Change::ConstraintAdded { entity: entity.into(), kind: kind(c) });
        }
    }
    for (k, c) in &old_keys {
        if !new_keys.contains_key(k) {
            out.push(Change::ConstraintRemoved { entity: entity.into(), kind: kind(c) });
        }
    }
}

fn searches(old: &Program, new: &Program, out: &mut Vec<Change>) {
    let list = |p: &Program| -> BTreeMap<String, crate::Search> {
        p.forms
            .iter()
            .filter_map(|f| match f {
                Form::Search(s) => Some((s.name.clone(), s.clone())),
                _ => None,
            })
            .collect()
    };
    let (o, n) = (list(old), list(new));
    for (name, s) in &n {
        if o.get(name) != Some(s) {
            out.push(Change::SearchChanged { name: name.clone(), entity: s.entity.clone() });
        }
    }
    for (name, s) in &o {
        if !n.contains_key(name) {
            out.push(Change::SearchChanged { name: name.clone(), entity: s.entity.clone() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(min: Option<i64>, max: Option<i64>) -> Type {
        Type::Text { min, max, trim: false, lower: false, pattern: None }
    }

    #[test]
    fn text_limits_widen_and_narrow() {
        assert_eq!(relation(&text(None, Some(30)), &text(None, Some(100))), TypeRelation::Widens);
        assert_eq!(relation(&text(None, Some(30)), &text(None, None)), TypeRelation::Widens);
        assert_eq!(relation(&text(None, Some(100)), &text(None, Some(30))), TypeRelation::Narrows);
        assert_eq!(relation(&text(None, None), &text(None, Some(30))), TypeRelation::Narrows);
        assert_eq!(relation(&text(Some(1), None), &text(None, None)), TypeRelation::Widens);
    }

    #[test]
    fn int_ranges_and_other_kinds() {
        let int = |min, max| Type::Int { min, max };
        assert_eq!(relation(&int(Some(0), Some(10)), &int(Some(-5), Some(20))), TypeRelation::Widens);
        assert_eq!(relation(&int(Some(0), Some(10)), &int(Some(1), Some(10))), TypeRelation::Narrows);
        assert_eq!(relation(&int(None, None), &text(None, None)), TypeRelation::Incompatible);
        assert_eq!(relation(&Type::Email, &text(None, None)), TypeRelation::Widens);
        assert_eq!(relation(&text(None, None), &Type::Email), TypeRelation::Incompatible);
    }
}
