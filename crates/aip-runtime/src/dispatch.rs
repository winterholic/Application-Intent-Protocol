//! Outbox dispatcher: delivers events to handlers and runs deferred effects
//! after commit, at least once, with backoff. Handlers record what they
//! processed in the same transaction, so a redelivery is a no-op.

use crate::engine::{Call, Engine};
use crate::exec::{self, Env, ExecCtx};
use aip_ir::codes;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

const MAX_ATTEMPTS: i32 = 10;

pub async fn run(engine: Arc<Engine>) {
    loop {
        match tick(&engine).await {
            Ok(true) => continue,
            Ok(false) => tokio::time::sleep(Duration::from_millis(300)).await,
            Err(e) => {
                tracing::warn!(error = %e, "dispatcher");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

/// Processes one outbox row. Returns true if a row was handled.
pub async fn tick(engine: &Engine) -> anyhow::Result<bool> {
    let mut client = engine.pool.get().await?;
    let tx = client.transaction().await?;
    let Some(row) = tx
        .query_opt(
            "SELECT \"id\", \"kind\", \"name\", \"payload\", \"attempts\" FROM \"_aip_outbox\" WHERE \"done_at\" IS NULL AND \"available_at\" <= now() AND \"attempts\" < $1 ORDER BY \"id\" LIMIT 1 FOR UPDATE SKIP LOCKED",
            &[&MAX_ATTEMPTS],
        )
        .await?
    else {
        return Ok(false);
    };
    let id: i64 = row.get(0);
    let kind: String = row.get(1);
    let name: String = row.get(2);
    let payload: Value = row.get(3);
    let result: Result<Option<chrono::DateTime<chrono::Utc>>, String> = match kind.as_str() {
        "event" => deliver_event(engine, &tx, id, &name, &payload).await.map(|_| None),
        "notify" => notify(&tx, &payload).await.map(|_| None),
        "effect" => effect(engine, &tx, &name, &payload).await,
        "webhook" => deliver_webhook(engine, &tx, id, &name, &payload).await.map(|_| None),
        other => Err(format!("unknown outbox kind {other}")),
    };
    match result {
        Ok(Some(later)) => {
            // not due yet (timer): reschedule without consuming an attempt
            tx.execute("UPDATE \"_aip_outbox\" SET \"available_at\" = $2 WHERE \"id\" = $1", &[&id, &later]).await?;
            tx.commit().await?;
        }
        Ok(None) => {
            let uploads = HashMap::new();
            let mut ctx = ExecCtx {
                program: &engine.program,
                intent: &name,
                uploads: &uploads,
                objects: &engine.objects,
                staged: Vec::new(),
                partial: Vec::new(),
                has_actor: false,
                keys: engine.keys.as_deref(),
            };
            if let Err(e) = exec::settle_rules(&mut ctx, &*tx, &Env::default()).await {
                drop(tx);
                let c2 = engine.pool.get().await?;
                c2.execute(
                    "UPDATE \"_aip_outbox\" SET \"attempts\" = \"attempts\" + 1, \"last_error\" = $2 WHERE \"id\" = $1",
                    &[&id, &e.to_string()],
                )
                .await?;
                return Ok(true);
            }
            tx.execute("UPDATE \"_aip_outbox\" SET \"done_at\" = now() WHERE \"id\" = $1", &[&id]).await?;
            tx.commit().await?;
        }
        Err(msg) => {
            drop(tx);
            let c2 = engine.pool.get().await?;
            c2.execute(
                "UPDATE \"_aip_outbox\" SET \"attempts\" = \"attempts\" + 1, \"last_error\" = $2, \"available_at\" = now() + make_interval(secs => least(3600, power(2, \"attempts\")::int)) WHERE \"id\" = $1",
                &[&id, &msg],
            )
            .await?;
            tracing::warn!(outbox = id, kind, name, error = msg, "delivery failed; will retry");
        }
    }
    Ok(true)
}

async fn deliver_event(engine: &Engine, tx: &deadpool_postgres::Transaction<'_>, id: i64, name: &str, payload: &Value) -> Result<(), String> {
    let data = payload.get("data").cloned().unwrap_or(Value::Null);
    let uploads = HashMap::new();
    for h in engine.program.handlers.iter().filter(|h| h.event == name) {
        let fresh = tx
            .query_opt(
                "INSERT INTO \"_aip_processed\" (\"handler\", \"outbox_id\") VALUES ($1, $2) ON CONFLICT DO NOTHING RETURNING 1",
                &[&h.name, &id],
            )
            .await
            .map_err(|e| e.to_string())?;
        if fresh.is_none() {
            continue;
        }
        let mut env = Env::default();
        env.set("__event", Some(data.to_string()));
        // names the event to a receiver of an `outbound webhooks` form, who dedupes on it
        env.set("__outbox", Some(id.to_string()));
        if let Some(w) = &h.when {
            let rows = exec::query(&**tx, w, &env).await.map_err(|e| e.to_string())?;
            if !rows.first().and_then(|r| r.get::<_, Option<bool>>(0)).unwrap_or(false) {
                continue;
            }
        }
        let mut ctx = ExecCtx {
            program: &engine.program,
            intent: &h.name,
            uploads: &uploads,
            objects: &engine.objects,
            staged: Vec::new(),
            partial: Vec::new(),
            has_actor: false,
            keys: engine.keys.as_deref(),
        };
        exec::run_steps(&mut ctx, &**tx, &h.steps, &mut env).await.map_err(|e| e.to_string())?;
    }
    if payload.get("broker").is_some_and(|b| !b.is_null()) {
        tracing::info!(event = name, "external broker delivery is not configured; kept in outbox history only");
    }
    Ok(())
}

async fn deliver_webhook(engine: &Engine, tx: &deadpool_postgres::Transaction<'_>, id: i64, name: &str, payload: &Value) -> Result<(), String> {
    let Some(w) = engine.program.webhooks.iter().find(|w| w.name == name) else { return Err(format!("webhook '{name}' is no longer defined")) };
    let event = payload.get("event").and_then(|e| e.as_str()).unwrap_or_default();
    let Some(h) = w.handlers.iter().find(|h| h.event == event) else { return Ok(()) };
    let handler = format!("webhook:{name}:{event}");
    let fresh = tx
        .query_opt("INSERT INTO \"_aip_processed\" (\"handler\", \"outbox_id\") VALUES ($1, $2) ON CONFLICT DO NOTHING RETURNING 1", &[&handler, &id])
        .await
        .map_err(|e| e.to_string())?;
    if fresh.is_none() {
        return Ok(());
    }
    let mut env = Env::default();
    env.set(&h.binding, Some(payload.get("data").cloned().unwrap_or(Value::Null).to_string()));
    let uploads = HashMap::new();
    let mut ctx = ExecCtx {
        program: &engine.program,
        intent: &handler,
        uploads: &uploads,
        objects: &engine.objects,
        staged: Vec::new(),
        partial: Vec::new(),
        has_actor: false,
        keys: engine.keys.as_deref(),
    };
    exec::run_steps(&mut ctx, &**tx, &h.steps, &mut env).await.map_err(|e| e.to_string())
}

async fn notify(tx: &deadpool_postgres::Transaction<'_>, payload: &Value) -> Result<(), String> {
    let template = payload.get("template").and_then(|t| t.as_str()).unwrap_or_default().to_string();
    let data = payload.get("data").cloned().unwrap_or(Value::Null);
    let category = payload.get("category").and_then(|c| c.as_str()).map(String::from);
    for r in payload.get("recipients").and_then(|r| r.as_array()).cloned().unwrap_or_default() {
        let Some(id) = r.as_str().and_then(|s| uuid::Uuid::parse_str(s).ok()) else { continue };
        tx.execute(
            "INSERT INTO \"_aip_notification\" (\"recipient\", \"template\", \"data\", \"category\") VALUES ($1, $2, $3, $4)",
            &[&id, &template, &data, &category],
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn effect(
    engine: &Engine,
    tx: &deadpool_postgres::Transaction<'_>,
    name: &str,
    payload: &Value,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, String> {
    match name {
        "s3.delete" => {
            if let Some(k) = payload.get("key").and_then(|k| k.as_str()) {
                engine.objects.remove(k).await;
                tx.execute("DELETE FROM \"_aip_object\" WHERE \"key\" = $1", &[&k]).await.map_err(|e| e.to_string())?;
            }
            Ok(None)
        }
        "timer.run" => {
            let at = payload
                .get("at")
                .and_then(|a| a.as_str())
                .and_then(|a| chrono::DateTime::parse_from_rfc3339(a).ok())
                .map(|a| a.with_timezone(&chrono::Utc));
            if let Some(at) = at
                && at > chrono::Utc::now()
            {
                return Ok(Some(at));
            }
            let intent = payload.get("intent").and_then(|i| i.as_str()).unwrap_or_default().to_string();
            let call = Call {
                intent,
                input: payload.get("args").cloned().unwrap_or(Value::Null),
                actor: payload.get("actor").and_then(|a| a.as_str()).map(String::from),
                idempotency_key: Some(format!("timer:{}", payload)),
                ..Default::default()
            };
            // a timer is how an internal intent runs at all
            match engine.call_internal(call).await {
                Ok(_) => Ok(None),
                Err(e) if e.code == codes::INPUT_IDEMPOTENCY_KEY_UNSUPPORTED => Ok(None),
                Err(e) => Err(e.to_string()),
            }
        }
        n if n.starts_with("mail.") => {
            let to = payload.get("to").or_else(|| payload.get("_0")).and_then(|t| t.as_str()).map(String::from);
            tx.execute("INSERT INTO \"_aip_mail\" (\"template\", \"to\", \"data\") VALUES ($1, $2, $3)", &[&n, &to, payload])
                .await
                .map_err(|e| e.to_string())?;
            Ok(None)
        }
        n if n.starts_with("auth.") || n.starts_with("export.") => {
            tracing::info!(effect = n, payload = %payload, "effect handled by the development provider");
            Ok(None)
        }
        other => Err(format!("no provider for effect '{other}'")),
    }
}
