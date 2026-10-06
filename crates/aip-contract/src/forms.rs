//! Output shapes of the intents that L3 forms stand for. Which intents there
//! are, their parameters and their facts come from `aip_ir::form_intents`; a
//! form is not an intent in Core IR, but clients call what it lowers to.

use crate::ty::inferred;
use crate::{CommandView, Item, QueryView, command_json, params, query_json};
use aip_ir as ir;
use aip_ir::form_intents::{self as fi, FormIntent};
use serde_json::{Value, json};
use std::collections::BTreeMap;

type Items = BTreeMap<String, Item>;

fn text() -> ir::Type {
    ir::Type::Text { min: None, max: None, trim: false, lower: false, pattern: None }
}

fn spec(t: ir::Type) -> Value {
    serde_json::to_value(inferred(&t)).unwrap_or(Value::Null)
}

fn nullable(t: ir::Type) -> Value {
    let mut v = spec(t);
    if let Some(m) = v.as_object_mut() {
        m.insert("nullable".into(), json!(true));
    }
    v
}

fn int() -> ir::Type {
    ir::Type::Int { min: None, max: None }
}

fn command(i: &FormIntent, output: Value) -> (String, Item) {
    command_audited(i, output, false)
}

fn command_audited(i: &FormIntent, output: Value, audited: bool) -> (String, Item) {
    let ps = params(&i.params);
    let view = CommandView { params: &ps, output, idempotent: false, derived_key: false, audited, facts: &i.facts, emits: &[] };
    (i.name.clone(), Item::Command(command_json(view)))
}

fn status_query(i: &FormIntent, output: Value) -> (String, Item) {
    let ps = params(&i.params);
    let view = QueryView { params: &ps, single: true, output, page: None, cache_seconds: None, errors: &i.facts.errors, search: None };
    (i.name.clone(), Item::Query(query_json(view)))
}

pub(crate) fn add(core: &ir::Program, form: &ir::Form, out: &mut Items) {
    let mut put = |(name, item): (String, Item)| {
        out.insert(name, item);
    };
    match form {
        ir::Form::GrantLink(g) => {
            let i = fi::grant_link(core, g);
            put(command(&i.issue, Value::Null));
            put(command(&i.redeem, Value::Null));
        }
        ir::Form::Verification(v) => {
            let i = fi::verification(core, v);
            put(command(&i.request, Value::Null));
            put(command(&i.verify, Value::Null));
        }
        ir::Form::Approval(a) => {
            let i = fi::approval(core, a);
            for c in [&i.request, &i.approve, &i.reject, &i.cancel] {
                put(command(c, Value::Null));
            }
            put(status_query(&i.status, approval_status_output()));
        }
        ir::Form::Consent(c) => {
            let i = fi::consent(core, c);
            put(command(&i.give, consent_output()));
            put(command(&i.withdraw, consent_output()));
            put(status_query(&i.status, consent_output()));
        }
        ir::Form::Impersonate(x) => {
            let i = fi::impersonation_intents(core, x);
            // `token` is signed by the HTTP layer from the session the command opens
            let started = json!({"kind": "object", "fields": {
                "session": spec(ir::Type::Uuid),
                "token": spec(text()),
                "target": spec(ir::Type::Uuid),
                "expiresAt": spec(ir::Type::Time),
            }});
            put(command_audited(&i.start, started, true));
            put(command_audited(&i.stop, json!({"kind": "object", "fields": {"ended": spec(int())}}), true));
        }
        ir::Form::Job(j) => {
            let i = fi::job(core, j);
            let started = json!({"kind": "object", "fields": {"job": {"kind": "uuid"}, "status": {"kind": "enum", "name": "JobStatus"}}});
            put(command(&i.start, started));
            put(status_query(&i.status, job_status_output()));
        }
        _ => {}
    }
}

/// `PublishE` / `DiscardEDraft` of a publishable entity.
pub(crate) fn add_publishable(core: &ir::Program, entity: &str, out: &mut Items) {
    let i = fi::publishable(core, entity);
    let (name, item) = command(&i.publish, json!({"kind": "object", "fields": {"publishedAt": spec(ir::Type::Time)}}));
    out.insert(name, item);
    let (name, item) = command(&i.discard, Value::Null);
    out.insert(name, item);
}

fn consent_output() -> Value {
    json!({"kind": "object", "fields": {
        "name": spec(text()),
        "version": spec(int()),
        "status": {"kind": "enum", "name": "ConsentStatus"},
        "givenVersion": nullable(int()),
        "givenAt": nullable(ir::Type::Time),
        "withdrawnAt": nullable(ir::Type::Time),
    }})
}

fn approval_status_output() -> Value {
    json!({"kind": "object", "fields": {
        "status": {"kind": "enum", "name": "ApprovalStatus"},
        "required": spec(int()),
        "approvals": spec(int()),
        "requestedBy": nullable(ir::Type::Uuid),
        "requestedAt": nullable(ir::Type::Time),
        "expiresAt": nullable(ir::Type::Time),
        "decidedAt": nullable(ir::Type::Time),
        "canVote": spec(ir::Type::Bool),
        "votes": {"kind": "list", "of": {"kind": "object", "fields": {
            "voter": spec(ir::Type::Uuid),
            "decision": {"kind": "enum", "name": "ApprovalDecision"},
            "comment": nullable(text()),
            "at": spec(ir::Type::Time),
        }}},
    }})
}

fn job_status_output() -> Value {
    json!({"kind": "object", "fields": {
        "status": {"kind": "enum", "name": "JobStatus"},
        "total": nullable(int()),
        "done": spec(int()),
        "file": {"kind": "url", "nullable": true},
        "error": nullable(text()),
        "createdAt": spec(ir::Type::Time),
        "startedAt": nullable(ir::Type::Time),
        "finishedAt": nullable(ir::Type::Time),
    }})
}
