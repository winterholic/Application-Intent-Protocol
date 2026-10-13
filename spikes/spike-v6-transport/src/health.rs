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

/// Returns the HTTP status line and the JSON body.
pub(crate) async fn probe(path: &str, runtime: &Runtime) -> (&'static str, String) {
    if path == "/health" {
        return ("200 OK", r#"{"ok":true,"status":"live"}"#.into());
    }
    match tokio::time::timeout(READY_TIMEOUT, ready(runtime)).await {
        Ok(Ok(())) => ("200 OK", r#"{"ok":true,"status":"ready"}"#.into()),
        Ok(Err(code)) => ("503 Service Unavailable", format!(r#"{{"ok":false,"status":"unready","code":"{code}"}}"#)),
        Err(_) => ("503 Service Unavailable", r#"{"ok":false,"status":"unready","code":"WIRE.DB_UNAVAILABLE"}"#.into()),
    }
}

async fn ready(runtime: &Runtime) -> Result<(), &'static str> {
    let _gate = READY_GATE.acquire().await.map_err(|_| "WIRE.DB_UNAVAILABLE")?;
    let pool = runtime.db_pool.as_ref().ok_or("WIRE.DB_UNAVAILABLE")?;
    let db = pool.acquire().await.map_err(|_| "WIRE.DB_UNAVAILABLE")?;
    let result = async {
        match &runtime.deployment {
            Some(fence) => {
                let row = db
                    .query_opt(&format!("SELECT facts_digest FROM {}.aip_migrate_meta WHERE singleton=true", fence.schema), &[])
                    .await
                    .map_err(|_| "WIRE.DB_UNAVAILABLE")?;
                if row.is_none_or(|row| row.get::<_, String>(0) != fence.expected_digest) {
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
