//! Shared fixed-window admission. PostgreSQL time and row locks arbitrate all instances.
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio_postgres::{Client, GenericClient};

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub limit: u32,
    pub window: Duration,
}
impl Policy {
    pub fn validate(self) -> Result<Self, &'static str> {
        if self.limit == 0 || self.limit > 1_000_000 || self.window < Duration::from_millis(100) || self.window > Duration::from_secs(86400) {
            return Err("invalid rate limit policy");
        }
        Ok(self)
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    Limited { retry_after_ms: u64 },
}
/// Rows kept in `aip_rate_limits`; beyond it the oldest scopes are evicted.
const MAX_SCOPES: i64 = 100_000;

#[derive(Clone)]
pub struct RateLimiter {
    schema: String,
}
impl RateLimiter {
    pub fn new(schema: &str) -> Result<Self, &'static str> {
        if !schema.starts_with("aip_") || schema.len() > 63 || !schema.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_') {
            return Err("invalid limiter schema");
        }
        Ok(Self { schema: schema.into() })
    }
    pub async fn install(&self, db: &impl GenericClient) -> Result<(), tokio_postgres::Error> {
        db.batch_execute(&format!("CREATE TABLE IF NOT EXISTS {}.aip_rate_limits (scope text PRIMARY KEY, used bigint NOT NULL, reset_at timestamptz NOT NULL); CREATE INDEX IF NOT EXISTS aip_rate_limits_expiry ON {}.aip_rate_limits(reset_at)", self.schema, self.schema)).await
    }
    pub async fn check(&self, db: &mut Client, principal: &str, route: &str, policy: Policy) -> Result<Decision, String> {
        policy.validate().map_err(str::to_owned)?;
        if principal.is_empty() || principal.len() > 1024 || route.is_empty() || route.len() > 128 {
            return Err("invalid server admission scope".into());
        }
        let scope = format!("{:x}", Sha256::digest(serde_json::to_vec(&(principal, route)).map_err(|_| "invalid scope")?));
        let table = format!("{}.aip_rate_limits", self.schema);
        let window = policy.window.as_millis() as i64;
        let limit = i64::from(policy.limit);
        let tx = db.transaction().await.map_err(|_| "rate storage unavailable")?;
        tx.batch_execute("SET LOCAL statement_timeout='1s'; SET LOCAL lock_timeout='100ms'").await.map_err(|_| "rate storage unavailable")?;
        // A window that already ended carries no state (the next hit starts a new one), so reclaim a bounded batch of
        // them on every check; never accumulate one row per time window.
        tx.execute(&format!("DELETE FROM {table} WHERE scope IN (SELECT scope FROM {table} WHERE reset_at<clock_timestamp() ORDER BY reset_at LIMIT 128 FOR UPDATE SKIP LOCKED)"), &[]).await.map_err(|_| "rate storage unavailable")?;
        if tx.query_opt(&format!("SELECT 1 FROM {table} WHERE scope=$1"), &[&scope]).await.map_err(|_| "rate storage unavailable")?.is_none() {
            // Serialize only new keys, so authenticated users cannot grow the table without bound.
            tx.query_one("SELECT pg_advisory_xact_lock(1095323726,hashtext($1))", &[&self.schema]).await.map_err(|_| "rate storage unavailable")?;
            let count: i64 = tx.query_one(&format!("SELECT count(*) FROM {table}"), &[]).await.map_err(|_| "rate storage unavailable")?.get(0);
            if count >= MAX_SCOPES {
                // Every scope still has a live window. Make room by evicting the ones that reset soonest: they hold the least
                // quota history, and refusing a new principal outright would lock out real users behind a flood of others.
                let evicted = tx
                    .execute(
                        &format!("DELETE FROM {table} WHERE scope IN (SELECT scope FROM {table} ORDER BY reset_at LIMIT 128 FOR UPDATE SKIP LOCKED)"),
                        &[],
                    )
                    .await
                    .map_err(|_| "rate storage unavailable")?;
                if evicted == 0 {
                    return Err("rate scope quota exceeded".into());
                }
            }
        }
        let admitted = tx.query_opt(&format!("INSERT INTO {table} AS current(scope,used,reset_at) VALUES($1,1,clock_timestamp()+$2::bigint*interval '1 millisecond') ON CONFLICT(scope) DO UPDATE SET used=CASE WHEN current.reset_at<=clock_timestamp() THEN 1 ELSE current.used+1 END, reset_at=CASE WHEN current.reset_at<=clock_timestamp() THEN clock_timestamp()+$2::bigint*interval '1 millisecond' ELSE current.reset_at END WHERE current.reset_at<=clock_timestamp() OR current.used<$3 RETURNING used"), &[&scope, &window, &limit]).await.map_err(|_| "rate storage unavailable")?.is_some();
        let decision = if admitted {
            Decision::Allowed
        } else {
            let retry: i64 = tx
                .query_one(
                    &format!("SELECT GREATEST(1,ceil(extract(epoch FROM(reset_at-clock_timestamp()))*1000))::bigint FROM {table} WHERE scope=$1"),
                    &[&scope],
                )
                .await
                .map_err(|_| "rate storage unavailable")?
                .get(0);
            Decision::Limited { retry_after_ms: retry.max(1) as u64 }
        };
        tx.commit().await.map_err(|_| "rate storage unavailable")?;
        Ok(decision)
    }
}
