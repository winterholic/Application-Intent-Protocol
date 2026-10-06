//! `consent X version N required for [A, B]` lowers to two commands and a query over the internal
//! `_aip_consent` table, and to one check at the start of every listed intent:
//!
//! - `GiveXConsent()` records consent to the current version (a second call changes nothing)
//! - `WithdrawXConsent()` ends every active consent record of the caller
//! - `XConsentStatus()` says whether the caller has consented to the current version
//!
//! A record is append-only history: who, which version, when given, when withdrawn. Raising the version in
//! the program makes older records stop counting without touching them; the check compares the version.

use super::{Planner, sql};
use crate::names::lit;
use crate::sqlexpr::{ACTOR, typed_marker};
use crate::ty::Ty;
use aip_ir as ir;
use aip_ir::form_intents::{self, FormIntent};
use aip_plan::*;
use serde_json::json;

impl Planner<'_> {
    pub fn consent(&mut self, c: &ir::Consent) -> (Vec<CommandPlan>, QueryPlan) {
        let i = form_intents::consent(self.core, c);
        let give = self.consent_write(c, &i.give, true);
        let withdraw = self.consent_write(c, &i.withdraw, false);
        (vec![give, withdraw], self.consent_status(&i.status, c))
    }

    /// The checks an intent starts with when a consent lists it, in declaration order of the consents.
    pub(super) fn consent_steps(&self, intent: &str) -> Vec<Step> {
        form_intents::consents_of(self.core, intent)
            .into_iter()
            .map(|c| Step::Check {
                kind: CheckKind::Consent,
                sql: sql(format!(
                    "SELECT EXISTS (SELECT 1 FROM \"_aip_consent\" WHERE \"name\" = {} AND \"actor\" = {} AND \"version\" = {} AND \"withdrawn_at\" IS NULL)",
                    lit(&c.name),
                    typed_marker(ACTOR, &Ty::Uuid),
                    c.version
                )),
                code: Some(c.name.clone()),
            })
            .collect()
    }

    fn authenticated_step() -> Step {
        Step::Check { kind: CheckKind::Allow, sql: sql(format!("SELECT {} IS NOT NULL", typed_marker(ACTOR, &Ty::Uuid))), code: None }
    }

    fn consent_write(&mut self, c: &ir::Consent, intent: &FormIntent, give: bool) -> CommandPlan {
        self.begin();
        let actor = typed_marker(ACTOR, &Ty::Uuid);
        let name = lit(&c.name);
        let mut steps = vec![Self::authenticated_step()];
        steps.extend(self.not_impersonating());
        steps.push(if give {
            // the partial unique index makes a repeated give a no-op instead of a second record
            Step::Exec {
                sql: sql(format!(
                    "INSERT INTO \"_aip_consent\" (\"name\", \"actor\", \"version\") VALUES ({name}, {actor}, {}) \
                     ON CONFLICT (\"name\", \"actor\", \"version\") WHERE \"withdrawn_at\" IS NULL DO NOTHING",
                    c.version
                )),
                bind: None,
                label: format!("record consent to {} v{}", c.name, c.version),
            }
        } else {
            Step::Exec {
                sql: sql(format!(
                    "UPDATE \"_aip_consent\" SET \"withdrawn_at\" = now() WHERE \"name\" = {name} AND \"actor\" = {actor} AND \"withdrawn_at\" IS NULL"
                )),
                bind: None,
                label: format!("withdraw consent to {}", c.name),
            }
        });
        let mut plan = self.finish(intent, Vec::new(), steps, Some(sql(Self::consent_status_sql(c))));
        plan.output = Self::consent_output();
        plan
    }

    fn consent_status(&mut self, intent: &FormIntent, c: &ir::Consent) -> QueryPlan {
        self.begin();
        QueryPlan {
            name: intent.name.clone(),
            params: Vec::new(),
            internal: false,
            prelude: vec![Self::authenticated_step()],
            variants: vec![QueryVariant { when: None, main: sql(Self::consent_status_sql(c)), key_count: 0 }],
            sort_param: None,
            single: true,
            page: None,
            cache_seconds: None,
            touches: Vec::new(),
            rate_limits: Vec::new(),
            errors: super::error_specs(&intent.facts),
            output: Self::consent_output(),
            decrypt: Vec::new(),
        }
    }

    /// GIVEN: an active record of the current version. Otherwise the caller's latest record says why not:
    /// WITHDRAWN, or OUTDATED (an older version), and NONE without any.
    fn consent_status_sql(c: &ir::Consent) -> String {
        let actor = typed_marker(ACTOR, &Ty::Uuid);
        let name = lit(&c.name);
        format!(
            "SELECT jsonb_build_object('name', {name}, 'version', {v}, \
             'status', CASE WHEN EXISTS (SELECT 1 FROM \"_aip_consent\" WHERE \"name\" = {name} AND \"actor\" = {actor} AND \"version\" = {v} AND \"withdrawn_at\" IS NULL) THEN 'GIVEN' \
               WHEN l.\"id\" IS NULL THEN 'NONE' WHEN l.\"withdrawn_at\" IS NOT NULL THEN 'WITHDRAWN' ELSE 'OUTDATED' END, \
             'givenVersion', l.\"version\", 'givenAt', l.\"given_at\", 'withdrawnAt', l.\"withdrawn_at\") \
             FROM (SELECT 1) d LEFT JOIN LATERAL (SELECT * FROM \"_aip_consent\" WHERE \"name\" = {name} AND \"actor\" = {actor} ORDER BY \"given_at\" DESC, \"id\" DESC LIMIT 1) l ON true",
            v = c.version
        )
    }

    fn consent_output() -> serde_json::Value {
        let spec = |t: &Ty| serde_json::to_value(crate::ty::ty_spec(t)).unwrap_or_default();
        let nullable = |t: &Ty| {
            let mut v = spec(t);
            if let Some(m) = v.as_object_mut() {
                m.insert("nullable".into(), json!(true));
            }
            v
        };
        json!({"kind": "object", "fields": {
            "name": spec(&Ty::Text),
            "version": spec(&Ty::Int),
            "status": {"kind": "enum", "name": "ConsentStatus"},
            "givenVersion": nullable(&Ty::Int),
            "givenAt": nullable(&Ty::Time),
            "withdrawnAt": nullable(&Ty::Time),
        }})
    }
}
