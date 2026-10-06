//! End-to-end for `subscribe`: a real server, real WebSocket clients, real PostgreSQL `LISTEN/NOTIFY`.
//!
//! What has to hold: a subscriber gets a snapshot and then the whole result again after a change that alters it, caused by
//! anyone; nothing for a change that does not alter what that subscriber may see (another workspace, a table the
//! subscription does not read); rows the subscriber may not see are never in a message; losing access ends the subscription
//! with an error; a connection that did not authenticate, or names something that is not a subscription, gets nothing.
//! `subscribe_negative_controls` runs the same questions with a check switched off and requires the attack to work.

mod common;

use aip_runtime::auth;
use aip_runtime::engine::Engine;
use aip_runtime::http::{AppState, serve};
use aip_runtime::subscribe::{Hub, Limits};
use common::{TEST_SECRET, ok};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

static KEY: AtomicUsize = AtomicUsize::new(0);

fn key() -> String {
    format!("s{}", KEY.fetch_add(1, Ordering::Relaxed))
}

fn db_url(db: &str) -> String {
    let admin = std::env::var("AIP_TEST_ADMIN_URL").unwrap_or_else(|_| "postgres://localhost/postgres".into());
    admin.rsplit_once('/').map(|(base, _)| format!("{base}/{db}")).expect("url")
}

/// Starts a server for `e` on a free port; the hub is returned too so a test can read how often it re-ran subscriptions.
async fn start(e: Arc<Engine>, db: &str, limits: Limits) -> (std::net::SocketAddr, Arc<Hub>) {
    let port = std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port();
    let hub = Hub::start_with(e.clone(), &db_url(db), limits).await;
    let state = AppState {
        engine: e,
        secret: Arc::new(TEST_SECRET.to_vec()),
        dev_auth: false,
        trusted_proxies: false,
        contract: Arc::new(json!({})),
        hub: hub.clone(),
    };
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    tokio::spawn(async move {
        let _ = serve(state, addr).await;
    });
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return (addr, hub);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("server did not start");
}

type Stream = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Client(Stream);

impl Client {
    async fn connect(addr: std::net::SocketAddr) -> Client {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/aip/subscribe")).await.expect("websocket");
        Client(ws)
    }

    async fn send(&mut self, v: Value) {
        self.0.send(Message::Text(v.to_string().into())).await.expect("send");
    }

    async fn auth(&mut self, actor: &str) {
        self.send(json!({"type": "auth", "token": auth::issue(TEST_SECRET, actor, 600)})).await;
    }

    async fn subscribe(&mut self, id: &str, name: &str, input: Value) {
        self.send(json!({"type": "subscribe", "id": id, "name": name, "input": input})).await;
    }

    /// The next JSON message, or `None` when the server closed the connection or `ms` passed in silence.
    async fn next_within(&mut self, ms: u64) -> Option<Value> {
        loop {
            match tokio::time::timeout(Duration::from_millis(ms), self.0.next()).await {
                Err(_) => return None,
                Ok(None) | Ok(Some(Err(_))) => return Some(json!({"type": "closed"})),
                Ok(Some(Ok(Message::Text(t)))) => {
                    let v: Value = serde_json::from_str(t.as_str()).expect("json");
                    // `ready` only says the connection is authenticated
                    if v["type"] != "ready" {
                        return Some(v);
                    }
                }
                Ok(Some(Ok(Message::Close(_)))) => return Some(json!({"type": "closed"})),
                Ok(Some(Ok(_))) => {}
            }
        }
    }

    async fn next(&mut self) -> Value {
        self.next_within(5000).await.expect("a message within 5s")
    }

    /// True when nothing arrives for `ms`.
    async fn silent(&mut self, ms: u64) -> bool {
        self.next_within(ms).await.is_none()
    }
}

fn titles(v: &Value) -> Vec<String> {
    let mut t: Vec<String> = v["rows"].as_array().expect("rows").iter().map(|r| r["title"].as_str().expect("title").to_string()).collect();
    t.sort();
    t
}

struct World {
    e: Arc<Engine>,
    alice: String,
    bob: String,
    carol: String,
    wa: String,
    pa: String,
    pb: String,
}

async fn scalar(e: &Engine, q: &str) -> String {
    common::sql_one(e, q).await.as_str().expect("text").to_string()
}

/// alice is only in workspace A, bob only in B, carol in both; one project in each.
async fn world(e: Arc<Engine>) -> World {
    let mut ids = Vec::new();
    for n in ["alice", "bob", "carol"] {
        ids.push(scalar(&e, &format!("INSERT INTO member (email) VALUES ('{n}@x.com') RETURNING id::text")).await);
    }
    let (alice, bob, carol) = (ids[0].clone(), ids[1].clone(), ids[2].clone());
    let wa = ok(&e, "CreateWorkspace", Some(&alice), json!({"name": "A"}), Some(&key())).await["id"].as_str().expect("id").to_string();
    let wb = ok(&e, "CreateWorkspace", Some(&bob), json!({"name": "B"}), Some(&key())).await["id"].as_str().expect("id").to_string();
    ok(&e, "AddMember", Some(&alice), json!({"workspace": wa, "member": carol}), None).await;
    ok(&e, "AddMember", Some(&bob), json!({"workspace": wb, "member": carol}), None).await;
    let pa =
        ok(&e, "CreateProject", Some(&alice), json!({"workspace": wa, "title": "PA"}), Some(&key())).await["id"].as_str().expect("id").to_string();
    let pb = ok(&e, "CreateProject", Some(&bob), json!({"workspace": wb, "title": "PB"}), Some(&key())).await["id"].as_str().expect("id").to_string();
    World { e, alice, bob, carol, wa, pa, pb }
}

impl World {
    async fn task(&self, actor: &str, project: &str, title: &str) {
        ok(&self.e, "CreateTask", Some(actor), json!({"project": project, "title": title}), Some(&key())).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subscriptions_end_to_end() {
    let db = "aip_e2e_saas_subscribe";
    let w = world(common::setup_app("saas", db).await).await;
    let (addr, hub) = start(w.e.clone(), db, Limits::default()).await;
    w.task(&w.alice, &w.pa, "first").await;

    // snapshot, then the whole result again after a change by someone else
    let mut alice = Client::connect(addr).await;
    alice.auth(&w.alice).await;
    alice.subscribe("s1", "LiveProjectTasks", json!({"project": w.pa})).await;
    let snap = alice.next().await;
    assert_eq!((snap["type"].as_str(), snap["id"].as_str()), (Some("snapshot"), Some("s1")), "{snap}");
    assert_eq!(titles(&snap), ["first"]);
    w.task(&w.carol, &w.pa, "second").await;
    let changed = alice.next().await;
    assert_eq!((changed["type"].as_str(), changed["id"].as_str()), (Some("changed"), Some("s1")), "{changed}");
    assert_eq!(titles(&changed), ["first", "second"], "the whole result, not a delta");

    // a table the subscription does not read: it is not even run again
    let before = hub.reruns();
    ok(&w.e, "AddComment", Some(&w.alice), json!({"task": scalar(&w.e, "SELECT id::text FROM task LIMIT 1").await, "body": "hi"}), Some(&key()))
        .await;
    assert!(alice.silent(700).await, "a comment is not part of what the subscription shows");
    assert_eq!(hub.reruns(), before, "nothing the subscription reads changed, so it was not run again");

    // another workspace's change wakes the subscription (same table) but alters nothing it may see: no message
    w.task(&w.bob, &w.pb, "other workspace").await;
    assert!(alice.silent(700).await, "a task of workspace B is not in the result of a project of A");
    assert!(hub.reruns() > before, "the change touched a table the subscription reads, so it was looked at again");

    // an update that changes a selected field is a change too
    let id = scalar(&w.e, "SELECT id::text FROM task WHERE title = 'first'").await;
    ok(&w.e, "CompleteTask", Some(&w.alice), json!({"task": id}), None).await;
    let changed = alice.next().await;
    assert_eq!(changed["rows"].as_array().expect("rows").iter().filter(|r| r["status"] == "DONE").count(), 1, "{changed}");

    // unsubscribe: nothing follows
    alice.send(json!({"type": "unsubscribe", "id": "s1"})).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    w.task(&w.alice, &w.pa, "after unsubscribe").await;
    assert!(alice.silent(700).await, "an unsubscribed subscription is not sent anything");

    // a stranger to the workspace: `allow` is open, the rows are not. Never a row of A in a message, however A changes
    let mut bob = Client::connect(addr).await;
    bob.auth(&w.bob).await;
    bob.subscribe("w", "LiveWorkspaceTasks", json!({"workspace": w.wa})).await;
    let snap = bob.next().await;
    assert_eq!((snap["type"].as_str(), snap["rows"].as_array().map(Vec::len)), (Some("snapshot"), Some(0)), "{snap}");
    w.task(&w.alice, &w.pa, "secret of A").await;
    assert!(bob.silent(700).await, "a task bob cannot see changes nothing for bob");
    // while a subscriber to the project of another workspace is refused outright
    bob.subscribe("p", "LiveProjectTasks", json!({"project": w.pa})).await;
    let refused = bob.next().await;
    assert_eq!((refused["type"].as_str(), refused["id"].as_str()), (Some("error"), Some("p")), "{refused}");
    assert_eq!(refused["code"], "AIP.NOT_FOUND", "{refused}");

    // carol is in both workspaces: her view of A never holds a task of B, whoever creates it
    let mut carol = Client::connect(addr).await;
    carol.auth(&w.carol).await;
    carol.subscribe("a", "LiveWorkspaceTasks", json!({"workspace": w.wa})).await;
    let snap = carol.next().await;
    assert!(titles(&snap).iter().all(|t| !t.contains("B-task")), "{snap}");
    w.task(&w.carol, &w.pb, "B-task").await;
    assert!(carol.silent(700).await, "a task of B does not reach a subscription of A");
    w.task(&w.carol, &w.pa, "A-task").await;
    let changed = carol.next().await;
    assert!(titles(&changed).contains(&"A-task".to_string()) && !titles(&changed).contains(&"B-task".to_string()), "{changed}");

    // access revoked: the next refresh says so and the subscription is over, the connection stays
    carol.subscribe("p", "LiveProjectTasks", json!({"project": w.pa})).await;
    assert_eq!(carol.next().await["type"], "snapshot");
    let removed =
        w.e.pool
            .get()
            .await
            .expect("conn")
            .execute(&format!("DELETE FROM membership WHERE member_id = '{}' AND workspace_id = '{}'", w.carol, w.wa), &[])
            .await;
    assert_eq!(removed.expect("delete"), 1);
    let mut ended = std::collections::BTreeMap::new();
    for _ in 0..2 {
        let m = carol.next().await;
        ended.insert(m["id"].as_str().unwrap_or("").to_string(), m);
    }
    // the project subscription ends with an error; the workspace one, which leans on visibility, goes on with what she may still see
    assert_eq!(ended["p"]["type"], "error", "{ended:?}");
    // the project is no longer visible to her, so the load that comes before `allow` fails
    assert_eq!((ended["p"]["code"].as_str(), ended["p"]["reason"].as_str()), (Some("AIP.NOT_FOUND"), Some("PROJECT_NOT_FOUND")), "{ended:?}");
    assert_eq!((ended["a"]["type"].as_str(), ended["a"]["rows"].as_array().map(Vec::len)), (Some("changed"), Some(0)), "{ended:?}");
    w.task(&w.alice, &w.pa, "after the revoke").await;
    assert!(carol.silent(700).await, "nothing reaches carol after she lost A");

    // alice's own subscription had ended; she can open it again and is current
    alice.subscribe("again", "LiveProjectTasks", json!({"project": w.pa})).await;
    let snap = alice.next().await;
    assert!(titles(&snap).contains(&"after the revoke".to_string()), "{snap}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connections_must_authenticate_and_name_a_subscription() {
    let db = "aip_e2e_saas_subscribe_auth";
    let w = world(common::setup_app("saas", db).await).await;
    let limits = Limits { auth_timeout: Duration::from_millis(800), ..Limits::default() };
    let (addr, _) = start(w.e.clone(), db, limits).await;

    // subscribing before authenticating is refused and ends the connection
    let mut c = Client::connect(addr).await;
    c.subscribe("x", "LiveProjectTasks", json!({"project": w.pa})).await;
    let m = c.next().await;
    assert_eq!((m["type"].as_str(), m["code"].as_str()), (Some("error"), Some("AIP.AUTH.UNAUTHENTICATED")), "{m}");
    assert_eq!(c.next().await["type"], "closed");

    // a connection that never says who it is is closed
    let mut c = Client::connect(addr).await;
    let m = c.next().await;
    assert_eq!(m["code"], "AIP.AUTH.UNAUTHENTICATED", "{m}");
    assert_eq!(c.next().await["type"], "closed");

    // a bad token, an expired one, and a token signed with another secret
    for token in ["nonsense".to_string(), auth::issue(TEST_SECRET, &w.alice, -5), auth::issue(b"another secret", &w.alice, 600)] {
        let mut c = Client::connect(addr).await;
        c.send(json!({"type": "auth", "token": token})).await;
        let m = c.next().await;
        assert_eq!(m["code"], "AIP.AUTH.UNAUTHENTICATED", "{m}");
        assert_eq!(c.next().await["type"], "closed");
    }

    // a well-formed token of someone who does not exist
    let mut c = Client::connect(addr).await;
    c.auth("7b0c8a58-8f1f-4a5e-9a57-0c2f2b3f2e11").await;
    assert_eq!(c.next().await["code"], "AIP.AUTH.UNAUTHENTICATED");

    // anonymous (`auth` without a token) is allowed to connect but `allow` still decides: these ask for a member
    let mut c = Client::connect(addr).await;
    c.send(json!({"type": "auth"})).await;
    c.subscribe("x", "LiveProjectTasks", json!({"project": w.pa})).await;
    let m = c.next().await;
    assert_eq!((m["type"].as_str(), m["id"].as_str()), (Some("error"), Some("x")), "{m}");

    // only declared subscriptions open: a query, a command and a name that does not exist are all the same answer
    let mut c = Client::connect(addr).await;
    c.auth(&w.alice).await;
    for name in ["ProjectTasks", "CreateTask", "NoSuchThing"] {
        c.subscribe("n", name, json!({"project": w.pa})).await;
        let m = c.next().await;
        assert_eq!((m["type"].as_str(), m["code"].as_str()), (Some("error"), Some("AIP.REQUEST.UNKNOWN_INTENT")), "{name}: {m}");
    }
    // a bad input is the same error a query gives, and the connection is still good
    c.subscribe("bad", "LiveProjectTasks", json!({"project": "not-a-uuid"})).await;
    assert_eq!(c.next().await["code"], "AIP.INPUT.INVALID");
    c.subscribe("ok", "LiveProjectTasks", json!({"project": w.pa})).await;
    assert_eq!(c.next().await["type"], "snapshot");
    // a message that is not JSON ends the connection
    c.0.send(Message::Text("not json".into())).await.expect("send");
    assert_eq!(c.next().await["code"], "AIP.REQUEST.MALFORMED");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn limits_keep_one_client_from_using_up_the_server() {
    let db = "aip_e2e_saas_subscribe_limits";
    let w = world(common::setup_app("saas", db).await).await;
    let limits = Limits { max_connections: 2, max_subscriptions_per_connection: 2, max_rows: 2, ..Limits::default() };
    let (addr, _) = start(w.e.clone(), db, limits).await;

    let mut c = Client::connect(addr).await;
    c.auth(&w.alice).await;
    for id in ["1", "2"] {
        c.subscribe(id, "LiveProjectTasks", json!({"project": w.pa})).await;
        assert_eq!(c.next().await["type"], "snapshot");
    }
    c.subscribe("3", "LiveProjectTasks", json!({"project": w.pa})).await;
    let m = c.next().await;
    assert_eq!((m["id"].as_str(), m["code"].as_str()), (Some("3"), Some("AIP.SUBSCRIPTION.LIMIT")), "{m}");
    // closing one makes room
    c.send(json!({"type": "unsubscribe", "id": "2"})).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    c.subscribe("3", "LiveProjectTasks", json!({"project": w.pa})).await;
    assert_eq!(c.next().await["type"], "snapshot");

    // too many connections: the third upgrade is refused
    let _second = Client::connect(addr).await;
    let third = tokio_tungstenite::connect_async(format!("ws://{addr}/aip/subscribe")).await;
    assert!(third.is_err(), "a third connection is refused above the limit");

    // a result larger than a subscription may carry ends that subscription, not the connection
    for t in ["a", "b", "c"] {
        w.task(&w.alice, &w.pa, t).await;
    }
    let mut seen = Vec::new();
    while let Some(m) = c.next_within(1500).await {
        seen.push(m);
    }
    assert!(seen.iter().any(|m| m["code"] == "AIP.SUBSCRIPTION.TOO_LARGE"), "{seen:?}");
}

const IMPERSONATION: &str = r#"
use postgres
use auth

actor Member via auth.oidc(google)

enum MemberRole { USER SUPPORT }

entity Member {
  email: Email
  role: MemberRole = USER
  unique (email) else EMAIL_TAKEN
}

entity Note {
  body: Text(1..100)
}

impersonate Member by actor.role = SUPPORT audited reason required ttl 30m

subscribe LiveNotes() {
  allow authenticated
  from Note n
  select { id body }
}

subscribe LiveMembers() {
  allow authenticated
  from Member m
  select { id email }
}

command AddNote(body: Text(1..100)) {
  allow authenticated
  do { insert Note { body } }
}
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_impersonation_token_is_only_good_while_its_session_is_open() {
    let db = "aip_e2e_subscribe_impersonation";
    let checked = aip_sema::pipeline::check_source(IMPERSONATION);
    let (core, map) = checked.into_core().expect("compiles");
    let e = common::setup_program(db, aip_pg::compile(&core, &map).program).await;
    let (addr, _) = start(e.clone(), db, Limits::default()).await;
    let sam = scalar(&e, "INSERT INTO member (email, role) VALUES ('sam@x.com', 'SUPPORT') RETURNING id::text").await;
    let alice = scalar(&e, "INSERT INTO member (email) VALUES ('alice@x.com') RETURNING id::text").await;
    let started = ok(&e, "StartMemberImpersonation", Some(&sam), json!({"target": alice, "reason": "ticket 7"}), None).await;
    let session = started["session"].as_str().expect("session").to_string();
    let exp = chrono::Utc::now().timestamp() + 600;
    let token = auth::issue_impersonation(TEST_SECRET, &alice, &session, exp);

    let mut c = Client::connect(addr).await;
    c.send(json!({"type": "auth", "token": token})).await;
    c.subscribe("n", "LiveNotes", json!({})).await;
    assert_eq!(c.next().await["type"], "snapshot");
    ok(&e, "AddNote", Some(&alice), json!({"body": "one"}), None).await;
    assert_eq!(c.next().await["type"], "changed");

    // the operator ends the session: the next change finds the token no longer good
    ok(&e, "StopMemberImpersonation", Some(&sam), json!({}), None).await;
    ok(&e, "AddNote", Some(&alice), json!({"body": "two"}), None).await;
    let m = c.next().await;
    assert_eq!((m["type"].as_str(), m["code"].as_str()), (Some("error"), Some("AIP.AUTH.UNAUTHENTICATED")), "{m}");
    assert_eq!(m["reason"], "IMPERSONATION_ENDED");
    ok(&e, "AddNote", Some(&alice), json!({"body": "three"}), None).await;
    assert!(c.silent(700).await, "nothing follows the error");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_change_wakes_only_the_subscriptions_that_read_it() {
    let db = "aip_e2e_subscribe_wake";
    let (core, map) = aip_sema::pipeline::check_source(IMPERSONATION).into_core().expect("compiles");
    let e = common::setup_program(db, aip_pg::compile(&core, &map).program).await;
    let (addr, hub) = start(e.clone(), db, Limits::default()).await;
    let alice = scalar(&e, "INSERT INTO member (email) VALUES ('alice@x.com') RETURNING id::text").await;

    let mut c = Client::connect(addr).await;
    c.auth(&alice).await;
    c.subscribe("notes", "LiveNotes", json!({})).await;
    assert_eq!(c.next().await["type"], "snapshot");
    c.subscribe("members", "LiveMembers", json!({})).await;
    assert_eq!(c.next().await["type"], "snapshot");

    let before = hub.reruns();
    ok(&e, "AddNote", Some(&alice), json!({"body": "one"}), None).await;
    let m = c.next().await;
    assert_eq!((m["type"].as_str(), m["id"].as_str()), (Some("changed"), Some("notes")), "{m}");
    assert!(c.silent(500).await);
    assert_eq!(hub.reruns(), before + 1, "the note changed, so only the subscription that reads notes was run again");

    scalar(&e, "INSERT INTO member (email) VALUES ('bob@x.com') RETURNING id::text").await;
    let m = c.next().await;
    assert_eq!((m["type"].as_str(), m["id"].as_str()), (Some("changed"), Some("members")), "{m}");
    assert_eq!(hub.reruns(), before + 2);
}
