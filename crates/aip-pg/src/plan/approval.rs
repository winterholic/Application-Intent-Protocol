//! `approval X for E r { ... }` lowers to four commands and one query over the
//! internal `_aip_approval` / `_aip_approval_vote` tables:
//!
//! - `RequestX(e)` opens a pending request (one per subject; stale ones expire first)
//! - `ApproveX(e, comment?)` records a vote; the N-th approval runs `on approved`
//! - `RejectX(e, reason?)` records a veto and runs `on rejected`
//! - `CancelX(e)` lets the requester withdraw
//! - `XStatus(e)` reports progress and whether the caller may vote
//!
//! Votes on one request are serialized by locking its row, so "the N-th
//! approval" is exact under concurrency.

use super::{Planner, sql};
use crate::names::lit;
use crate::schema::Col;
use crate::sqlexpr::{ACTOR, Compiler, SetElem, marker, typed_marker};
use crate::ty::Ty;
use aip_ir as ir;
use aip_ir::form_intents::{self, FormIntent};
use aip_plan::*;
use serde_json::json;
use std::collections::BTreeSet;

const APPR: &str = "__approval";

impl Planner<'_> {
    pub fn approval(&mut self, a: &ir::Approval) -> (Vec<CommandPlan>, QueryPlan) {
        let i = form_intents::approval(self.core, a);
        let commands = vec![
            self.approval_request(a, &i.request),
            self.approval_vote(a, &i.approve, true),
            self.approval_vote(a, &i.reject, false),
            self.approval_cancel(a, &i.cancel),
        ];
        (commands, self.approval_status(a, &i.status))
    }

    /// Binds the subject parameter under the declaration's alias and loads it.
    /// `lock`: None for read-only queries, Some(write) for commands.
    fn approval_prelude(&mut self, c: &mut Compiler<'_>, a: &ir::Approval, intent: &FormIntent, lock: Option<bool>) -> (Vec<ParamSpec>, Vec<Step>) {
        let subject = &intent.params[0];
        let params = self.params(c, &intent.params);
        let written: BTreeSet<String> = if lock == Some(true) { [subject.name.clone()].into() } else { BTreeSet::new() };
        let mut steps = self.loads(c, std::slice::from_ref(subject), &written, lock.is_some());
        // the row under approval fixes the tenant of the approvers and of what the decision writes
        steps.extend(self.tenant_steps(c, std::slice::from_ref(subject), false, lock.is_some()));
        c.bind(&a.alias, self.param_val(&subject.name, &Ty::Entity(a.entity.clone())));
        (params, steps)
    }

    /// `EXISTS (approver row linked to the actor)`.
    fn approver_sql(&mut self, c: &mut Compiler<'_>, a: &ir::Approval) -> String {
        let parts = c.begin_set(&a.approvers);
        c.end_set();
        let actor_entity = self.actor_entity().map(String::from).unwrap_or_default();
        let link = match &parts.elem {
            SetElem::Entity(e) if *e == actor_entity => Some("\"id\"".to_string()),
            SetElem::Entity(e) => self.s.table(e).cols.values().find_map(|col| match col {
                Col::Ref { col, target } if *target == actor_entity => Some(crate::names::q(col)),
                _ => None,
            }),
            _ => None,
        };
        let Some(link) = link else {
            self.fail("approvers must be linked to the actor");
            return "false".into();
        };
        let mut conds = parts.conds.clone();
        conds.push(format!("{}.{link} = {}", parts.alias, typed_marker(ACTOR, &Ty::Uuid)));
        format!("EXISTS (SELECT 1 FROM {}{})", parts.from, Compiler::where_sql(&conds))
    }

    /// Locks the pending request; an expired one is closed (and that sticks) before failing.
    fn pending_steps(&mut self, a: &ir::Approval, subject: &str) -> Vec<Step> {
        let mut steps = vec![Step::Let {
            name: APPR.into(),
            sql: sql(format!(
                "SELECT \"id\"::text FROM \"_aip_approval\" WHERE \"name\" = {} AND \"subject\" = {} AND \"status\" = 'PENDING' FOR UPDATE",
                lit(&a.name),
                typed_marker(subject, &Ty::Uuid)
            )),
            code: Some("APPROVAL_NOT_PENDING".into()),
        }];
        if a.expires_seconds.is_some() {
            steps.push(Step::When {
                cond: sql(format!("SELECT \"expires_at\" <= now() FROM \"_aip_approval\" WHERE \"id\" = ({}::text)::uuid", marker(APPR))),
                steps: vec![
                    Step::Exec {
                        sql: sql(format!(
                            "UPDATE \"_aip_approval\" SET \"status\" = 'EXPIRED', \"decided_at\" = now() WHERE \"id\" = ({}::text)::uuid",
                            marker(APPR)
                        )),
                        bind: None,
                        label: "expire request".into(),
                    },
                    Step::Fail { code: "APPROVAL_EXPIRED".into(), commit: true },
                ],
            });
        }
        steps
    }

    fn approval_request(&mut self, a: &ir::Approval, intent: &FormIntent) -> CommandPlan {
        let subject = &intent.params[0];
        self.begin();
        let mut c = self.compiler();
        c.push();
        let (params, mut steps) = self.approval_prelude(&mut c, a, intent, Some(false));
        let actor = typed_marker(ACTOR, &Ty::Uuid);
        steps.push(Step::Check { kind: CheckKind::Allow, sql: sql(format!("SELECT {actor} IS NOT NULL")), code: None });
        if let Some(r) = &a.requested_by {
            steps.push(self.allow_step(&mut c, &ir::Policy { cond: r.clone(), code: None }));
        }
        let name = lit(&a.name);
        let subj = typed_marker(&subject.name, &Ty::Uuid);
        steps.push(Step::Exec {
            sql: sql(format!(
                "UPDATE \"_aip_approval\" SET \"status\" = 'EXPIRED', \"decided_at\" = now() WHERE \"name\" = {name} AND \"subject\" = {subj} AND \"status\" = 'PENDING' AND \"expires_at\" <= now()"
            )),
            bind: None,
            label: "close expired request".into(),
        });
        let expires = match &a.expires_seconds {
            Some(d) => format!("now() + make_interval(secs => {})", (*d).max(0) as u64),
            None => "NULL".into(),
        };
        steps.push(Step::Exec {
            sql: sql(format!(
                "INSERT INTO \"_aip_approval\" (\"name\", \"subject\", \"requested_by\", \"required\", \"expires_at\") VALUES ({name}, {subj}, {actor}, {}, {expires})",
                a.required
            )),
            bind: None,
            label: format!("open {}", a.name),
        });
        c.pop();
        self.absorb(&mut c);
        self.finish(intent, params, steps, None)
    }

    fn approval_vote(&mut self, a: &ir::Approval, intent: &FormIntent, approve: bool) -> CommandPlan {
        let subject = &intent.params[0];
        self.begin();
        let mut c = self.compiler();
        c.push();
        let (params, mut steps) = self.approval_prelude(&mut c, a, intent, Some(true));
        let actor = typed_marker(ACTOR, &Ty::Uuid);
        // a vote is the voter's own act
        steps.extend(self.not_impersonating());
        steps.extend(self.pending_steps(a, &subject.name));
        let is_approver = self.approver_sql(&mut c, a);
        steps.push(Step::Check { kind: CheckKind::Allow, sql: sql(format!("SELECT {is_approver}")), code: Some("NOT_AN_APPROVER".into()) });
        if a.no_self {
            steps.push(Step::Check {
                kind: CheckKind::Allow,
                sql: sql(format!(
                    "SELECT \"requested_by\" IS DISTINCT FROM {actor} FROM \"_aip_approval\" WHERE \"id\" = ({}::text)::uuid",
                    marker(APPR)
                )),
                code: Some("SELF_APPROVAL_FORBIDDEN".into()),
            });
        }
        steps.push(Step::Exec {
            sql: sql(format!(
                "INSERT INTO \"_aip_approval_vote\" (\"approval\", \"voter\", \"decision\", \"comment\") VALUES (({}::text)::uuid, {actor}, {}, ({}::text))",
                marker(APPR),
                if approve { "'APPROVE'" } else { "'REJECT'" },
                marker(&intent.params[1].name)
            )),
            bind: None,
            label: "record vote".into(),
        });
        let (status, block) = if approve { ("APPROVED", &a.on_approved) } else { ("REJECTED", &a.on_rejected) };
        let mut decided = vec![Step::Exec {
            sql: sql(format!(
                "UPDATE \"_aip_approval\" SET \"status\" = '{status}', \"decided_at\" = now() WHERE \"id\" = ({}::text)::uuid",
                marker(APPR)
            )),
            bind: None,
            label: format!("mark {}", status.to_lowercase()),
        }];
        self.stmts(&mut c, block, &mut decided);
        if approve {
            steps.push(Step::When {
                cond: sql(format!(
                    "SELECT count(*) >= {} FROM \"_aip_approval_vote\" WHERE \"approval\" = ({}::text)::uuid AND \"decision\" = 'APPROVE'",
                    a.required,
                    marker(APPR)
                )),
                steps: decided,
            });
        } else {
            steps.extend(decided);
        }
        c.pop();
        self.absorb(&mut c);
        self.finish(intent, params, steps, None)
    }

    fn approval_cancel(&mut self, a: &ir::Approval, intent: &FormIntent) -> CommandPlan {
        let subject = &intent.params[0];
        self.begin();
        let mut c = self.compiler();
        c.push();
        let (params, mut steps) = self.approval_prelude(&mut c, a, intent, Some(false));
        steps.extend(self.pending_steps(a, &subject.name));
        let su = c.superuser_sql().map(|s| format!(" OR {s}")).unwrap_or_default();
        steps.push(Step::Check {
            kind: CheckKind::Allow,
            sql: sql(format!(
                "SELECT coalesce(\"requested_by\" = {}, false){su} FROM \"_aip_approval\" WHERE \"id\" = ({}::text)::uuid",
                typed_marker(ACTOR, &Ty::Uuid),
                marker(APPR)
            )),
            code: Some("ONLY_REQUESTER_CAN_CANCEL".into()),
        });
        steps.push(Step::Exec {
            sql: sql(format!(
                "UPDATE \"_aip_approval\" SET \"status\" = 'CANCELLED', \"decided_at\" = now() WHERE \"id\" = ({}::text)::uuid",
                marker(APPR)
            )),
            bind: None,
            label: "cancel request".into(),
        });
        c.pop();
        self.absorb(&mut c);
        self.finish(intent, params, steps, None)
    }

    fn approval_status(&mut self, a: &ir::Approval, intent: &FormIntent) -> QueryPlan {
        self.begin();
        let mut c = self.compiler();
        c.push();
        let (params, prelude) = self.approval_prelude(&mut c, a, intent, None);
        let actor = typed_marker(ACTOR, &Ty::Uuid);
        let is_approver = self.approver_sql(&mut c, a);
        let not_self = if a.no_self { format!(" AND ap.\"requested_by\" IS DISTINCT FROM {actor}") } else { String::new() };
        let main = format!(
            "SELECT coalesce((SELECT jsonb_build_object(\
             'status', CASE WHEN ap.\"status\" = 'PENDING' AND ap.\"expires_at\" <= now() THEN 'EXPIRED' ELSE ap.\"status\" END, \
             'required', ap.\"required\", \
             'approvals', (SELECT count(*) FROM \"_aip_approval_vote\" v WHERE v.\"approval\" = ap.\"id\" AND v.\"decision\" = 'APPROVE'), \
             'requestedBy', ap.\"requested_by\", 'requestedAt', ap.\"created_at\", 'expiresAt', ap.\"expires_at\", 'decidedAt', ap.\"decided_at\", \
             'canVote', ap.\"status\" = 'PENDING' AND coalesce(ap.\"expires_at\" > now(), true) AND {is_approver}{not_self} \
               AND NOT EXISTS (SELECT 1 FROM \"_aip_approval_vote\" v WHERE v.\"approval\" = ap.\"id\" AND v.\"voter\" = {actor}), \
             'votes', coalesce((SELECT jsonb_agg(jsonb_build_object('voter', v.\"voter\", 'decision', v.\"decision\", 'comment', v.\"comment\", 'at', v.\"at\") ORDER BY v.\"at\") \
               FROM \"_aip_approval_vote\" v WHERE v.\"approval\" = ap.\"id\"), '[]'::jsonb)) \
             FROM \"_aip_approval\" ap WHERE ap.\"name\" = {} AND ap.\"subject\" = {} ORDER BY ap.\"created_at\" DESC LIMIT 1), \
             jsonb_build_object('status', 'NONE', 'required', {}, 'approvals', 0, 'requestedBy', NULL, 'requestedAt', NULL, 'expiresAt', NULL, 'decidedAt', NULL, 'canVote', false, 'votes', '[]'::jsonb))",
            lit(&a.name),
            typed_marker(&intent.params[0].name, &Ty::Uuid),
            a.required
        );
        c.pop();
        self.absorb(&mut c);
        let spec = |t: &Ty| serde_json::to_value(crate::ty::ty_spec(t)).unwrap_or_default();
        let nullable = |t: &Ty| {
            let mut v = spec(t);
            if let Some(m) = v.as_object_mut() {
                m.insert("nullable".into(), json!(true));
            }
            v
        };
        let output = json!({"kind": "object", "fields": {
            "status": {"kind": "enum", "name": "ApprovalStatus"},
            "required": spec(&Ty::Int),
            "approvals": spec(&Ty::Int),
            "requestedBy": nullable(&Ty::Uuid),
            "requestedAt": nullable(&Ty::Time),
            "expiresAt": nullable(&Ty::Time),
            "decidedAt": nullable(&Ty::Time),
            "canVote": spec(&Ty::Bool),
            "votes": {"kind": "list", "of": {"kind": "object", "fields": {
                "voter": spec(&Ty::Uuid),
                "decision": {"kind": "enum", "name": "ApprovalDecision"},
                "comment": nullable(&Ty::Text),
                "at": spec(&Ty::Time),
            }}},
        }});
        QueryPlan {
            name: intent.name.clone(),
            params,
            internal: false,
            prelude,
            variants: vec![QueryVariant { when: None, main: sql(main), key_count: 0 }],
            sort_param: None,
            single: true,
            page: None,
            cache_seconds: None,
            touches: Vec::new(),
            rate_limits: Vec::new(),
            errors: super::error_specs(&intent.facts),
            output,
            decrypt: Vec::new(),
        }
    }
}
