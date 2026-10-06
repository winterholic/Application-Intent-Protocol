//! `rule X on E e when cond do { ... }`. The condition may read the row and its
//! one-hop collections (`e.items`); a trigger on each of those tables marks the
//! owning row pending, and the runtime settles pending rows before commit.

use super::{Planner, sql};
use crate::ddl::trigger;
use crate::names::{lit, q};
use crate::schema::Col;
use crate::sqlexpr::{Val, typed_marker};
use crate::ty::Ty;
use aip_ir::{Expr, Node, SetExpr, SetSource, Stmt};
use aip_plan::*;
use std::collections::BTreeSet;

impl Planner<'_> {
    /// Returns the plan and the trigger DDL that feeds it.
    pub fn rule(&mut self, rule_name: &str, entity: &str, alias: &str, when: &Expr, body: &[Stmt], cross: bool) -> (Rule, Vec<String>) {
        let entity = entity.to_string();
        let t = self.s.table(&entity).clone();
        let binding = alias.to_string();
        let mut c = self.compiler();
        c.push();
        let id = typed_marker(&binding, &Ty::Uuid);
        c.bind(&binding, Val::Id { entity: entity.clone(), sql: id.clone() });
        // the row that triggers the rule fixes the tenant of its condition and steps
        if let Some(sel) = crate::tenant::rows_select(self.core, self.s, &entity, &id, false).filter(|_| !cross) {
            c.tenant = Some(crate::tenant::TenantScope { sql: format!("({sel} LIMIT 1)"), checked: Vec::new() });
        }
        let cond = c.pred(when);
        // rows settle one after another in the transaction that changed them: each starts from its own tenant
        let mut steps: Vec<Step> = self.context_pin(&c, cross).into_iter().collect();
        self.stmts(&mut c, body, &mut steps);
        c.pop();
        self.absorb(&mut c);
        let when_sql = sql(format!("SELECT EXISTS (SELECT 1 FROM {} WHERE \"id\" = {id}) AND {cond}", q(&t.table)));

        // which tables can change the condition's value, and how to find the owning row
        let mut deps: BTreeSet<(String, String)> = BTreeSet::new();
        deps.insert((t.table.clone(), "id".into()));
        let mut far = Vec::new();
        collect_deps(when, &binding, &t, self, &mut deps, &mut far);
        for _ in far {
            self.fail("a rule condition may read the row and its direct collections only");
        }
        let name = lit(rule_name);
        let mut ddl = Vec::new();
        for (table, col) in &deps {
            let body = if col == "id" {
                format!("INSERT INTO \"_aip_rule_pending\" VALUES ({name}, NEW.\"id\") ON CONFLICT DO NOTHING; RETURN NULL;")
            } else {
                let c = q(col);
                format!(
                    "IF TG_OP <> 'DELETE' AND NEW.{c} IS NOT NULL THEN INSERT INTO \"_aip_rule_pending\" VALUES ({name}, NEW.{c}) ON CONFLICT DO NOTHING; END IF; \
                     IF TG_OP <> 'INSERT' AND OLD.{c} IS NOT NULL THEN INSERT INTO \"_aip_rule_pending\" VALUES ({name}, OLD.{c}) ON CONFLICT DO NOTHING; END IF; RETURN NULL;"
                )
            };
            let timing = if col == "id" { "AFTER INSERT OR UPDATE" } else { "AFTER INSERT OR UPDATE OR DELETE" };
            ddl.push(trigger(table, &format!("rule_{}", rule_name.to_lowercase()), timing, &body));
        }
        (Rule { name: rule_name.to_string(), entity, binding, when: when_sql, steps }, ddl)
    }
}

fn collect_deps(e: &Expr, alias: &str, root: &crate::schema::Table, p: &Planner<'_>, deps: &mut BTreeSet<(String, String)>, far: &mut Vec<()>) {
    let is_alias = |x: &Expr| matches!(&x.node, Node::Local { name } if name == alias);
    if let Node::Field { base, field: f, .. } = &e.node {
        match &base.node {
            _ if is_alias(base) => {
                if let Some(Col::Inverse { target, via_col }) = root.col(f) {
                    deps.insert((p.s.table(target).table.clone(), via_col.clone()));
                }
            }
            // e.ref.field reads another row that no trigger can trace back to e
            Node::Field { base: inner, field: g, .. } if is_alias(inner) => {
                if matches!(root.col(g), Some(Col::Ref { .. })) {
                    far.push(());
                }
            }
            _ => {}
        }
    }
    // `Order o where o.product = p ...`: another table, tied to the row by a reference
    let sets: Vec<&SetExpr> = match &e.node {
        Node::Exists { set } | Node::The { set } | Node::Latest { set, .. } | Node::First { set, .. } | Node::Binder { set, .. } => vec![set],
        Node::Quant { set, .. } => vec![set],
        Node::Agg { set: Some(se), .. } => vec![se],
        _ => Vec::new(),
    };
    for se in sets {
        let SetSource::Entity { entity: src } = &se.source else { continue };
        if src == alias || !p.s.tables.contains_key(src) {
            continue;
        }
        let t = p.s.table(src);
        let x = se.alias.as_deref();
        let link = se.filter.as_ref().and_then(|f| ref_link(f, x, alias, t, &root.entity));
        match link {
            Some(col) => {
                deps.insert((t.table.clone(), col));
            }
            None => far.push(()),
        }
    }
    for child in children(e) {
        collect_deps(child, alias, root, p, deps, far);
    }
}

/// Finds `x.f = alias` (either side, under `and`) where `f` references the root entity.
fn ref_link(f: &Expr, x: Option<&str>, alias: &str, t: &crate::schema::Table, root: &str) -> Option<String> {
    match &f.node {
        Node::Binary { op: aip_ir::BinOp::And, l, r } => ref_link(l, x, alias, t, root).or_else(|| ref_link(r, x, alias, t, root)),
        Node::Binary { op: aip_ir::BinOp::Eq, l, r } => {
            let side = |a: &Expr, b: &Expr| -> Option<String> {
                let Node::Field { base, field, .. } = &a.node else { return None };
                let is_elem = match (&base.node, x) {
                    (Node::Local { name }, Some(x)) => name == x,
                    _ => false,
                };
                let is_root = matches!(&b.node, Node::Local { name } if name == alias);
                match t.col(field) {
                    Some(Col::Ref { col, target }) if is_elem && is_root && target == root => Some(col.clone()),
                    _ => None,
                }
            };
            side(l, r).or_else(|| side(r, l))
        }
        _ => None,
    }
}

fn set(se: &SetExpr) -> Vec<&Expr> {
    let mut v: Vec<&Expr> = Vec::new();
    if let SetSource::Expr { expr } = &se.source {
        v.push(expr);
    }
    v.extend(se.filter.as_ref());
    v
}

fn children(e: &Expr) -> Vec<&Expr> {
    match &e.node {
        Node::Field { base: b, .. } | Node::Unary { arg: b, .. } | Node::Pred { row: b, .. } | Node::TypeTest { value: b, .. } => vec![b],
        Node::HasScope { subject: b, .. } => vec![b],
        Node::Binary { l, r, .. } => vec![l, r],
        Node::InSet { value, set: r } | Node::InRangeValue { value, range: r } => vec![value, r],
        Node::InList { value, list } => std::iter::once(value.as_ref()).chain(list.iter()).collect(),
        Node::InRange { value, lo, hi } => vec![value, lo, hi],
        Node::If { cond, then, otherwise } => vec![cond, then, otherwise],
        Node::Exists { set: se } | Node::The { set: se } => set(se),
        Node::Latest { set: se, by } => set(se).into_iter().chain(std::iter::once(by.as_ref())).collect(),
        Node::First { set: se, .. } => set(se),
        Node::Quant { set: se, body, .. } => set(se).into_iter().chain(std::iter::once(body.as_ref())).collect(),
        Node::Agg { set: se, value, .. } => se.as_deref().map(set).unwrap_or_default().into_iter().chain(value.as_deref()).collect(),
        Node::Binder { set: se, body } => set(se).into_iter().chain(std::iter::once(body.as_ref())).collect(),
        Node::Call(call) => call.args.iter().map(|a| &a.value).collect(),
        Node::List { items } => items.iter().collect(),
        _ => Vec::new(),
    }
}
