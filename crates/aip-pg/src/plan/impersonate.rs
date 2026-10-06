//! `impersonate E by <cond> audited reason required ttl <dur>` lowers to two commands over the internal
//! `_aip_impersonation` table:
//!
//! - `StartEImpersonation(target, reason)` opens a session for the operator and returns its id and expiry; the HTTP
//!   layer signs the session into a token whose actor is the target
//! - `StopEImpersonation()` ends the session of the call (or, from the operator's own token, all of theirs)
//!
//! What the runtime does for a call that carries a session is in `aip-runtime` (the session must be open and
//! unexpired, every command is audited with both people); what the plans do is the rules below.
//!
//! Rules a session cannot get around, all checked in the database against the session id the engine puts in `__imp`:
//! - nesting: starting from inside a session fails
//! - the target is not the operator, and is not someone the condition would also let impersonate (or a superuser),
//!   so a session never lends more power than the operator has
//! - the superuser bypass is switched off for the session (see `Compiler::superuser_sql`)
//! - commands that read out or remove the person's data, give or withdraw consent and vote in approvals are refused

use super::{Planner, sql};
use crate::sqlexpr::{ACTOR, IMPERSONATION, finalize, marker, typed_marker};
use crate::ty::Ty;
use aip_ir as ir;
use aip_ir::form_intents::{self, FormIntent, IMPERSONATION_FORBIDDEN, IMPERSONATION_NESTED, IMPERSONATION_SELF, IMPERSONATION_TARGET_PRIVILEGED};
use aip_plan::*;
use serde_json::json;
use std::collections::BTreeSet;

const SESSION: &str = "__session";

impl Planner<'_> {
    pub fn impersonation(&mut self, i: &ir::Impersonate) -> Vec<CommandPlan> {
        let intents = form_intents::impersonation_intents(self.core, i);
        vec![self.impersonation_start(i, &intents.start), self.impersonation_stop(&intents.stop)]
    }

    /// `by` (or the superuser test) asked about the session's operator, as boolean SQL that reads the operator from
    /// the CTE `s` the runtime wraps around it. Re-asked on every call of a session, not only when it opens.
    pub fn operator_check(&mut self, i: &ir::Impersonate) -> Sql {
        let mut c = self.compiler();
        let by = c.pred(&i.by);
        let su = c.superuser_sql();
        self.absorb(&mut c);
        let text = match su {
            Some(su) => format!("({by} OR {su})"),
            None => by,
        };
        // the operator acts as themselves here, so the session marker is NULL and the actor is the session's operator
        let text = text.replace(&marker(IMPERSONATION), "NULL").replace(&marker(ACTOR), "(SELECT \"operator\"::text FROM s)");
        finalize(&format!("SELECT coalesce({text}, false)"))
    }

    /// The step that refuses a caller acting as someone else; `None` in a program that has no impersonation.
    pub(super) fn not_impersonating(&self) -> Option<Step> {
        form_intents::impersonation(self.core).map(|_| Step::Check {
            kind: CheckKind::Allow,
            sql: sql(format!("SELECT ({}::text) IS NULL", marker(IMPERSONATION))),
            code: Some(IMPERSONATION_FORBIDDEN.into()),
        })
    }

    fn impersonation_start(&mut self, i: &ir::Impersonate, intent: &FormIntent) -> CommandPlan {
        self.begin();
        let target = &intent.params[0];
        let mut c = self.compiler();
        c.push();
        let params = self.params(&mut c, &intent.params);
        let mut steps = vec![Step::Check {
            kind: CheckKind::Allow,
            sql: sql(format!("SELECT ({}::text) IS NULL", marker(IMPERSONATION))),
            code: Some(IMPERSONATION_NESTED.into()),
        }];
        steps.extend(self.loads(&mut c, std::slice::from_ref(target), &BTreeSet::new(), false));
        steps.extend(self.tenant_steps(&mut c, std::slice::from_ref(target), false, false));
        steps.push(Self::authenticated_check());
        steps.push(self.allow_step(&mut c, &ir::Policy { cond: i.by.clone(), code: None }));
        let actor = typed_marker(ACTOR, &Ty::Uuid);
        let target_id = typed_marker(&target.name, &Ty::Uuid);
        steps.push(Step::Check {
            kind: CheckKind::Allow,
            sql: sql(format!("SELECT {target_id} IS DISTINCT FROM {actor}")),
            code: Some(IMPERSONATION_SELF.into()),
        });
        // the same condition and superuser test, asked about the target instead of the caller
        let by = c.pred(&i.by);
        let su = c.superuser_sql();
        let as_target = |text: String| text.replace(&marker(ACTOR), &marker(&target.name));
        let privileged = match su {
            Some(su) => format!("({} OR {})", as_target(by), as_target(su)),
            None => as_target(by),
        };
        steps.push(Step::Check {
            kind: CheckKind::Allow,
            sql: sql(format!("SELECT NOT coalesce({privileged}, false)")),
            code: Some(IMPERSONATION_TARGET_PRIVILEGED.into()),
        });
        steps.push(Step::Exec {
            sql: sql(format!(
                "INSERT INTO \"_aip_impersonation\" (\"operator\", \"target\", \"reason\", \"expires_at\") VALUES ({actor}, {target_id}, ({}::text), now() + make_interval(secs => {})) RETURNING \"id\"",
                marker(&intent.params[1].name),
                i.ttl_seconds.max(1)
            )),
            bind: Some(SESSION.into()),
            label: format!("open {} impersonation", i.entity),
        });
        c.pop();
        self.absorb(&mut c);
        let returns = sql(format!(
            "SELECT jsonb_build_object('session', \"id\", 'target', \"target\", 'expiresAt', \"expires_at\") FROM \"_aip_impersonation\" WHERE \"id\" = ({}::text)::uuid",
            marker(SESSION)
        ));
        let mut plan = self.finish(intent, params, steps, Some(returns));
        // `impersonate ... audited`: the start is on record in the audit log as well as in the session row
        plan.audited = true;
        let spec = |t: &Ty| serde_json::to_value(crate::ty::ty_spec(t)).unwrap_or_default();
        // `token` is added by the HTTP layer, which holds the signing secret
        plan.output = json!({"kind": "object", "fields": {
            "session": spec(&Ty::Uuid),
            "token": spec(&Ty::Text),
            "target": spec(&Ty::Uuid),
            "expiresAt": spec(&Ty::Time),
        }});
        plan
    }

    fn authenticated_check() -> Step {
        Step::Check { kind: CheckKind::Allow, sql: sql(format!("SELECT {} IS NOT NULL", typed_marker(ACTOR, &Ty::Uuid))), code: None }
    }

    fn impersonation_stop(&mut self, intent: &FormIntent) -> CommandPlan {
        self.begin();
        let actor = typed_marker(ACTOR, &Ty::Uuid);
        let steps = vec![Self::authenticated_check()];
        // inside a session: that session. With the operator's own token: every session of theirs that is still open
        let returns = sql(format!(
            "WITH ended AS (UPDATE \"_aip_impersonation\" SET \"ended_at\" = now() WHERE \"ended_at\" IS NULL AND \"expires_at\" > now() \
             AND (\"id\" = ({imp}::text)::uuid OR (({imp}::text) IS NULL AND \"operator\" = {actor})) RETURNING 1) \
             SELECT jsonb_build_object('ended', (SELECT count(*) FROM ended))",
            imp = marker(IMPERSONATION)
        ));
        let mut plan = self.finish(intent, Vec::new(), steps, Some(returns));
        plan.audited = true;
        plan.output = json!({"kind": "object", "fields": {"ended": serde_json::to_value(crate::ty::ty_spec(&Ty::Int)).unwrap_or_default()}});
        plan
    }
}
