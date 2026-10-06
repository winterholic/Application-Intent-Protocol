//! HTTP transport: one endpoint per intent, JSON in and out; multipart for uploads.

use crate::engine::{Call, Engine, Impersonation};
use crate::error::AipError;
use crate::objects::Upload;
use aip_ir::codes;
use axum::body::Body;
use axum::extract::{Multipart, Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<Engine>,
    pub secret: Arc<Vec<u8>>,
    pub dev_auth: bool,
    pub trusted_proxies: bool,
    pub contract: Arc<Value>,
    pub hub: Arc<crate::subscribe::Hub>,
}

impl AppState {
    /// `database_url` is where the subscription hub listens for changes (it needs its own connection).
    pub async fn new(
        engine: Arc<Engine>,
        secret: Arc<Vec<u8>>,
        dev_auth: bool,
        trusted_proxies: bool,
        contract: Arc<Value>,
        database_url: &str,
    ) -> AppState {
        let hub = crate::subscribe::Hub::start(engine.clone(), database_url).await;
        AppState { engine, secret, dev_auth, trusted_proxies, contract, hub }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/aip/describe", get(describe))
        .route("/aip/health", get(|| async { Json(json!({"ok": true})) }))
        .route("/aip/objects/{*key}", get(object))
        .route("/aip/webhooks/{name}", post(webhook))
        .route("/aip/subscribe", get(crate::subscribe::upgrade))
        .route("/aip/{intent}", post(invoke))
        .route("/aip/query/{intent}", post(invoke))
        .route("/aip/command/{intent}", post(invoke))
        .with_state(state)
}

async fn describe(State(s): State<AppState>) -> Json<Value> {
    Json(s.contract.as_ref().clone())
}

async fn object(State(s): State<AppState>, Path(key): Path<String>) -> Response {
    let Some(path) = s.engine.objects.path(&key) else { return StatusCode::NOT_FOUND.into_response() };
    // only objects that were committed are served
    let active = match s.engine.pool.get().await {
        Ok(c) => {
            c.query_opt("SELECT \"content_type\" FROM \"_aip_object\" WHERE \"key\" = $1 AND \"state\" = 'active'", &[&key]).await.ok().flatten()
        }
        Err(_) => None,
    };
    let Some(row) = active else { return StatusCode::NOT_FOUND.into_response() };
    let ct: Option<String> = row.get(0);
    match tokio::fs::read(path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, ct.unwrap_or_else(|| "application/octet-stream".into()))], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn error_response(e: AipError) -> Response {
    let status = StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(json!({ "error": e }))).into_response()
}

/// The actor of a request and, for an impersonation token, the session it was issued for.
async fn actor_of(s: &AppState, headers: &HeaderMap, intent: &str) -> Result<(Option<String>, Option<String>), AipError> {
    if let Some(auth) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        let token = auth.strip_prefix("Bearer ").unwrap_or(auth);
        let id =
            crate::auth::verify(&s.secret, token).ok_or_else(|| AipError::new(codes::AUTH_UNAUTHENTICATED, intent, "invalid or expired token"))?;
        return Ok((Some(id.actor), id.session));
    }
    if s.dev_auth
        && let Some(a) = headers.get("x-aip-actor").and_then(|v| v.to_str().ok())
    {
        if uuid::Uuid::parse_str(a).is_err() {
            return Err(AipError::new(codes::AUTH_UNAUTHENTICATED, intent, "x-aip-actor must be a uuid"));
        }
        return Ok((Some(a.to_lowercase()), None));
    }
    Ok((None, None))
}

async fn actor_exists(s: &AppState, actor: &str, intent: &str) -> Result<(), AipError> {
    s.engine.actor_exists(actor, intent).await
}

fn client_of(s: &AppState, headers: &HeaderMap) -> Option<String> {
    if s.trusted_proxies
        && let Some(f) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok())
    {
        return f.split(',').next().map(|x| x.trim().to_string());
    }
    headers.get("x-aip-client").and_then(|v| v.to_str().ok()).map(String::from)
}

async fn webhook(State(s): State<AppState>, Path(name): Path<String>, headers: axum::http::HeaderMap, body: axum::body::Bytes) -> Response {
    let hs: HashMap<String, String> =
        headers.iter().filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_ascii_lowercase(), v.to_string()))).collect();
    match crate::webhook::receive(&s.engine, &name, &hs, &body).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => error_response(e),
    }
}

async fn invoke(State(s): State<AppState>, Path(intent): Path<String>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let headers = parts.headers.clone();
    let started = std::time::Instant::now();
    let (actor, session) = match actor_of(&s, &headers, &intent).await {
        Ok(a) => a,
        Err(e) => return error_response(e),
    };
    if let Some(a) = &actor
        && let Err(e) = actor_exists(&s, a, &intent).await
    {
        return error_response(e);
    }
    let content_type = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let mut uploads = HashMap::new();
    let input: Value = if content_type.starts_with("multipart/form-data") {
        let req = Request::from_parts(parts, body);
        let mut mp = match <Multipart as axum::extract::FromRequest<()>>::from_request(req, &()).await {
            Ok(m) => m,
            Err(e) => return error_response(AipError::new(codes::REQUEST_MALFORMED, &intent, format!("bad multipart body: {e}"))),
        };
        let mut input = Value::Null;
        loop {
            match mp.next_field().await {
                Ok(Some(field)) => {
                    let name = field.name().unwrap_or_default().to_string();
                    let filename = field.file_name().map(String::from);
                    let ct = field.content_type().map(String::from);
                    let bytes = match field.bytes().await {
                        Ok(b) => b,
                        Err(e) => return error_response(AipError::new(codes::REQUEST_MALFORMED, &intent, format!("bad multipart field: {e}"))),
                    };
                    if name == "input" && filename.is_none() {
                        input = match serde_json::from_slice(&bytes) {
                            Ok(v) => v,
                            Err(_) => return error_response(AipError::new(codes::REQUEST_MALFORMED, &intent, "part 'input' must be JSON")),
                        };
                    } else {
                        uploads.insert(name.clone(), Upload { filename: filename.unwrap_or(name), content_type: ct, bytes });
                    }
                }
                Ok(None) => break,
                Err(e) => return error_response(AipError::new(codes::REQUEST_MALFORMED, &intent, format!("bad multipart body: {e}"))),
            }
        }
        input
    } else {
        let bytes = match axum::body::to_bytes(Body::new(body), 1 << 20).await {
            Ok(b) => b,
            Err(_) => return error_response(AipError::new(codes::REQUEST_MALFORMED, &intent, "body too large (max 1MB without multipart)")),
        };
        if bytes.is_empty() {
            Value::Null
        } else {
            match serde_json::from_slice(&bytes) {
                Ok(v) => v,
                Err(_) => return error_response(AipError::new(codes::REQUEST_MALFORMED, &intent, "body must be JSON")),
            }
        }
    };
    let call = Call {
        intent: intent.clone(),
        input,
        actor: actor.clone(),
        client: client_of(&s, &headers),
        idempotency_key: headers.get("idempotency-key").and_then(|v| v.to_str().ok()).map(String::from),
        uploads,
        flags: HashMap::new(),
        impersonation: session.map(|session| Impersonation { session, operator: None }),
    };
    let result = s.engine.call(call).await;
    let ms = started.elapsed().as_millis();
    match result {
        Ok(mut r) => {
            tracing::info!(intent = %intent, actor = actor.as_deref().unwrap_or("-"), status = 200, ms, "call");
            sign_session(&s, &intent, &mut r.data);
            let mut body = json!({ "data": r.data });
            if let Some(p) = r.page {
                body["page"] = p;
            }
            if let Some(p) = r.partial {
                body["items"] = p;
            }
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => {
            tracing::info!(intent = %intent, actor = actor.as_deref().unwrap_or("-"), status = e.status(), code = %e.code, reason = e.reason.as_deref().unwrap_or(""), ms, "call");
            error_response(e)
        }
    }
}

/// `Start<E>Impersonation` returns the session it opened; the token for it is signed here, where the secret is.
/// The token lives exactly as long as the session and speaks for the target.
fn sign_session(s: &AppState, intent: &str, data: &mut Value) {
    let Some(spec) = &s.engine.program.impersonation else { return };
    if spec.start != intent {
        return;
    }
    let (Some(session), Some(target), Some(expires)) =
        (data["session"].as_str(), data["target"].as_str(), data["expiresAt"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()))
    else {
        return;
    };
    let token = crate::auth::issue_impersonation(&s.secret, target, session, expires.timestamp());
    data["token"] = Value::String(token);
}

pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "aip runtime listening");
    axum::serve(listener, router(state)).await?;
    Ok(())
}
