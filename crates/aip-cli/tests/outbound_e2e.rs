//! End-to-end for `outbound webhooks`: the SaaS example delivers `TaskCompleted` to the endpoints customers registered.
//!
//! A receiver runs inside the test (a local axum server) so deliveries are really sent, signed and answered. What has to hold:
//! the signature verifies with the endpoint's own secret and with nobody else's, a 5xx is retried on a doubling schedule
//! and then succeeds, an endpoint that only fails is disabled, a private address is never called, the `where` filter
//! and the tenant of the event decide who receives what, and the secret is shown once and stored nowhere.
//! `outbound_negative_controls` runs the same probes with each protection off and requires them to fail.

mod common;

use aip_runtime::engine::Engine;
use aip_runtime::outbound::{self, Settings};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use common::{drain, fails, ok, sql_one};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

static KEY: AtomicUsize = AtomicUsize::new(0);

fn key() -> String {
    format!("o{}", KEY.fetch_add(1, Ordering::Relaxed))
}

fn id(v: &Value) -> String {
    v["id"].as_str().expect("id").to_string()
}

// ---------------------------------------------------------------- the receiver

#[derive(Clone, Debug)]
struct Hit {
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

#[derive(Default)]
struct Rx {
    hits: Mutex<Vec<Hit>>,
    /// answers still to give per path, then 200
    script: Mutex<HashMap<String, VecDeque<u16>>>,
}

impl Rx {
    fn hits(&self, path: &str) -> Vec<Hit> {
        self.hits.lock().expect("lock").iter().filter(|h| h.path == path).cloned().collect()
    }

    fn answer(&self, path: &str, codes: &[u16]) {
        self.script.lock().expect("lock").insert(path.to_string(), codes.iter().copied().collect());
    }

    /// Every request to `path` from now on gets this status.
    fn always(&self, path: &str, code: u16) {
        self.answer(path, &[code; 64]);
    }
}

async fn receive(State(rx): State<Arc<Rx>>, Path(name): Path<String>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let path = format!("/{name}");
    let headers = headers.iter().map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or_default().to_string())).collect();
    rx.hits.lock().expect("lock").push(Hit { path: path.clone(), headers, body: body.to_vec() });
    let code = rx.script.lock().expect("lock").get_mut(&path).and_then(VecDeque::pop_front).unwrap_or(200);
    StatusCode::from_u16(code).unwrap_or(StatusCode::OK)
}

async fn receiver() -> (Arc<Rx>, String) {
    let rx = Arc::new(Rx::default());
    let app = axum::Router::new().route("/{name}", post(receive)).with_state(rx.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (rx, base)
}

// ---------------------------------------------------------------- the app

async fn member(e: &Engine, name: &str) -> String {
    sql_one(e, &format!("INSERT INTO member (email) VALUES ('{name}@x.com') RETURNING id")).await.as_str().expect("id").to_string()
}

struct World {
    e: Arc<Engine>,
    alice: String,
    bob: String,
    carol: String,
    wa: String,
    wb: String,
    /// two projects in workspace A, one in B
    pa1: String,
    pa2: String,
    pb: String,
}

async fn world(e: Arc<Engine>) -> World {
    let (alice, bob, carol) = (member(&e, "alice").await, member(&e, "bob").await, member(&e, "carol").await);
    let wa = id(&ok(&e, "CreateWorkspace", Some(&alice), json!({"name": "A"}), Some(&key())).await);
    let wb = id(&ok(&e, "CreateWorkspace", Some(&bob), json!({"name": "B"}), Some(&key())).await);
    ok(&e, "AddMember", Some(&alice), json!({"workspace": wa, "member": carol}), None).await;
    let project = |actor: &str, ws: &str, title: &str| {
        let input = json!({"workspace": ws, "title": title});
        let (e, actor) = (e.clone(), actor.to_string());
        async move { id(&ok(&e, "CreateProject", Some(&actor), input, Some(&key())).await) }
    };
    let pa1 = project(&alice, &wa, "PA1").await;
    let pa2 = project(&alice, &wa, "PA2").await;
    let pb = project(&bob, &wb, "PB").await;
    World { e, alice, bob, carol, wa, wb, pa1, pa2, pb }
}

struct Endpoint {
    id: String,
    secret: String,
}

async fn endpoint(w: &World, actor: &str, project: &str, url: &str) -> Endpoint {
    let r = ok(&w.e, "AddEndpoint", Some(actor), json!({"project": project, "url": url}), None).await;
    Endpoint { id: id(&r), secret: r["secret"].as_str().expect("the secret is in the response").to_string() }
}

async fn complete(w: &World, actor: &str, project: &str, title: &str) -> String {
    let t = id(&ok(&w.e, "CreateTask", Some(actor), json!({"project": project, "title": title}), Some(&key())).await);
    ok(&w.e, "CompleteTask", Some(actor), json!({"task": t}), None).await;
    drain(&w.e).await;
    t
}

/// Sends everything that is due, once each.
async fn pump(e: &Engine) -> usize {
    let mut n = 0;
    while outbound::tick(e).await.expect("tick") {
        n += 1;
    }
    n
}

async fn exec(e: &Engine, sql: &str) {
    e.pool.get().await.expect("conn").execute(sql, &[]).await.expect("sql");
}

async fn make_due(e: &Engine) {
    exec(e, "UPDATE _aip_outbound_delivery SET next_attempt_at = now() WHERE status = 'PENDING'").await;
}

async fn deliveries(e: &Engine, endpoint: &str) -> Vec<Value> {
    let q = format!(
        "SELECT coalesce(jsonb_agg(jsonb_build_object('status', status, 'attempts', attempts, 'last_status', last_status, 'last_error', last_error, \
         'event_id', event_id, 'wait', extract(epoch FROM next_attempt_at - created_at)::float8, 'window', extract(epoch FROM deadline - created_at)::float8) ORDER BY created_at, id), '[]'::jsonb) AS val \
         FROM _aip_outbound_delivery WHERE endpoint = '{endpoint}'"
    );
    sql_one(e, &q).await.as_array().cloned().unwrap_or_default()
}

async fn queued_for(e: &Engine, task: &str) -> BTreeSet<String> {
    let q = format!(
        "SELECT coalesce(jsonb_agg(endpoint::text), '[]'::jsonb) AS val FROM _aip_outbound_delivery WHERE body #>> '{{data,task}}' = '{task}'"
    );
    sql_one(e, &q).await.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn header<'h>(h: &'h Hit, name: &str) -> &'h str {
    h.headers.get(&name.to_ascii_lowercase()).map(String::as_str).unwrap_or_default()
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The saas example with the form's numbers replaced, so a test does not wait for hours.
fn program(tweak: impl FnOnce(&mut aip_ir::OutboundWebhooks)) -> aip_plan::Program {
    let path = format!("{}/../../examples/saas/app.aip", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(path).expect("example source");
    let (mut core, map) = aip_sema::pipeline::check_source(&src).into_core().expect("saas checks clean");
    let mut tweak = Some(tweak);
    for f in core.forms.iter_mut() {
        if let aip_ir::Form::OutboundWebhooks(o) = f
            && let Some(t) = tweak.take()
        {
            t(o);
        }
    }
    let compiled = aip_pg::compile(&core, &map);
    assert!(!compiled.diagnostics.iter().any(|d| d.is_error()), "{:?}", compiled.diagnostics);
    compiled.program
}

/// retry 3 over 7s: the waits are 1s, 2s and 4s
fn quick(o: &mut aip_ir::OutboundWebhooks) {
    o.retry = 3;
    o.over_seconds = 7;
    o.disable_after_seconds = Some(600);
}

async fn engine(db: &str, program: aip_plan::Program, allow_private: bool) -> Arc<Engine> {
    common::setup_program_with(db, program, Settings { allow_private }).await
}

fn verifies(h: &Hit, secret: &str) -> bool {
    outbound::verify_signature(secret, header(h, "AIP-Signature"), &h.body, now()).is_ok()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_webhooks_end_to_end() {
    let (rx, base) = receiver().await;
    let w = world(engine("aip_e2e_outbound", program(quick), true).await).await;
    let e = &w.e;

    // --- registering shows the secret once; it is derived, never stored, and the registration is refused for a non-web URL
    let e1 = endpoint(&w, &w.alice, &w.pa1, &format!("{base}/e1")).await;
    let e2 = endpoint(&w, &w.alice, &w.pa2, &format!("{base}/e2")).await;
    let e3 = endpoint(&w, &w.bob, &w.pb, &format!("{base}/e3")).await;
    assert!(e1.secret.starts_with("whsec_") && e1.secret.len() == 70, "{}", e1.secret);
    assert_eq!(e1.secret, outbound::signing_secret(common::TEST_SECRET, "Endpoint", &e1.id), "the SQL and the sender derive the same secret");
    assert_ne!(e1.secret, e2.secret, "one secret per endpoint");
    let listed = ok(e, "ProjectEndpoints", Some(&w.alice), json!({"project": w.pa1}), None).await;
    assert!(listed.to_string().contains(&e1.id) && !listed.to_string().contains("whsec_"), "a later read does not show it: {listed}");
    let again = fails(e, "AddEndpoint", Some(&w.alice), json!({"project": w.pa1, "url": format!("{base}/e1")}), None).await;
    assert_eq!(again.code, "AIP.CONFLICT.UNIQUE", "registering twice does not hand out a second copy: {again}");
    for bad in ["ftp://example.com/hook", "javascript:alert(1)", "http://user:pw@example.com/"] {
        let err = fails(e, "AddEndpoint", Some(&w.alice), json!({"project": w.pa1, "url": bad}), None).await;
        assert_eq!(err.code, "AIP.INPUT.INVALID", "{bad}: {err}");
    }
    let raw = e.pool.get().await.expect("conn");
    for bad in ["ftp://example.com/hook", "file:///etc/passwd", "http://user@example.com/"] {
        let r = raw.execute("INSERT INTO endpoint (project_id, url) VALUES ($1::text::uuid, $2)", &[&w.pa1, &bad]).await;
        assert!(r.is_err(), "the database refuses {bad} even behind the application's back");
    }
    // the secret is nowhere in the database: not in a row, an audit entry, the idempotency store or the outbox
    let tables: Vec<String> = sql_one(e, "SELECT jsonb_agg(table_name) AS val FROM information_schema.tables WHERE table_schema = 'public'")
        .await
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    for t in &tables {
        let n = sql_one(e, &format!("SELECT count(*) AS val FROM \"{t}\" x WHERE x::text LIKE '%{}%' OR x::text LIKE '%{}%'", e1.secret, e3.secret))
            .await;
        assert_eq!(n, json!(0), "the secret is stored in {t}");
    }

    // --- an event reaches the endpoints of its own project only: not the other project of the workspace (where), not another workspace (tenant)
    let t1 = complete(&w, &w.alice, &w.pa1, "ship it").await;
    assert_eq!(queued_for(e, &t1).await, BTreeSet::from([e1.id.clone()]), "only the endpoint of the task's project");
    assert_eq!(pump(e).await, 1);
    assert_eq!(rx.hits("/e1").len(), 1);
    assert!(rx.hits("/e2").is_empty() && rx.hits("/e3").is_empty());
    let t3 = complete(&w, &w.bob, &w.pb, "other tenant").await;
    assert_eq!(queued_for(e, &t3).await, BTreeSet::from([e3.id.clone()]));
    pump(e).await;
    assert_eq!((rx.hits("/e1").len(), rx.hits("/e2").len(), rx.hits("/e3").len()), (1, 0, 1));

    // --- what the receiver sees: a signature only this endpoint's secret verifies, the event id, the event, the data
    let hit = rx.hits("/e1").remove(0);
    assert!(verifies(&hit, &e1.secret), "the endpoint's own secret verifies the delivery");
    assert!(!verifies(&hit, &e2.secret) && !verifies(&hit, &e3.secret), "no other endpoint's secret does");
    let mut tampered = hit.clone();
    tampered.body.extend_from_slice(b" ");
    assert!(!verifies(&tampered, &e1.secret), "the body is signed");
    let header_value = header(&hit, "AIP-Signature");
    let (t, v1) = header_value.split_once(",v1=").expect("t=..,v1=..");
    let t: i64 = t.trim_start_matches("t=").parse().expect("timestamp");
    assert!((now() - t).abs() < 30, "the timestamp is the time of sending");
    assert!(
        outbound::verify_signature(&e1.secret, header_value, &hit.body, t + outbound::TOLERANCE_SECS + 1).is_err(),
        "a copy replayed later is refused"
    );
    {
        use hmac::{Hmac, Mac};
        let mut m = Hmac::<sha2::Sha256>::new_from_slice(e1.secret.as_bytes()).expect("key");
        m.update(format!("{t}.").as_bytes());
        m.update(&hit.body);
        let want: String = m.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(v1, want, "the documented scheme: HMAC-SHA256 over \"<t>.<body>\"");
    }
    let body: Value = serde_json::from_slice(&hit.body).expect("json body");
    assert_eq!(body["type"], json!("TaskCompleted"));
    assert_eq!(body["data"]["task"], json!(t1));
    assert_eq!(body["id"], json!(header(&hit, "AIP-Event-Id")), "the id in the body is the id in the header");
    assert!(header(&hit, "AIP-Event-Id").starts_with("evt_"));
    assert_eq!(header(&hit, "AIP-Event"), "TaskCompleted");
    assert_eq!(header(&hit, "AIP-Delivery-Attempt"), "1");
    assert_eq!(header(&hit, "content-type"), "application/json");
    assert_eq!(deliveries(e, &e1.id).await[0]["status"], json!("DELIVERED"));

    // --- a 5xx is retried on a doubling schedule inside the window, with the same event id, and then succeeds
    rx.answer("/e1", &[503, 500]);
    let t2 = complete(&w, &w.alice, &w.pa1, "flaky receiver").await;
    assert_eq!(pump(e).await, 1);
    let first = deliveries(e, &e1.id).await.pop().expect("delivery");
    assert_eq!((first["status"].clone(), first["attempts"].clone(), first["last_status"].clone()), (json!("PENDING"), json!(1), json!(503)));
    assert!((first["wait"].as_f64().expect("wait") - 1.0).abs() < 0.5, "first retry 1s after the event: {first}");
    assert_eq!(first["window"].as_f64(), Some(7.0));
    assert_eq!(pump(e).await, 0, "nothing is due before the wait is over");
    make_due(e).await;
    assert_eq!(pump(e).await, 1);
    let second = deliveries(e, &e1.id).await.pop().expect("delivery");
    assert_eq!((second["attempts"].clone(), second["last_status"].clone()), (json!(2), json!(500)));
    assert!((second["wait"].as_f64().expect("wait") - 3.0).abs() < 0.5, "second retry 3s after the event (waits 1s then 2s): {second}");
    make_due(e).await;
    assert_eq!(pump(e).await, 1);
    let done = deliveries(e, &e1.id).await.pop().expect("delivery");
    assert_eq!((done["status"].clone(), done["attempts"].clone()), (json!("DELIVERED"), json!(3)));
    let flaky: Vec<Hit> = rx.hits("/e1").into_iter().filter(|h| String::from_utf8_lossy(&h.body).contains(&t2)).collect();
    assert_eq!(flaky.len(), 3);
    assert_eq!(flaky.iter().map(|h| header(h, "AIP-Delivery-Attempt")).collect::<Vec<_>>(), ["1", "2", "3"]);
    assert_eq!(
        flaky.iter().map(|h| header(h, "AIP-Event-Id")).collect::<BTreeSet<_>>().len(),
        1,
        "every attempt carries the same event id: the receiver can dedupe"
    );
    assert!(flaky.iter().all(|h| verifies(h, &e1.secret)), "each attempt is signed afresh");
    // an unacknowledged-but-delivered case: a second dispatch of the same event does not queue a second delivery
    let n_before = deliveries(e, &e1.id).await.len();
    exec(e, "UPDATE _aip_outbox SET done_at = NULL WHERE kind = 'event' AND name = 'TaskCompleted'").await;
    drain(e).await;
    assert_eq!(deliveries(e, &e1.id).await.len(), n_before, "re-dispatching an event queues nothing twice");

    // --- the failure of one endpoint does not hold the others up
    rx.always("/e1", 500);
    rx.answer("/e2", &[]);
    let t4 = complete(&w, &w.alice, &w.pa2, "e2 is healthy").await;
    pump(e).await;
    assert_eq!(deliveries(e, &e2.id).await.last().expect("delivery")["status"], json!("DELIVERED"), "{t4}");

    // --- the secret of another endpoint is what a second registration gets, so a leak of one reveals nothing of the others
    assert_ne!(e2.secret, e1.secret);
    let _ = (w.carol.as_str(), w.wa.as_str(), w.wb.as_str());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn endpoint_that_only_fails_is_disabled() {
    let (rx, base) = receiver().await;
    // two retries over 2s, disabled after 2s of failing
    let p = program(|o| {
        o.retry = 2;
        o.over_seconds = 3;
        o.disable_after_seconds = Some(2);
    });
    let w = world(engine("aip_e2e_outbound_disable", p, true).await).await;
    let e = &w.e;
    let bad = endpoint(&w, &w.alice, &w.pa1, &format!("{base}/bad")).await;
    let good = endpoint(&w, &w.alice, &w.pa2, &format!("{base}/good")).await;
    rx.always("/bad", 500);

    let t1 = complete(&w, &w.alice, &w.pa1, "first").await;
    pump(e).await;
    let state = |e: &Engine, ep: &str| {
        let q = format!(
            "SELECT jsonb_build_object('failing', failing_since IS NOT NULL, 'disabled', disabled_at IS NOT NULL, 'reason', disabled_reason) AS val FROM _aip_outbound_endpoint WHERE endpoint = '{ep}'"
        );
        let e = e.pool.clone();
        async move {
            let c = e.get().await.expect("conn");
            let row = c.query_opt(&format!("WITH x AS ({q}) SELECT to_jsonb(x) FROM x"), &[]).await.expect("sql");
            row.map(|r| r.get::<_, Value>(0)["val"].clone()).unwrap_or(Value::Null)
        }
    };
    assert_eq!(state(e, &bad.id).await["failing"], json!(true), "a failure starts the streak");
    assert_eq!(state(e, &bad.id).await["disabled"], json!(false), "not yet");
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
    make_due(e).await;
    pump(e).await;
    let s = state(e, &bad.id).await;
    assert_eq!(s["disabled"], json!(true), "failing for as long as `disable after` disables the endpoint: {s}");
    assert!(s["reason"].as_str().is_some_and(|r| r.contains("failing for") && r.contains("HTTP 500")), "the reason is recorded: {s}");
    let ds = deliveries(e, &bad.id).await;
    assert!(ds.iter().all(|d| d["status"] != json!("PENDING")), "nothing stays queued for a disabled endpoint: {ds:?}");
    assert!(ds.iter().any(|d| d["status"] == json!("CANCELLED")), "{ds:?}");
    let hits_when_disabled = rx.hits("/bad").len();
    // later events skip it, the healthy endpoint is unaffected, and nothing more is sent to the disabled one
    let t2 = complete(&w, &w.alice, &w.pa1, "after the disabling").await;
    assert!(queued_for(e, &t2).await.is_empty(), "a disabled endpoint is not queued for");
    let t3 = complete(&w, &w.alice, &w.pa2, "healthy").await;
    assert_eq!(queued_for(e, &t3).await, BTreeSet::from([good.id.clone()]));
    pump(e).await;
    assert_eq!(rx.hits("/bad").len(), hits_when_disabled);
    assert_eq!(rx.hits("/good").len(), 1);
    let _ = t1;

    // a success ends a streak: a receiver that fails once and then recovers is not disabled
    rx.answer("/good", &[500]);
    let t4 = complete(&w, &w.alice, &w.pa2, "blip").await;
    pump(e).await;
    assert_eq!(state(e, &good.id).await["failing"], json!(true));
    make_due(e).await;
    pump(e).await;
    assert_eq!(state(e, &good.id).await["failing"], json!(false), "a success clears the streak");
    assert_eq!(state(e, &good.id).await["disabled"], json!(false), "{t4}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn private_addresses_are_not_called() {
    let (rx, base) = receiver().await;
    // production default: private addresses are refused
    let w = world(engine("aip_e2e_outbound_private", program(quick), false).await).await;
    let e = &w.e;
    let local = endpoint(&w, &w.alice, &w.pa1, &format!("{base}/local")).await;
    let t = complete(&w, &w.alice, &w.pa1, "to loopback").await;
    assert_eq!(pump(e).await, 1);
    let d = deliveries(e, &local.id).await.pop().expect("delivery");
    assert_eq!(d["status"], json!("FAILED"), "a refused URL is not retried: {d}");
    assert!(d["last_error"].as_str().is_some_and(|m| m.starts_with("URL_REJECTED")), "{d}");
    assert!(rx.hits("/local").is_empty(), "nothing reached the loopback server");
    // the other addresses the server must never be sent to, by IP and by name
    for (n, url) in [
        "http://169.254.169.254/latest/meta-data",
        "http://10.0.0.7/hook",
        "http://[::1]:9/hook",
        "http://localhost:9/hook",
        "http://192.168.1.1/hook",
    ]
    .iter()
    .enumerate()
    {
        let project = if n % 2 == 0 { &w.pa1 } else { &w.pa2 };
        let path = format!("/n{n}");
        let _ = path;
        // a second endpoint per project needs another URL; vary the query string
        let r = e.pool.get().await.expect("conn");
        r.execute("INSERT INTO endpoint (project_id, url) VALUES ($1::text::uuid, $2)", &[project, &format!("{url}?n={n}")]).await.expect("insert");
    }
    let t2 = complete(&w, &w.alice, &w.pa1, "again").await;
    let t3 = complete(&w, &w.alice, &w.pa2, "again 2").await;
    pump(e).await;
    let rejected: Value =
        sql_one(e, "SELECT count(*) AS val FROM _aip_outbound_delivery WHERE status = 'FAILED' AND last_error LIKE 'URL_REJECTED%'").await;
    assert!(rejected.as_i64().unwrap_or(0) >= 7, "every private target was refused: {rejected}");
    assert_eq!(sql_one(e, "SELECT count(*) AS val FROM _aip_outbound_delivery WHERE status = 'DELIVERED'").await, json!(0));
    let _ = (t, t2, t3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_event_names_the_tenant_even_without_a_condition() {
    let (rx, base) = receiver().await;
    // no `where`: every endpoint of the event's tenant, none of another
    let w = world(
        engine(
            "aip_e2e_outbound_nofilter",
            program(|o| {
                quick(o);
                o.filter = None;
            }),
            true,
        )
        .await,
    )
    .await;
    let e = &w.e;
    let e1 = endpoint(&w, &w.alice, &w.pa1, &format!("{base}/e1")).await;
    let e2 = endpoint(&w, &w.alice, &w.pa2, &format!("{base}/e2")).await;
    let e3 = endpoint(&w, &w.bob, &w.pb, &format!("{base}/e3")).await;
    let t = complete(&w, &w.alice, &w.pa1, "without a condition").await;
    assert_eq!(queued_for(e, &t).await, BTreeSet::from([e1.id.clone(), e2.id.clone()]), "both endpoints of workspace A, and not the one of B");
    assert!(!queued_for(e, &t).await.contains(&e3.id));
    pump(e).await;
    assert!(rx.hits("/e3").is_empty());
    let _ = (w.carol.as_str(), w.wa.as_str(), w.wb.as_str());
}

/// Each protection removed, the probe it stops has to work: the guarantees above are not tests that cannot fail.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_negative_controls() {
    // the private-address check off (the development setting): the loopback server is called
    let (rx, base) = receiver().await;
    let w = world(engine("aip_e2e_outbound_no_ssrf", program(quick), true).await).await;
    endpoint(&w, &w.alice, &w.pa1, &format!("{base}/local")).await;
    complete(&w, &w.alice, &w.pa1, "x").await;
    pump(&w.e).await;
    assert_eq!(rx.hits("/local").len(), 1, "without the check the server calls its own loopback");

    // no retries: a 5xx is final
    let (rx, base) = receiver().await;
    let w = world(
        engine(
            "aip_e2e_outbound_no_retry",
            program(|o| {
                quick(o);
                o.retry = 0;
            }),
            true,
        )
        .await,
    )
    .await;
    let ep = endpoint(&w, &w.alice, &w.pa1, &format!("{base}/e1")).await;
    rx.answer("/e1", &[500]);
    complete(&w, &w.alice, &w.pa1, "x").await;
    pump(&w.e).await;
    make_due(&w.e).await;
    pump(&w.e).await;
    let d = deliveries(&w.e, &ep.id).await.pop().expect("delivery");
    assert_eq!((d["status"].clone(), d["attempts"].clone()), (json!("FAILED"), json!(1)), "without retries the delivery is lost: {d}");

    // no `disable after`: an endpoint that only fails stays enabled
    let (rx, base) = receiver().await;
    let w = world(
        engine(
            "aip_e2e_outbound_no_disable",
            program(|o| {
                o.retry = 1;
                o.over_seconds = 1;
                o.disable_after_seconds = None;
            }),
            true,
        )
        .await,
    )
    .await;
    let ep = endpoint(&w, &w.alice, &w.pa1, &format!("{base}/e1")).await;
    rx.always("/e1", 500);
    for _ in 0..3 {
        complete(&w, &w.alice, &w.pa1, "x").await;
        pump(&w.e).await;
        make_due(&w.e).await;
        pump(&w.e).await;
    }
    let disabled = sql_one(
        &w.e,
        &format!("SELECT coalesce(bool_or(disabled_at IS NOT NULL), false) AS val FROM _aip_outbound_endpoint WHERE endpoint = '{}'", ep.id),
    )
    .await;
    assert_eq!(disabled, json!(false));
    let streaks =
        sql_one(&w.e, &format!("SELECT count(*) AS val FROM _aip_outbound_endpoint WHERE endpoint = '{}' AND failing_since IS NOT NULL", ep.id))
            .await;
    assert_eq!(streaks, json!(1), "it is failing, and still enabled");
    assert!(deliveries(&w.e, &ep.id).await.len() >= 3, "and it keeps being queued for");
}
