//! `subscribe X(params) { allow ...; from ...; where ...; select ... }` and `migration X { ... }`.
//!
//! A subscription is planned as the list query it stands for, with the checks of any query (allow, visibility, field
//! visibility, tenant filter), plus the tables that query reads: the runtime re-runs it when one of them changes.
//! A migration is planned like the body of a schedule, with the tenant pin lifted: nobody is acting and it has to be
//! able to rewrite rows of every tenant.

use super::Planner;
use crate::names::q;
use aip_ir as ir;
use aip_plan::*;
use std::collections::BTreeSet;

const NOTIFY_FN: &str = "_aip_notify_changed";

impl Planner<'_> {
    pub fn subscription(&mut self, s: &ir::Subscribe) -> Subscription {
        let qd = s.as_query();
        let query = self.query(&s.name, &qd);
        let reads = self.tables_in(&query);
        Subscription { name: s.name.clone(), query, reads }
    }

    /// The tables whose name appears in the plan's SQL as a table (a quoted name not following a `.`, which is a column of an alias).
    /// Read from the SQL itself, so a table reached through a relation, a visibility rule or a tenant path is not forgotten.
    fn tables_in(&self, query: &QueryPlan) -> Vec<String> {
        let mut texts: Vec<&str> = query.variants.iter().map(|v| v.main.text.as_str()).collect();
        let mut stack: Vec<&[Step]> = vec![&query.prelude];
        while let Some(steps) = stack.pop() {
            for st in steps {
                match st {
                    Step::Load { sql, .. } | Step::Lock { sql } | Step::Let { sql, .. } | Step::Check { sql, .. } => texts.push(&sql.text),
                    Step::When { cond, steps } => {
                        texts.push(&cond.text);
                        stack.push(steps);
                    }
                    _ => {}
                }
            }
        }
        let tables: BTreeSet<&str> = self.s.tables.values().map(|t| t.table.as_str()).collect();
        let mut out = BTreeSet::new();
        for text in texts {
            for name in &tables {
                let needle = q(name);
                let mut from = 0;
                while let Some(i) = text[from..].find(&needle) {
                    let at = from + i;
                    if !text[..at].ends_with('.') {
                        out.insert((*name).to_string());
                        break;
                    }
                    from = at + needle.len();
                }
            }
        }
        out.into_iter().collect()
    }

    pub fn migration(&mut self, m: &ir::Migration) -> Migration {
        let mut c = self.compiler();
        c.push();
        let mut steps: Vec<Step> = self.context_pin(&c, true).into_iter().collect();
        self.stmts(&mut c, &m.body, &mut steps);
        c.pop();
        self.absorb(&mut c);
        Migration { name: m.name.clone(), digest: m.digest(), steps }
    }
}

/// The function and the statement triggers that announce a commit touching a table some subscription reads. NOTIFY is
/// delivered when the transaction commits, so a rolled back change announces nothing.
pub fn notify_ddl(tables: &BTreeSet<String>) -> Vec<String> {
    if tables.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!(
        "CREATE OR REPLACE FUNCTION {NOTIFY_FN}() RETURNS trigger LANGUAGE plpgsql AS $aip$ BEGIN PERFORM pg_notify('{CHANGE_CHANNEL}', TG_TABLE_NAME); RETURN NULL; END $aip$"
    )];
    for t in tables {
        out.push(format!(
            "CREATE TRIGGER {} AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON {} FOR EACH STATEMENT EXECUTE FUNCTION {NOTIFY_FN}()",
            q(&format!("{t}__notify")),
            q(t)
        ));
    }
    out
}
