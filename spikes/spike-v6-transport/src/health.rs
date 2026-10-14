//! Unauthenticated liveness and readiness probes for load balancers and orchestrators.
//!
//! `/health` only proves the listener answers. `/ready` checks that a pooled database connection works
//! and that the stored deployment digest still matches this process, so a stale instance after a migration
//! stops receiving traffic. Responses carry no digests, versions or error details.
use crate::server::Runtime;
use std::time::Duration;

const READY_TIMEOUT: Duration = Duration::from_secs(2);
/// Unauthenticated probes must not drain the request pool; they take at most one connection at a time.
static READY_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

pub(crate) fn is_probe(path: &str) -> bool {
    path == "/health" || path == "/ready"
}

/// Last answer per runtime, served while another check is already running. A flood of unauthenticated `/ready` requests
/// therefore cannot queue behind the gate and starve the load balancer's own probe: it gets the previous answer at once.
static LAST: std::sync::Mutex<Vec<(usize, std::time::Instant, &'static str, String)>> = std::sync::Mutex::new(Vec::new());
/// A previous answer older than this is not served while a check is in flight.
const STALE_LIMIT: Duration = Duration::from_secs(10);

fn remember(key: usize, status: &'static str, body: &str) {
    let mut last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    last.retain(|(k, at, ..)| *k != key && at.elapsed() < Duration::from_secs(300));
    last.push((key, std::time::Instant::now(), status, body.to_owned()));
}

fn recall(key: usize) -> Option<(&'static str, String)> {
    let last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    last.iter().find(|(k, at, ..)| *k == key && at.elapsed() < STALE_LIMIT).map(|(_, _, status, body)| (*status, body.clone()))
}

/// Returns the HTTP status line and the JSON body.
pub(crate) async fn probe(path: &str, runtime: &Runtime) -> (&'static str, String) {
    if path == "/health" {
        return ("200 OK", r#"{"ok":true,"status":"live"}"#.into());
    }
    let key = runtime as *const Runtime as usize;
    // Idle: check fresh. Busy: another check is running, so answer with its predecessor instead of waiting in line.
    let permit = match READY_GATE.try_acquire() {
        Ok(permit) => Some(permit),
        Err(_) => {
            if let Some(answer) = recall(key) {
                return answer;
            }
            None
        }
    };
    let answer = match tokio::time::timeout(READY_TIMEOUT, ready(runtime, permit)).await {
        Ok(Ok(())) => ("200 OK", r#"{"ok":true,"status":"ready"}"#.to_owned()),
        Ok(Err(code)) => ("503 Service Unavailable", format!(r#"{{"ok":false,"status":"unready","code":"{code}"}}"#)),
        Err(_) => ("503 Service Unavailable", r#"{"ok":false,"status":"unready","code":"WIRE.DB_UNAVAILABLE"}"#.to_owned()),
    };
    remember(key, answer.0, &answer.1);
    answer
}

async fn ready(runtime: &Runtime, permit: Option<tokio::sync::SemaphorePermit<'static>>) -> Result<(), &'static str> {
    let _gate = match permit {
        Some(permit) => permit,
        None => READY_GATE.acquire().await.map_err(|_| "WIRE.DB_UNAVAILABLE")?,
    };
    let pool = runtime.db_pool.as_ref().ok_or("WIRE.DB_UNAVAILABLE")?;
    let db = pool.acquire().await.map_err(|_| "WIRE.DB_UNAVAILABLE")?;
    let result = async {
        match &runtime.deployment {
            Some(fence) => {
                let row = db
                    .query_opt(&format!("SELECT facts_digest,facts FROM {}.aip_migrate_meta WHERE singleton=true", fence.schema), &[])
                    .await
                    .map_err(|_| "WIRE.DB_UNAVAILABLE")?;
                if row.as_ref().is_none_or(|row| row.get::<_, String>(0) != fence.expected_digest) {
                    return Err("WIRE.DEPLOYMENT_CHANGED");
                }
                let facts: serde_json::Value = row.unwrap().get(1);
                if !spike_v2_read::external::matches(&*db, &facts).await.map_err(|_| "WIRE.DB_UNAVAILABLE")? {
                    return Err("WIRE.DEPLOYMENT_CHANGED");
                }
            }
            None => {
                db.query_one("SELECT 1", &[]).await.map_err(|_| "WIRE.DB_UNAVAILABLE")?;
            }
        }
        Ok(())
    }
    .await;
    if db.release().await.is_err() {
        return Err("WIRE.DB_UNAVAILABLE");
    }
    result
}
