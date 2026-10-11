use aip_execution::messages::{Error, MessageLease, MessageStore, NewMessage};
use serde_json::{json, Value};
use tokio_postgres::GenericClient;

type Result<T> = std::result::Result<T, &'static str>;
fn error(e: Error) -> &'static str {
    match e {
        Error::IdempotencyConflict => "IDEMPOTENCY_MISMATCH",
        Error::Database(_) => "DB_UNAVAILABLE",
        Error::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
        _ => "BAD_VALUE",
    }
}
fn number(body: &Value, key: &str, default: i64, min: i64, max: i64) -> Result<i64> {
    body.get(key).map_or(Ok(default), |v| v.as_i64().filter(|n| (*n >= min) && (*n <= max)).ok_or("BAD_VALUE"))
}
fn text<'a>(body: &'a Value, key: &str) -> Result<&'a str> {
    body[key].as_str().ok_or("BAD_REQUEST")
}
fn shape(body: &Value, required: &[&str], optional: &[&str]) -> Result<()> {
    if !body.as_object().is_some_and(|o| {
        required.iter().all(|k| o.contains_key(*k)) && o.keys().all(|k| required.contains(&k.as_str()) || optional.contains(&k.as_str()))
    }) {
        return Err("BAD_REQUEST");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn publish(
    db: &impl GenericClient,
    schema: &str,
    owner: &str,
    queue: &str,
    key: &str,
    payload: Value,
    delay_ms: i64,
    ttl_ms: i64,
    max_deliveries: i32,
) -> Result<Value> {
    db.query_one("SELECT pg_advisory_xact_lock(1095323727,hashtext($1))", &[&format!("{schema}:{owner}")]).await.map_err(|_| "DB_UNAVAILABLE")?;
    let row=db.query_one(&format!("SELECT count(*)::bigint,COALESCE(sum(octet_length(payload::text)),0)::bigint, bool_or(queue=$2 AND idempotency_key=$3) FROM {schema}.aip_messages WHERE owner=$1 AND expires_at>clock_timestamp()"),&[&owner,&queue,&key]).await.map_err(|_|"DB_UNAVAILABLE")?;
    if row.get::<_, Option<bool>>(2) != Some(true)
        && (row.get::<_, i64>(0) >= 1000 || row.get::<_, i64>(1) + payload.to_string().len() as i64 > 64 * 1024 * 1024)
    {
        return Err("QUOTA_EXCEEDED");
    }
    let message = MessageStore::new(schema)
        .map_err(error)?
        .enqueue(db, NewMessage { owner: owner.into(), queue: queue.into(), key: key.into(), payload, delay_ms, ttl_ms, max_deliveries })
        .await
        .map_err(error)?;
    Ok(json!({"message":message}))
}

pub(crate) async fn handle(
    db: &impl GenericClient,
    rt: &crate::server::Runtime,
    facts: &Value,
    schema: &str,
    owner: &str,
    path: &str,
    body: &Value,
) -> Result<Value> {
    if !rt.infrastructure.messaging {
        return Err("NOT_EXPOSED");
    }
    match path {
        "/messages/enqueue" if !rt.infrastructure.client_publish => return Err("NOT_EXPOSED"),
        "/messages/claim" | "/messages/ack" | "/messages/nack" | "/messages/dead" | "/messages/redrive" if !rt.infrastructure.client_consume => {
            return Err("NOT_EXPOSED");
        }
        "/messages/enqueue" | "/messages/claim" | "/messages/ack" | "/messages/nack" | "/messages/dead" | "/messages/redrive" => {}
        _ => return Err("NOT_FOUND"),
    }
    let queue = text(body, "queue")?;
    if !facts["operations"].as_object().into_iter().flatten().any(|(_, op)| op["kind"] == "job" && op["access"][format!("messages.{queue}")] == true)
    {
        return Err("NOT_EXPOSED");
    }
    let store = MessageStore::new(schema).map_err(error)?;
    let mut out = match path {
        "/messages/enqueue" => {
            shape(body, &["queue", "key", "payload"], &["delayMs", "ttlMs", "maxDeliveries"])?;
            publish(
                db,
                schema,
                owner,
                queue,
                text(body, "key")?,
                body["payload"].clone(),
                number(body, "delayMs", 0, 0, 86_400_000)?,
                number(body, "ttlMs", 86_400_000, 1, 86_400_000)?,
                number(body, "maxDeliveries", 3, 1, 100)? as i32,
            )
            .await?
        }
        "/messages/claim" => {
            shape(body, &["queue", "worker"], &["leaseMs"])?;
            json!({"lease":store.claim(db,owner,queue,text(body,"worker")?,number(body,"leaseMs",30_000,100,300_000)?).await.map_err(error)?})
        }
        "/messages/ack" | "/messages/nack" => {
            shape(body, &["queue", "lease"], if path.ends_with("nack") { &["error"] } else { &[] })?;
            let mut lease: MessageLease = serde_json::from_value(body["lease"].clone()).map_err(|_| "BAD_REQUEST")?;
            lease.owner = owner.into();
            lease.queue = queue.into();
            if path.ends_with("/ack") {
                json!({"acknowledged":store.ack(db,&lease).await.map_err(error)?})
            } else {
                json!({"released":store.nack(db,&lease,body.get("error").map_or(Ok("CONSUMER_FAILED"),|v|v.as_str().ok_or("BAD_REQUEST"))?).await.map_err(error)?})
            }
        }
        "/messages/dead" => {
            shape(body, &["queue"], &["limit", "after"])?;
            serde_json::to_value(
                store
                    .dead_letters_after(
                        db,
                        owner,
                        queue,
                        body.get("after").map(|v| v.as_str().ok_or("BAD_REQUEST")).transpose()?,
                        number(body, "limit", 100, 1, 100)?,
                    )
                    .await
                    .map_err(error)?,
            )
            .map_err(|_| "INTERNAL")?
        }
        "/messages/redrive" => {
            shape(body, &["queue", "id"], &["maxDeliveries"])?;
            json!({"redriven":store.redrive(db,owner,queue,text(body,"id")?,number(body,"maxDeliveries",3,1,100)? as i32).await.map_err(error)?})
        }
        _ => return Err("NOT_FOUND"),
    };
    out["ok"] = json!(true);
    Ok(out)
}
