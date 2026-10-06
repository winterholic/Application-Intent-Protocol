//! `entity E { publishable by <cond> }` keeps two versions of every row. The entity's own table is the working
//! (draft) version: every command, rule and job reads and edits it exactly as before. `<table>_published` holds the
//! version clients read: queries are compiled against [`Schema::published`], so a row nobody published is not in
//! any list and an edit does not show until someone publishes it. Two commands move rows between the two:
//!
//! - `PublishE(e)` copies the working row over its published one and records when
//! - `DiscardEDraft(e)` copies the published row back over the working one
//!
//! Both are allowed to whoever the `by` condition says (over the row and the actor), and to the superuser without it.
//! A `drafts query` reads the working table, which is how an editor sees what is not published yet.

use super::{Planner, sql};
use crate::names::q;
use crate::schema::{Col, Table, published_table};
use crate::sqlexpr::{Compiler, Val, typed_marker};
use crate::ty::Ty;
use aip_ir::form_intents::{self, FormIntent, NOT_PUBLISHED};
use aip_plan::*;
use serde_json::json;
use std::collections::BTreeSet;

/// Every physical column of a table, in a stable order.
fn columns(t: &Table) -> Vec<String> {
    let mut out = Vec::new();
    for c in t.cols.values() {
        match c {
            Col::Scalar { col, .. } | Col::Counter { col } | Col::Ref { col, .. } => out.push(col.clone()),
            Col::Union { type_col, id_col, .. } => {
                out.push(type_col.clone());
                out.push(id_col.clone());
            }
            Col::Snapshot { id_col, ver_col, .. } => {
                out.push(id_col.clone());
                out.push(ver_col.clone());
            }
            Col::Inverse { .. } => {}
        }
    }
    out.sort();
    out.dedup();
    out
}

impl Planner<'_> {
    pub fn publishable(&mut self, entity: &str) -> Vec<CommandPlan> {
        let i = form_intents::publishable(self.core, entity);
        vec![self.publish(entity, &i.publish), self.discard(entity, &i.discard)]
    }

    /// Loads and locks the row, pins its tenant and checks who may publish it.
    fn publish_prelude(&mut self, c: &mut Compiler<'_>, entity: &str, intent: &FormIntent) -> (Vec<ParamSpec>, Vec<Step>) {
        let subject = &intent.params[0];
        let params = self.params(c, &intent.params);
        let written: BTreeSet<String> = [subject.name.clone()].into();
        let mut steps = self.loads(c, std::slice::from_ref(subject), &written, true);
        steps.extend(self.tenant_steps(c, std::slice::from_ref(subject), false, true));
        let row = Val::Id { entity: entity.to_string(), sql: typed_marker(&subject.name, &Ty::Uuid) };
        let by = self.core.entities.get(entity).and_then(|e| e.traits.publish_by.as_ref());
        let cond = match by {
            Some(by) => c.row_pred(row, by),
            None => "false".to_string(),
        };
        let full = match c.superuser_sql() {
            Some(su) => format!("({cond} OR {su})"),
            None => cond,
        };
        steps.push(Step::Check { kind: CheckKind::Allow, sql: sql(format!("SELECT {full}")), code: None });
        (params, steps)
    }

    fn publish(&mut self, entity: &str, intent: &FormIntent) -> CommandPlan {
        self.begin();
        let mut c = self.compiler();
        c.push();
        let (params, mut steps) = self.publish_prelude(&mut c, entity, intent);
        let t = self.s.table(entity).clone();
        let (main, published) = (q(&t.table), q(&published_table(&t.table)));
        let cols = columns(&t);
        let list = cols.iter().map(|c| q(c)).collect::<Vec<_>>().join(", ");
        let updates = cols.iter().filter(|c| *c != "id").map(|c| format!("{0} = EXCLUDED.{0}", q(c))).collect::<Vec<_>>().join(", ");
        let id = typed_marker(&intent.params[0].name, &Ty::Uuid);
        steps.push(Step::Exec {
            sql: sql(format!(
                "INSERT INTO {published} ({list}, \"published_at\") SELECT {list}, now() FROM {main} WHERE \"id\" = {id} \
                 ON CONFLICT (\"id\") DO UPDATE SET {updates}, \"published_at\" = now()"
            )),
            bind: None,
            label: format!("publish {entity}"),
        });
        c.pop();
        self.absorb(&mut c);
        let returns = sql(format!("SELECT jsonb_build_object('publishedAt', \"published_at\") FROM {published} WHERE \"id\" = {id}"));
        let mut plan = self.finish(intent, params, steps, Some(returns));
        let time = serde_json::to_value(crate::ty::ty_spec(&Ty::Time)).unwrap_or_default();
        plan.output = json!({"kind": "object", "fields": {"publishedAt": time}});
        plan
    }

    fn discard(&mut self, entity: &str, intent: &FormIntent) -> CommandPlan {
        self.begin();
        let mut c = self.compiler();
        c.push();
        let (params, mut steps) = self.publish_prelude(&mut c, entity, intent);
        let t = self.s.table(entity).clone();
        let (main, published) = (q(&t.table), q(&published_table(&t.table)));
        let id = typed_marker(&intent.params[0].name, &Ty::Uuid);
        // nothing published yet: there is no version to go back to, and silently keeping the draft would hide that
        steps.push(Step::Let {
            name: "__published".into(),
            sql: sql(format!("SELECT \"id\"::text FROM {published} WHERE \"id\" = {id}")),
            code: Some(NOT_PUBLISHED.into()),
        });
        let sets = columns(&t).iter().filter(|c| *c != "id").map(|c| format!("{0} = p.{0}", q(c))).collect::<Vec<_>>().join(", ");
        steps.push(Step::Exec {
            sql: sql(format!("UPDATE {main} SET {sets} FROM {published} p WHERE {main}.\"id\" = p.\"id\" AND {main}.\"id\" = {id}")),
            bind: None,
            label: format!("discard the draft of {entity}"),
        });
        c.pop();
        self.absorb(&mut c);
        self.finish(intent, params, steps, None)
    }
}
