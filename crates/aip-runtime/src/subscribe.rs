//! `subscribe`: a query a client keeps open over a WebSocket (`GET /aip/subscribe`) and hears from when its result changes.
//!
//! Messages are JSON text frames. Client to server: `auth` (first, with a token), `subscribe {id, name, input}`,
//! `unsubscribe {id}`. Server to client: `ready`, `snapshot {id, rows}`, `changed {id, rows}` (the whole result again,
//! sent only when it differs from the last one sent), `error {id, code, ...}`. See `spec/grammar.md`.
//!
//! Change detection is PostgreSQL `LISTEN/NOTIFY`: the tables a subscription reads carry a trigger that notifies at
//! commit (`aip_pg::plan::subscribe`), and the [`Hub`] holds the one connection that listens. A connection wakes only the
//! subscriptions that read the table that changed, waits a short while so a burst of commits costs one re-run, and runs
//! the subscription again as the subscriber: the answer is compared with the one sent last and goes out only if it differs.
//! The re-run is the full check of any call (actor, session, allow, visibility, tenant), which is why a subscriber who lost
//! access receives an `error` on the next change and nothing after it.

use crate::auth::{self, Identity};
use crate::engine::{Engine, Impersonation};
use crate::error::AipError;
use crate::http::AppState;
use aip_ir::codes;
use aip_plan::Subscription;
use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::SinkExt;
use futures_util::stream::{SplitSink, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Semaphore, broadcast};
use tokio::time::Instant;

/// Name of the message protocol, sent in `ready`. It is separate from `aip-protocol/N`, which versions the contract document.
pub const WIRE: &str = "aip-subscribe/1";

/// Bounds that keep one client, or all of them together, from using up the server. Fan-out beyond these is OI-10.
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_connections: usize,
    pub max_subscriptions_per_connection: usize,
    pub max_subscriptions: usize,
    /// Re-runs that may be in flight at once, over all connections (each holds a database connection while it runs).
    pub refresh_concurrency: usize,
    /// Changes closer together than this are answered by one re-run.
    pub debounce: Duration,
    /// Largest result a subscription may carry: the whole of it is sent on every change.
    pub max_rows: usize,
    pub auth_timeout: Duration,
    pub max_message_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_connections: 1000,
            max_subscriptions_per_connection: 20,
            max_subscriptions: 5000,
            refresh_concurrency: 8,
            debounce: Duration::from_millis(100),
            max_rows: 1000,
            auth_timeout: Duration::from_secs(5),
            max_message_bytes: 64 * 1024,
        }
    }
}

pub struct Hub {
    engine: Arc<Engine>,
    /// Names of the tables that changed; `*` means any of them may have (the listening connection was lost and came back).
    changes: broadcast::Sender<Arc<str>>,
    pub limits: Limits,
    connections: AtomicUsize,
    subscriptions: AtomicUsize,
    refreshes: Semaphore,
    reruns: AtomicUsize,
}

impl Hub {
    /// How many times a subscription was run again because a table it reads changed (not counting the first snapshot).
    pub fn reruns(&self) -> usize {
        self.reruns.load(Ordering::SeqCst)
    }

    pub async fn start(engine: Arc<Engine>, database_url: &str) -> Arc<Hub> {
        Hub::start_with(engine, database_url, Limits::default()).await
    }

    pub async fn start_with(engine: Arc<Engine>, database_url: &str, limits: Limits) -> Arc<Hub> {
        let (changes, _) = broadcast::channel(1024);
        let hub = Arc::new(Hub {
            engine,
            changes,
            refreshes: Semaphore::new(limits.refresh_concurrency.max(1)),
            limits,
            connections: AtomicUsize::new(0),
            subscriptions: AtomicUsize::new(0),
            reruns: AtomicUsize::new(0),
        });
        if !hub.engine.program.subscriptions.is_empty() {
            let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
            tokio::spawn(listen(database_url.to_string(), hub.changes.clone(), Some(ready_tx)));
            // a client that connects right after start must not be able to commit a change the listener has not heard
            let _ = tokio::time::timeout(Duration::from_secs(5), ready_rx).await;
        }
        hub
    }
}

/// Keeps one connection listening on the change channel, and starts over when it drops.
async fn listen(url: String, tx: broadcast::Sender<Arc<str>>, mut ready: Option<tokio::sync::oneshot::Sender<()>>) {
    use tokio_postgres::AsyncMessage;
    loop {
        match tokio_postgres::connect(&url, tokio_postgres::NoTls).await {
            Ok((client, mut conn)) => {
                let forward = tx.clone();
                let driver = tokio::spawn(async move {
                    let mut messages = futures_util::stream::poll_fn(move |cx| conn.poll_message(cx));
                    while let Some(m) = messages.next().await {
                        match m {
                            Ok(AsyncMessage::Notification(n)) => {
                                let _ = forward.send(n.payload().into());
                            }
                            Ok(_) => {}
                            Err(e) => {
                                tracing::warn!(error = %e, "change listener lost");
                                break;
                            }
                        }
                    }
                });
                match client.batch_execute(&format!("LISTEN {}", aip_plan::CHANGE_CHANNEL)).await {
                    Ok(()) => {
                        if let Some(r) = ready.take() {
                            let _ = r.send(());
                        }
                        // whatever committed while the connection was gone was not heard: every subscription looks again
                        let _ = tx.send("*".into());
                        let _ = driver.await;
                    }
                    Err(e) => tracing::warn!(error = %e, "cannot listen for changes"),
                }
            }
            Err(e) => tracing::warn!(error = %e, "change listener cannot connect"),
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// What the upgrade request already proved, if it carried credentials in headers (clients that can set them).
struct Early {
    identity: Option<(Identity, i64)>,
}

pub async fn upgrade(State(s): State<AppState>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    let hub = s.hub.clone();
    if hub.connections.fetch_add(1, Ordering::SeqCst) >= hub.limits.max_connections {
        hub.connections.fetch_sub(1, Ordering::SeqCst);
        let e = AipError::new(codes::SUBSCRIPTION_LIMIT, "subscribe", "too many connections");
        return (StatusCode::from_u16(e.status()).unwrap_or(StatusCode::TOO_MANY_REQUESTS), axum::Json(json!({ "error": e }))).into_response();
    }
    let guard = ConnGuard(hub.clone());
    let early = match early_identity(&s, &headers) {
        Ok(e) => e,
        Err(e) => return (StatusCode::UNAUTHORIZED, axum::Json(json!({ "error": e }))).into_response(),
    };
    ws.max_message_size(hub.limits.max_message_bytes).max_frame_size(hub.limits.max_message_bytes).on_upgrade(move |socket| async move {
        let _guard = guard;
        Session::new(s, early).run(socket).await;
    })
}

struct ConnGuard(Arc<Hub>);

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::SeqCst);
    }
}

fn early_identity(s: &AppState, headers: &HeaderMap) -> Result<Early, AipError> {
    if let Some(auth) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        let token = auth.strip_prefix("Bearer ").unwrap_or(auth);
        let id = auth::verify_until(&s.secret, token)
            .ok_or_else(|| AipError::new(codes::AUTH_UNAUTHENTICATED, "subscribe", "invalid or expired token"))?;
        return Ok(Early { identity: Some(id) });
    }
    if s.dev_auth
        && let Some(a) = headers.get("x-aip-actor").and_then(|v| v.to_str().ok())
    {
        if uuid::Uuid::parse_str(a).is_err() {
            return Err(AipError::new(codes::AUTH_UNAUTHENTICATED, "subscribe", "x-aip-actor must be a uuid"));
        }
        return Ok(Early { identity: Some((Identity { actor: a.to_lowercase(), session: None }, i64::MAX)) });
    }
    Ok(Early { identity: None })
}

struct Open {
    /// Index into `program.subscriptions`.
    plan: usize,
    input: Value,
    /// What the client holds: the last result sent.
    last: Value,
    dirty: bool,
}

struct Session {
    state: AppState,
    hub: Arc<Hub>,
    /// `None` until the connection has authenticated; `Some(None)` is an anonymous connection (`auth` without a token).
    who: Option<Option<(Identity, i64)>>,
    client: Option<String>,
    subs: HashMap<String, Open>,
}

type Sink = SplitSink<WebSocket, Message>;

/// What handling one message asks of the loop.
enum Next {
    Go,
    Close,
}

impl Session {
    fn new(state: AppState, early: Early) -> Session {
        let hub = state.hub.clone();
        Session { state, hub, who: early.identity.map(Some), client: None, subs: HashMap::new() }
    }

    async fn run(mut self, socket: WebSocket) {
        let (mut sink, mut incoming) = socket.split();
        let mut changes = self.hub.changes.subscribe();
        let started = Instant::now();
        let mut refresh_at: Option<Instant> = None;
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        if self.who.is_some() {
            let _ = send(&mut sink, json!({"type": "ready", "protocol": WIRE})).await;
        }
        loop {
            let auth_due = started + self.hub.limits.auth_timeout;
            tokio::select! {
                frame = incoming.next() => {
                    let Some(Ok(frame)) = frame else { break };
                    match frame {
                        Message::Text(t) => {
                            if let Next::Close = self.message(&mut sink, &t).await { break }
                        }
                        Message::Close(_) => break,
                        Message::Ping(_) | Message::Pong(_) => {}
                        Message::Binary(_) => {
                            let e = AipError::new(codes::REQUEST_MALFORMED, "subscribe", "messages are JSON text frames");
                            let _ = send(&mut sink, error_message(None, &e)).await;
                            break;
                        }
                    }
                }
                changed = changes.recv() => {
                    let table = match changed {
                        Ok(t) => t,
                        // missed some notifications: any table may have changed
                        Err(broadcast::error::RecvError::Lagged(_)) => "*".into(),
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    if self.wake(&table) && refresh_at.is_none() {
                        refresh_at = Some(Instant::now() + self.hub.limits.debounce);
                    }
                }
                _ = sleep_until(refresh_at), if refresh_at.is_some() => {
                    refresh_at = None;
                    match self.refresh(&mut sink).await {
                        Next::Close => break,
                        // a re-run failed for a reason that may pass (database busy): try the rest again shortly
                        Next::Go => if self.subs.values().any(|s| s.dirty) { refresh_at = Some(Instant::now() + Duration::from_secs(1)); }
                    }
                }
                _ = sleep_until(self.who.is_none().then_some(auth_due)), if self.who.is_none() => {
                    let e = AipError::new(codes::AUTH_UNAUTHENTICATED, "subscribe", "no auth message arrived in time");
                    let _ = send(&mut sink, error_message(None, &e)).await;
                    break;
                }
                _ = tick.tick() => {
                    // a token that expired ends the connection's subscriptions; nothing may outlive the credential it was opened with
                    if self.expired() {
                        let e = AipError::new(codes::AUTH_UNAUTHENTICATED, "subscribe", "the token expired");
                        for id in self.subs.keys().cloned().collect::<Vec<_>>() {
                            let _ = send(&mut sink, error_message(Some(&id), &e)).await;
                        }
                        break;
                    }
                }
            }
        }
        self.hub.subscriptions.fetch_sub(self.subs.len(), Ordering::SeqCst);
        let _ = sink.send(Message::Close(Some(CloseFrame { code: 1000, reason: "".into() }))).await;
    }

    fn expired(&self) -> bool {
        matches!(&self.who, Some(Some((_, exp))) if *exp < chrono::Utc::now().timestamp())
    }

    /// Marks the subscriptions that read `table` for a re-run; whether there are any.
    fn wake(&mut self, table: &str) -> bool {
        let program = self.hub.engine.program.clone();
        let mut any = false;
        for s in self.subs.values_mut() {
            if table == "*" || program.subscriptions[s.plan].reads.iter().any(|r| r == table) {
                s.dirty = true;
                any = true;
            }
        }
        any
    }

    async fn message(&mut self, sink: &mut Sink, text: &str) -> Next {
        let Ok(msg) = serde_json::from_str::<Value>(text) else {
            let e = AipError::new(codes::REQUEST_MALFORMED, "subscribe", "a message must be a JSON object");
            let _ = send(sink, error_message(None, &e)).await;
            return Next::Close;
        };
        let id = msg.get("id").and_then(Value::as_str).map(String::from);
        let result = match msg.get("type").and_then(Value::as_str) {
            Some("auth") => self.auth(sink, &msg).await,
            Some("subscribe") => self.subscribe(sink, &msg, id.as_deref()).await,
            Some("unsubscribe") => {
                if let Some(id) = &id
                    && self.subs.remove(id).is_some()
                {
                    self.hub.subscriptions.fetch_sub(1, Ordering::SeqCst);
                }
                Ok(Next::Go)
            }
            _ => Err((None, AipError::new(codes::REQUEST_MALFORMED, "subscribe", "unknown message type"), true)),
        };
        match result {
            Ok(n) => n,
            Err((id, e, fatal)) => {
                let _ = send(sink, error_message(id.as_deref(), &e)).await;
                if fatal { Next::Close } else { Next::Go }
            }
        }
    }

    /// `{type: 'auth', token}`, or `{type: 'auth'}` for an anonymous connection (only `allow public` subscriptions will open),
    /// or, with `--dev-auth`, `{type: 'auth', actor}`.
    async fn auth(&mut self, sink: &mut Sink, msg: &Value) -> Result<Next, (Option<String>, AipError, bool)> {
        let fail = |m: &str| (None, AipError::new(codes::AUTH_UNAUTHENTICATED, "subscribe", m.to_string()), true);
        if self.who.is_some() {
            return Err((None, AipError::new(codes::REQUEST_MALFORMED, "subscribe", "already authenticated"), true));
        }
        let who = if let Some(t) = msg.get("token").and_then(Value::as_str) {
            Some(auth::verify_until(&self.state.secret, t).ok_or_else(|| fail("invalid or expired token"))?)
        } else if let Some(a) = msg.get("actor").and_then(Value::as_str) {
            if !self.state.dev_auth || uuid::Uuid::parse_str(a).is_err() {
                return Err(fail("an actor can only be named with --dev-auth, and must be a uuid"));
            }
            Some((Identity { actor: a.to_lowercase(), session: None }, i64::MAX))
        } else {
            None
        };
        if let Some((id, _)) = &who {
            self.state.engine.actor_exists(&id.actor, "subscribe").await.map_err(|e| (None, e, true))?;
        }
        self.who = Some(who);
        let _ = send(sink, json!({"type": "ready", "protocol": WIRE})).await;
        Ok(Next::Go)
    }

    async fn subscribe(&mut self, sink: &mut Sink, msg: &Value, id: Option<&str>) -> Result<Next, (Option<String>, AipError, bool)> {
        let bad = |m: &str| (id.map(String::from), AipError::new(codes::REQUEST_MALFORMED, "subscribe", m.to_string()), false);
        let Some(who) = self.who.clone() else {
            return Err((id.map(String::from), AipError::new(codes::AUTH_UNAUTHENTICATED, "subscribe", "send an auth message first"), true));
        };
        let Some(id) = id.filter(|i| !i.is_empty() && i.len() <= 64) else {
            return Err(bad("a subscribe message needs an 'id' of 1 to 64 characters"));
        };
        let Some(name) = msg.get("name").and_then(Value::as_str) else { return Err(bad("a subscribe message needs a 'name'")) };
        if self.subs.contains_key(id) {
            return Err(bad("this id is already in use on the connection"));
        }
        let program = self.hub.engine.program.clone();
        let Some(ix) = program.subscriptions.iter().position(|s| s.name == name) else {
            // only declared subscriptions can be opened: a name that is a query or does not exist gets the same answer
            return Err((Some(id.to_string()), AipError::new(codes::REQUEST_UNKNOWN_INTENT, name, format!("no subscription named '{name}'")), false));
        };
        let limits = &self.hub.limits;
        let total = self.hub.subscriptions.fetch_add(1, Ordering::SeqCst);
        if self.subs.len() >= limits.max_subscriptions_per_connection || total >= limits.max_subscriptions {
            self.hub.subscriptions.fetch_sub(1, Ordering::SeqCst);
            return Err((Some(id.to_string()), AipError::new(codes::SUBSCRIPTION_LIMIT, name, "too many open subscriptions"), false));
        }
        let input = msg.get("input").cloned().unwrap_or(Value::Null);
        match self.evaluate(&who, &program.subscriptions[ix], &input).await {
            Ok(rows) => {
                let _ = send(sink, json!({"type": "snapshot", "id": id, "rows": rows})).await;
                self.subs.insert(id.to_string(), Open { plan: ix, input, last: rows, dirty: false });
                Ok(Next::Go)
            }
            Err(e) => {
                self.hub.subscriptions.fetch_sub(1, Ordering::SeqCst);
                Err((Some(id.to_string()), e, false))
            }
        }
    }

    /// One run of a subscription for the connection's subscriber, under the global bound on concurrent runs.
    async fn evaluate(&self, who: &Option<(Identity, i64)>, plan: &Subscription, input: &Value) -> Result<Value, AipError> {
        if matches!(who, Some((_, exp)) if *exp < chrono::Utc::now().timestamp()) {
            return Err(AipError::new(codes::AUTH_UNAUTHENTICATED, &plan.name, "the token expired"));
        }
        let (actor, impersonation) = match who {
            Some((id, _)) => (Some(id.actor.clone()), id.session.clone().map(|session| Impersonation { session, operator: None })),
            None => (None, None),
        };
        let _permit = self.hub.refreshes.acquire().await.map_err(|_| AipError::new(codes::UNAVAILABLE, &plan.name, "shutting down"))?;
        let rows = self.hub.engine.run_subscription(plan, input.clone(), actor, impersonation, self.client.clone()).await?;
        let n = rows.as_array().map_or(0, Vec::len);
        if n > self.hub.limits.max_rows {
            return Err(AipError::new(
                codes::SUBSCRIPTION_TOO_LARGE,
                &plan.name,
                format!("the result has {n} rows; a subscription carries at most {}", self.hub.limits.max_rows),
            ));
        }
        Ok(rows)
    }

    /// Re-runs the subscriptions marked dirty and tells the client what differs. A subscription whose re-run fails ends with an
    /// `error`; one that fails because the database is busy stays marked and is tried again.
    async fn refresh(&mut self, sink: &mut Sink) -> Next {
        let Some(who) = self.who.clone() else { return Next::Go };
        let program = self.hub.engine.program.clone();
        let ids: Vec<String> = self.subs.iter().filter(|(_, s)| s.dirty).map(|(i, _)| i.clone()).collect();
        for id in ids {
            let Some(open) = self.subs.get(&id) else { continue };
            let plan = &program.subscriptions[open.plan];
            let input = open.input.clone();
            self.hub.reruns.fetch_add(1, Ordering::SeqCst);
            match self.evaluate(&who, plan, &input).await {
                Ok(rows) => {
                    let Some(open) = self.subs.get_mut(&id) else { continue };
                    open.dirty = false;
                    if rows != open.last {
                        open.last = rows.clone();
                        if send(sink, json!({"type": "changed", "id": id, "rows": rows})).await.is_err() {
                            return Next::Close;
                        }
                    }
                }
                Err(e) if e.code == codes::UNAVAILABLE || e.code == codes::CONCURRENCY_CONFLICT => {}
                Err(e) => {
                    // the subscriber may no longer see these rows: say so once and stop sending
                    self.subs.remove(&id);
                    self.hub.subscriptions.fetch_sub(1, Ordering::SeqCst);
                    if send(sink, error_message(Some(&id), &e)).await.is_err() {
                        return Next::Close;
                    }
                }
            }
        }
        Next::Go
    }
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending::<()>().await,
    }
}

fn error_message(id: Option<&str>, e: &AipError) -> Value {
    let mut v = serde_json::to_value(e).unwrap_or_else(|_| json!({"code": e.code}));
    if let Some(m) = v.as_object_mut() {
        m.insert("type".into(), json!("error"));
        if let Some(id) = id {
            m.insert("id".into(), json!(id));
        }
    }
    v
}

async fn send(sink: &mut Sink, v: Value) -> Result<(), axum::Error> {
    sink.send(Message::Text(v.to_string().into())).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;

    #[test]
    fn error_messages_carry_the_type_and_the_subscription() {
        let e = AipError::new(codes::AUTH_FORBIDDEN, "X", "no");
        let v = error_message(Some("a"), &e);
        assert_eq!((v["type"].as_str(), v["id"].as_str(), v["code"].as_str()), (Some("error"), Some("a"), Some(codes::AUTH_FORBIDDEN)));
        assert!(error_message(None, &e).get("id").is_none());
    }

    #[test]
    fn a_closed_future_is_not_needed_for_a_missing_deadline() {
        // `sleep_until(None)` never completes, which is what disables a select branch whose guard is off
        assert!(sleep_until(None).now_or_never().is_none());
    }
}
