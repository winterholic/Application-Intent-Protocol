//! The client contract (`GET /aip/describe`) and the typed clients generated
//! from it. Everything here is derived from Core IR, the language's common
//! semantic model, so a program from any frontend yields the same contract
//! whatever backend runs it.

pub mod compat;
mod forms;
mod shape;
pub mod tsgen;
mod ty;

use aip_ir as ir;
use aip_ir::facts::{self, ErrorFact};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use ty::{ParamSpec, declared};

/// Version of the wire protocol and of the shape of the contract document;
/// separate from Core IR, which versions the meaning a contract is derived from.
pub const PROTOCOL_VERSION: &str = "aip-protocol/0.1";

/// A callable intent as a client sees it.
pub(crate) enum Item {
    Query(Value),
    Command(Value),
}

pub fn describe(core: &ir::Program) -> Value {
    let mut items: BTreeMap<String, Item> = BTreeMap::new();
    for (name, i) in &core.intents {
        match i {
            ir::Intent::Query(q) if !q.internal => {
                items.insert(name.clone(), Item::Query(query(core, name, q)));
            }
            ir::Intent::Command(c) if !c.internal => {
                items.insert(name.clone(), Item::Command(command(core, name, c)));
            }
            _ => {}
        }
    }
    // intents a form stands for are callable like declared ones; a form wins a name clash
    for f in &core.forms {
        forms::add(core, f, &mut items);
    }
    for (name, e) in &core.entities {
        if e.traits.publishable {
            forms::add_publishable(core, name, &mut items);
        }
    }
    let mut queries = serde_json::Map::new();
    let mut commands = serde_json::Map::new();
    for (name, item) in items {
        match item {
            Item::Query(v) => queries.insert(name, v),
            Item::Command(v) => commands.insert(name, v),
        };
    }
    let mut doc = json!({
        "protocol": PROTOCOL_VERSION,
        "core_ir": ir::CORE_IR_VERSION,
        "transport": {
            "call": "POST /aip/{intent}",
            "describe": "GET /aip/describe",
            "auth": "Authorization: Bearer <token>",
            "idempotency": "Idempotency-Key: <key>",
            "uploads": "multipart/form-data: part 'input' (JSON) + one part per file; the input refers to a file by its part name",
            "pagination": "cursor (keyset) or page (offset) in the input object"
        },
        "enums": enums(core),
        "records": records(core),
        "events": events(core),
        "queries": queries,
        "commands": commands,
        "jobs": jobs(core),
    });
    let subs = subscriptions(core);
    if !subs.is_empty()
        && let Some(m) = doc.as_object_mut()
    {
        m.insert("subscriptions".into(), Value::Object(subs));
        if let Some(t) = m.get_mut("transport").and_then(Value::as_object_mut) {
            t.insert("subscribe".into(), json!(SUBSCRIBE_TRANSPORT));
        }
    }
    let hooks = outbound_webhooks(core);
    if !hooks.is_empty()
        && let Some(m) = doc.as_object_mut()
    {
        m.insert("outbound_webhooks".into(), Value::Array(hooks));
    }
    mark_decimal_wire(&mut doc);
    doc
}

/// How a client opens a subscription; the message protocol is in `spec/grammar.md`.
const SUBSCRIBE_TRANSPORT: &str = "WebSocket GET /aip/subscribe: send {type:'auth', token} first, then {type:'subscribe', id, name, input}; receive {type:'snapshot', id, rows}, then {type:'changed', id, rows} (the whole result, only when it differs from the last one sent) or {type:'error', id, code}; send {type:'unsubscribe', id} to stop";

/// What a client may rely on for each `subscribe`: what to send, what a row looks like, and what the server promises.
fn subscriptions(core: &ir::Program) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    for f in &core.forms {
        let ir::Form::Subscribe(s) = f else { continue };
        let mut errors = facts::query_errors(core, &s.name, &s.as_query());
        for code in [ir::codes::AUTH_UNAUTHENTICATED, ir::codes::SUBSCRIPTION_LIMIT, ir::codes::SUBSCRIPTION_TOO_LARGE] {
            errors.insert(ErrorFact::new(code, None));
        }
        out.insert(
            s.name.clone(),
            json!({
                "input": params(&s.params),
                "output": shape::to_json(&s.output),
                "errors": errors,
                "guarantees": {
                    "consistency": "each update is one snapshot (repeatable read) of the whole result",
                    "delivery": "a snapshot first, then the whole result again after a change that alters it; an unchanged result is not sent, and changes closer together than the server's debounce may arrive as one update",
                    "permissions": "every update is computed for the subscriber with the allow condition, row and field visibility and the tenant filter of a query; when the subscriber loses access the server sends an error and ends that subscription",
                    "order": "rows by id",
                },
            }),
        );
    }
    out
}

/// What the receiver of an `outbound webhooks` form is sent and may rely on. Written for the people who build the
/// receiving server; the numbers of the form (retries, window, disabling) are in the operator contract.
fn outbound_webhooks(core: &ir::Program) -> Vec<Value> {
    core.forms
        .iter()
        .filter_map(|f| match f {
            ir::Form::OutboundWebhooks(o) => Some(o),
            _ => None,
        })
        .map(|o| {
            let events: BTreeMap<&String, Value> = o
                .events
                .iter()
                .map(|e| (e, json!(core.events.get(e).map(|d| d.fields.iter().map(|(n, t)| (n.clone(), ty::inferred(t))).collect::<BTreeMap<_, _>>()))))
                .collect();
            json!({
                "endpoint": o.entity,
                "events": events,
                "request": {
                    "method": "POST",
                    "content_type": "application/json",
                    "body": "{\"id\": \"evt_<n>\", \"type\": <event>, \"created_at\": <UTC time>, \"data\": <the event's fields; a row is its id>}",
                    "headers": {
                        "AIP-Signature": "t=<unix seconds>,v1=<hex HMAC-SHA256 over \"<t>.<body>\"> keyed with the endpoint's secret",
                        "AIP-Event-Id": "the same for every attempt and every endpoint of one event: dedupe on it",
                        "AIP-Event": "the event name",
                        "AIP-Delivery-Id": "one per endpoint and event",
                        "AIP-Delivery-Attempt": "1 for the first attempt, then 2, 3, ...",
                    },
                },
                "guarantees": {
                    "delivery": "at least once: a delivery without a 2xx answer is repeated, so receivers must tolerate duplicates",
                    "order": "none: events reach an endpoint independently, and a retried one arrives after later ones",
                    "authenticity": "verify AIP-Signature with the endpoint's secret and refuse a timestamp more than 5 minutes old, which stops a replay",
                    "secret": "shown once, in the response of the command that created the endpoint; the server stores it nowhere and cannot show it again",
                    "target": "only public http(s) addresses are called; redirects are not followed",
                },
            })
        })
        .collect()
}

/// Typed TypeScript client for `core`.
pub fn gen_ts(core: &ir::Program) -> String {
    tsgen::generate(&describe(core))
}

fn enums(core: &ir::Program) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = core.enums.iter().map(|(n, d)| (n.clone(), d.values.clone())).collect();
    for (name, values) in ir::builtin::form_enums(core) {
        out.entry(name).or_insert(values);
    }
    out
}

fn records(core: &ir::Program) -> BTreeMap<String, Vec<ParamSpec>> {
    core.records
        .iter()
        .map(|(n, r)| {
            let fields = r
                .fields
                .iter()
                .map(|f| ParamSpec { name: f.name.clone(), ty: declared(&f.ty), optional: f.optional || f.default.is_some(), default: None })
                .collect();
            (n.clone(), fields)
        })
        .collect()
}

fn events(core: &ir::Program) -> BTreeMap<String, Vec<(String, ty::TypeSpec)>> {
    core.events.iter().map(|(n, e)| (n.clone(), e.fields.iter().map(|(f, t)| (f.clone(), ty::inferred(t))).collect())).collect()
}

/// Literal defaults a client can show; computed defaults are the server's business.
fn default_value(d: &ir::Expr) -> Option<Value> {
    match &d.node {
        ir::Node::Lit { lit: ir::Literal::Int(n) } => Some(json!(n)),
        ir::Node::Lit { lit: ir::Literal::Text(s) } => Some(json!(s)),
        ir::Node::Lit { lit: ir::Literal::Bool(b) } => Some(json!(b)),
        ir::Node::EnumValue { value, .. } => Some(json!(value)),
        _ => None,
    }
}

pub(crate) fn params(ps: &[ir::Param]) -> Vec<ParamSpec> {
    ps.iter()
        .map(|p| ParamSpec {
            name: p.name.clone(),
            ty: declared(&p.ty),
            optional: p.optional || p.default.is_some(),
            default: p.default.as_ref().and_then(default_value),
        })
        .collect()
}

pub(crate) struct QueryView<'a> {
    pub params: &'a [ParamSpec],
    pub single: bool,
    pub output: Value,
    pub page: Option<Value>,
    pub cache_seconds: Option<u64>,
    pub errors: &'a BTreeSet<ErrorFact>,
    /// What a client may rely on when the query searches text.
    pub search: Option<Value>,
}

pub(crate) fn query_json(q: QueryView<'_>) -> Value {
    let mut guarantees = json!({
        "consistency": "single snapshot (repeatable read)",
        "round_trips": 1,
    });
    if let (Some(search), Some(g)) = (q.search, guarantees.as_object_mut()) {
        g.insert("search".into(), search);
    }
    json!({
        "input": q.params,
        "returns": if q.single { "object" } else { "list" },
        "output": q.output,
        "page": q.page,
        "cache_seconds": q.cache_seconds,
        "errors": q.errors,
        "guarantees": guarantees,
    })
}

pub(crate) struct CommandView<'a> {
    pub params: &'a [ParamSpec],
    pub output: Value,
    pub idempotent: bool,
    pub derived_key: bool,
    pub audited: bool,
    pub facts: &'a facts::Facts,
    pub emits: &'a [String],
}

pub(crate) fn command_json(c: CommandView<'_>) -> Value {
    json!({
        "input": c.params,
        "output": c.output,
        "idempotent": c.idempotent,
        "idempotency": if c.derived_key { "derived from input" } else if c.idempotent { "Idempotency-Key header required" } else { "not idempotent" },
        "audited": c.audited,
        "writes": c.facts.writes,
        "emits": c.emits,
        "effects": c.facts.effects,
        "errors": c.facts.errors,
        "guarantees": {
            "atomic": "one database transaction",
            "events": if c.emits.is_empty() { Value::Null } else { json!("at-least-once via transactional outbox; consumers must tolerate duplicates") },
            "retry": if c.idempotent { "safe with the same key" } else { "a retry may apply twice" },
        }
    })
}

fn query(core: &ir::Program, name: &str, q: &ir::Query) -> Value {
    let page = q.page.as_ref().map(|pg| {
        json!({
            "size": pg.size.max(0) as u64,
            "mode": if pg.offset_max_page.is_none() { "cursor" } else { "offset" },
            "max_page": pg.offset_max_page.map(|x| x.max(0) as u64),
        })
    });
    // a query from an extension call has no output shape
    let output = if matches!(q.source, Some(ir::QuerySource::Call { .. })) { Value::Null } else { shape::to_json(&q.output) };
    let search = match &q.source {
        Some(ir::QuerySource::Search { search, .. }) => core.forms.iter().find_map(|f| match f {
            ir::Form::Search(x) if x.name == *search => Some(search_guarantee(x)),
            _ => None,
        }),
        _ => None,
    };
    query_json(QueryView {
        params: &params(&q.params),
        single: matches!(q.source, Some(ir::QuerySource::Param { .. })),
        output,
        page,
        cache_seconds: q.cache.as_ref().map(|c| c.seconds.max(0) as u64),
        errors: &facts::query_errors(core, name, q),
        search,
    })
}

/// Words of the query are combined with AND (`or`, quotes and a leading `-` work as in a web search box); hits come best
/// first with ties broken by id, and a Korean search is a word-prefix approximation, not morphological analysis.
fn search_guarantee(s: &ir::Search) -> Value {
    let approximate = ir::builtin::search_is_approximate(s.language.as_deref());
    json!({
        "language": s.language,
        "query_syntax": "web search: words are ANDed, `or`, \"quoted phrase\" and `-word` are understood",
        "order": "best match first, ties by id",
        "match": if approximate { "word prefix; not morphological analysis (a changed stem, a compound or the middle of a word is not found)" } else { "stemmed words" },
        "approximate": approximate,
    })
}

fn command(core: &ir::Program, name: &str, c: &ir::Command) -> Value {
    let mut ps = params(&c.params);
    for v in facts::version_params(core, c) {
        ps.push(ParamSpec { name: v.name, ty: ty::TypeSpec::Int { min: Some(1), max: None }, optional: false, default: None });
    }
    let emits: Vec<String> = c.emits.iter().map(|e| e.event.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    command_json(CommandView {
        params: &ps,
        output: shape::to_json(&c.output),
        idempotent: !matches!(c.idempotency, ir::Idempotency::None),
        derived_key: matches!(c.idempotency, ir::Idempotency::Derived { .. }),
        audited: c.audited,
        facts: &facts::command_facts(core, name, c),
        emits: &emits,
    })
}

/// What the people who run the server need and clients must not see: how
/// providers are to call and sign webhooks (including the name of the secret
/// the server reads) and how long job files are kept. Printed by
/// `aip contract --operator`; no runtime endpoint serves it.
pub fn operator(core: &ir::Program) -> Value {
    let mut webhooks = Vec::new();
    for r in &core.reactions {
        let ir::Reaction::Webhook(w) = r else { continue };
        let st = ir::builtin::webhook_settings(w);
        webhooks.push(json!({
            "name": w.name,
            "url": format!("POST /aip/webhooks/{}", w.name),
            "source": st.source,
            "signature": {"scheme": st.scheme, "header": st.header, "secret_env": st.secret_env},
            "events": w.handlers.iter().map(|h| h.event.clone()).collect::<Vec<_>>(),
        }));
    }
    let jobs: Vec<Value> = core
        .forms
        .iter()
        .filter_map(|f| match f {
            ir::Form::Job(j) => Some(j),
            _ => None,
        })
        .map(|j| {
            let retention = j.produce.as_ref().map(
                |x| json!({"format": x.format, "store": x.store, "bucket": x.bucket, "expires_seconds": x.expires_seconds.map(|s| s.max(0) as u64)}),
            );
            json!({"start": j.name, "produces": retention})
        })
        .collect();
    let outbound: Vec<Value> = core
        .forms
        .iter()
        .filter_map(|f| match f {
            ir::Form::OutboundWebhooks(o) => Some(json!({
                "endpoint": o.entity,
                "events": o.events,
                "sign": o.sign,
                "secret": "whsec_<hex> = HMAC-SHA256(AIP_SECRET, \"aip-outbound-webhook:<Entity>:<endpoint id>\"): changing AIP_SECRET changes every endpoint's secret",
                "retry": o.retry.max(0) as u64,
                "over_seconds": o.over_seconds.max(0) as u64,
                "backoff": "each wait twice the one before; the last retry falls exactly `over` after the event",
                "disable_after_seconds": o.disable_after_seconds.map(|s| s.max(0) as u64),
                "private_targets": "refused unless the server is started with --allow-private-webhook-targets (development only)",
            })),
            _ => None,
        })
        .collect();
    let mut doc = json!({"protocol": PROTOCOL_VERSION, "core_ir": ir::CORE_IR_VERSION, "webhooks": webhooks, "jobs": jobs});
    if !outbound.is_empty()
        && let Some(m) = doc.as_object_mut()
    {
        m.insert("outbound_webhooks".into(), Value::Array(outbound));
    }
    doc
}

fn jobs(core: &ir::Program) -> Vec<Value> {
    core.forms
        .iter()
        .filter_map(|f| match f {
            ir::Form::Job(j) => Some(j),
            _ => None,
        })
        .map(|j| {
            // a file is only produced from rows the job walks
            let walks_rows = j.progress.as_ref().is_some_and(|se| facts::set_entity(se).is_some());
            let produces = j
                .produce
                .as_ref()
                .filter(|_| walks_rows)
                .map(|x| json!({"format": x.format, "expires_seconds": x.expires_seconds.map(|s| s.max(0) as u64)}));
            json!({"start": j.name, "status": format!("{}Status", j.name), "produces": produces})
        })
        .collect()
}

/// Decimal and Money cross the wire as strings (`"12.50"`), in input and output alike.
fn mark_decimal_wire(v: &mut Value) {
    match v {
        Value::Object(m) => {
            if matches!(m.get("kind").and_then(Value::as_str), Some("decimal" | "money")) {
                m.insert("wire".into(), json!("decimal_string"));
                m.insert("format".into(), json!("^-?(0|[1-9][0-9]*)(\\.[0-9]+)?$"));
            }
            m.values_mut().for_each(mark_decimal_wire);
        }
        Value::Array(a) => a.iter_mut().for_each(mark_decimal_wire),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_and_money_are_marked_as_strings_everywhere() {
        let mut doc = json!({
            "queries": {"Q": {"input": [{"name": "a", "ty": {"kind": "money", "currency": "KRW"}}],
                              "output": {"kind": "list", "of": {"kind": "object", "fields": {"p": {"kind": "decimal", "nullable": true}, "n": {"kind": "int"}}}}}}
        });
        mark_decimal_wire(&mut doc);
        let q = &doc["queries"]["Q"];
        assert_eq!(q["input"][0]["ty"]["wire"], json!("decimal_string"));
        assert_eq!(q["output"]["of"]["fields"]["p"]["wire"], json!("decimal_string"));
        assert_eq!(q["output"]["of"]["fields"]["n"].get("wire"), None, "ints stay plain numbers");
    }
}
