//! Whole-program write analysis: which entity fields are ever assigned an
//! absolute value (`set x.f = v`, `update E ... set f = v`). A field that is
//! only ever moved by `+=`/`-=` (stock, balances, tallies) commutes, so it is
//! treated like a counter by the version trigger: an order that takes stock
//! does not make a concurrent price edit stale.

use crate::ty::Ty;
use aip_ir::*;
use std::collections::BTreeSet;

pub fn absolute_fields(core: &Program) -> BTreeSet<(String, String)> {
    collect(core, true)
}

/// Fields moved by `+=` / `-=` somewhere.
pub fn delta_fields(core: &Program) -> BTreeSet<(String, String)> {
    collect(core, false)
}

fn collect(core: &Program, absolute: bool) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    let mut blocks: Vec<&[Stmt]> = Vec::new();
    for i in core.intents.values() {
        if let Intent::Command(c) = i {
            blocks.push(&c.body);
        }
    }
    for r in &core.reactions {
        match r {
            Reaction::Event { body, .. } | Reaction::Rule { body, .. } | Reaction::Consume { body, .. } => blocks.push(body),
            Reaction::Schedule(s) => blocks.push(&s.body),
            Reaction::Webhook(w) => blocks.extend(w.handlers.iter().map(|h| h.body.as_slice())),
            Reaction::Retain { .. } => {}
        }
    }
    for f in &core.forms {
        match f {
            Form::Job(j) => blocks.extend(j.body.as_deref()),
            Form::Verification(v) => blocks.push(&v.on_verified.2),
            Form::GrantLink(g) => blocks.extend(g.on_redeem.as_deref()),
            Form::Approval(ap) => {
                blocks.push(&ap.on_approved);
                blocks.push(&ap.on_rejected);
            }
            Form::Migration(m) => blocks.push(&m.body),
            Form::Projection(p) => blocks.extend(p.handlers.iter().map(|(_, _, s)| std::slice::from_ref(s))),
            _ => {}
        }
    }
    for e in core.entities.values() {
        for c in &e.constraints {
            if let Constraint::Cardinality { repair: Some(b), .. } = c {
                blocks.push(b);
            }
        }
    }
    for b in blocks {
        walk(b, absolute, &mut out);
    }
    out
}

fn entity_of(e: &Expr) -> Option<String> {
    match Ty::from_ir(&e.ty) {
        Ty::Entity(n) => Some(n),
        Ty::Coll(inner) => match *inner {
            Ty::Entity(n) => Some(n),
            _ => None,
        },
        _ => None,
    }
}

fn set_entity(se: &SetExpr) -> Option<String> {
    match &se.source {
        SetSource::Entity { entity } => Some(entity.clone()),
        SetSource::Expr { expr } => entity_of(expr),
    }
}

fn walk(stmts: &[Stmt], absolute: bool, out: &mut BTreeSet<(String, String)>) {
    let wanted = |x: &Assign| matches!(x.op, AssignOp::Set) == absolute;
    for s in stmts {
        match s {
            Stmt::Set { assigns } => {
                for x in assigns.iter().filter(|x| wanted(x)) {
                    if let Node::Field { base, field, .. } = &x.target.node
                        && let Some(e) = entity_of(base)
                    {
                        out.insert((e, field.clone()));
                    }
                }
            }
            Stmt::Update { target, assigns, .. } => {
                let Some(e) = set_entity(target) else { continue };
                for x in assigns.iter().filter(|x| wanted(x)) {
                    let f = match &x.target.node {
                        Node::RowField { field, .. } | Node::Field { field, .. } => field.clone(),
                        _ => continue,
                    };
                    out.insert((e.clone(), f));
                }
            }
            Stmt::When { body, .. } | Stmt::Each { body, .. } => walk(body, absolute, out),
            _ => {}
        }
    }
}
