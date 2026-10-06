//! Tenant model derived from `tenant via <path>` (see `spec/grammar.md`, "tenant").
//!
//! An entity with the trait is *tenant-scoped*; the tenant of one of its rows is
//! the row the path ends at (the *root*, e.g. `Workspace`). The root is its own
//! tenant. Nothing here names SQL: backends turn the path into whatever they use
//! to compare tenants, and every consumer (analysis, backends) asks these
//! functions instead of re-reading the traits.
//!
//! Single-tenant rules, in terms of this module:
//! - references between tenant entities stay inside one tenant (backend-enforced on write);
//! - an intent's entity parameters of tenant entities ([`checked_params`]) all belong to one tenant;
//! - a set of tenant-scoped rows that is not [`bound`] to those parameters is filtered to the
//!   tenant of the first required one ([`anchor`]), and an intent without one cannot read such a set;
//! - `internal cross tenant` intents are exempt;
//! - every execution context (an intent call, an event handler, a webhook, a schedule item, a rule row,
//!   a job item) writes one tenant, which the backend fixes on the first write or from the anchor;
//!   `cross tenant` declarations are the exception, and a reference between two tenants stays refused even there.

use crate::facts::entity_of_type;
use crate::*;
use std::collections::BTreeSet;

/// One step of a tenant path: `field` of `entity` references `target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hop {
    pub entity: String,
    pub field: String,
    pub target: String,
}

/// The entity a field refers to, whether declared as a reference or typed as one.
pub fn ref_target(f: &Field) -> Option<&str> {
    match (&f.kind, &f.ty) {
        (FieldKind::Ref { target, .. }, _) => Some(target),
        (FieldKind::Stored, Type::Ref { entity }) => Some(entity),
        _ => None,
    }
}

/// Does the program use tenants at all? Backends add tenant machinery to plans only then.
pub fn active(p: &Program) -> bool {
    p.entities.keys().any(|e| path(p, e).is_ok())
}

/// Does the entity carry `tenant via ...`?
pub fn scoped(p: &Program, entity: &str) -> bool {
    p.entities.get(entity).is_some_and(|e| e.traits.tenant.is_some())
}

/// The hops from a scoped entity to its root. `Err` says why the path is not usable
/// (reported by analysis as `AIP-E313`); backends treat an `Err` as "not scoped".
pub fn path(p: &Program, entity: &str) -> Result<Vec<Hop>, String> {
    let Some(via) = p.entities.get(entity).and_then(|e| e.traits.tenant.as_ref()) else {
        return Err(format!("{entity} has no tenant path"));
    };
    let mut hops: Vec<Hop> = Vec::new();
    let mut cur = entity.to_string();
    for seg in via {
        let Some(f) = p.entities.get(&cur).and_then(|e| e.fields.iter().find(|f| f.name == *seg)) else {
            return Err(format!("{cur} has no field '{seg}'"));
        };
        let Some(target) = ref_target(f) else {
            return Err(format!("{cur}.{seg} is not a reference"));
        };
        if f.optional {
            return Err(format!("{cur}.{seg} is optional; every row needs a tenant"));
        }
        hops.push(Hop { entity: cur.clone(), field: seg.clone(), target: target.to_string() });
        cur = target.to_string();
    }
    if hops.is_empty() {
        return Err(format!("the tenant path of {entity} is empty"));
    }
    if scoped(p, &cur) {
        return Err(format!("the path ends at {cur}, which is itself tenant-scoped; end it at the entity that is the tenant"));
    }
    Ok(hops)
}

/// Every entity some usable tenant path ends at.
pub fn roots(p: &Program) -> BTreeSet<String> {
    p.entities.keys().filter_map(|e| path(p, e).ok()).filter_map(|h| h.last().map(|x| x.target.clone())).collect()
}

pub fn is_root(p: &Program, entity: &str) -> bool {
    roots(p).contains(entity)
}

/// Scoped entity or root: rows that have a tenant.
pub fn has_tenant(p: &Program, entity: &str) -> bool {
    (scoped(p, entity) && path(p, entity).is_ok()) || is_root(p, entity)
}

/// Entity parameters whose rows have a tenant. Optional ones are included: they are
/// compared with the others when present.
pub fn checked_params<'a>(p: &Program, params: &'a [Param]) -> Vec<&'a Param> {
    params.iter().filter(|x| entity_of_type(&x.ty).is_some_and(|e| has_tenant(p, &e))).collect()
}

/// The parameter whose tenant filters unbound scans: the first required one.
pub fn anchor<'a>(checked: &[&'a Param]) -> Option<&'a Param> {
    checked.iter().find(|x| !x.optional && x.default.is_none()).copied()
}

/// Must the runtime compare tenants after loading? Two parameters can disagree, and a set parameter can hold rows of several.
pub fn needs_check(checked: &[&Param]) -> bool {
    checked.len() > 1 || checked.iter().any(|x| !matches!(x.ty, Type::Ref { .. }))
}

/// Is `e` a set of rows (or one row) already inside the tenant of the checked parameters?
/// A checked parameter is; so is anything reached from a bound row through references between
/// tenant entities, or through the inverse of such a reference, because the backend refuses
/// a reference that crosses tenants. A set that starts anywhere else (an entity scan, the
/// actor's inverse relations, a local) is not bound and gets the tenant filter.
pub fn bound(p: &Program, checked: &[&Param], e: &Expr) -> bool {
    match &e.node {
        Node::Param { name } => checked.iter().any(|x| x.name == *name),
        Node::Field { base, entity: Some(ent), field } => {
            if !has_tenant(p, ent) || !bound(p, checked, base) {
                return false;
            }
            let Some(f) = p.entities.get(ent).and_then(|x| x.fields.iter().find(|f| f.name == *field)) else { return false };
            match &f.kind {
                FieldKind::Inverse { target, .. } => scoped(p, target),
                _ => ref_target(f).is_some_and(|t| has_tenant(p, t)),
            }
        }
        _ => false,
    }
}

/// The field of an event payload that fixes the tenant of its handlers: the first
/// reference to a row that has a tenant. `(field, entity)`.
pub fn event_anchor(p: &Program, event: &str) -> Option<(String, String)> {
    p.events.get(event)?.fields.iter().find_map(|(n, t)| match t {
        Type::Ref { entity } if has_tenant(p, entity) => Some((n.clone(), entity.clone())),
        _ => None,
    })
}
