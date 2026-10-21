//! MySQL 8.4 / MariaDB 11.4 durable message queues (WBS 7.2.1.3): the PostgreSQL `aip_execution::messages::MessageStore` contract
//! over InnoDB.
//!
//! Same contract, same types: an idempotent enqueue scoped to `(owner, queue)` where a replay must carry the same payload, a claim
//! with a lease token and a fencing epoch, ack and nack that only the live lease can commit, a bounded retry budget that ends in
//! a dead-letter set, operator redrive and expiry cleanup. What changes is how InnoDB gets there:
//!
//! * `RETURNING` does not exist, so a claim picks a candidate with `FOR UPDATE SKIP LOCKED`, takes it with a conditional update
//!   (the row count says whether it won) and reads the lease back by its token.
//! * Every deadline comes from the database clock (`UTC_TIMESTAMP(6)`).
//! * A duplicate insert under the idempotency key is read back with a locking read, because a plain read in a repeatable-read
//!   transaction would not see the row another transaction just committed.
use aip_execution::messages::{DeadLetter, DeadLetterPage, Error, Message, MessageLease, NewMessage};
use mysql_async::Value as MyValue;
use mysql_async::prelude::Queryable;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const MAX_PAYLOAD_BYTES: usize = 512 << 10;
const MAX_TTL_MS: i64 = 365 * 24 * 60 * 60 * 1000;
const MAX_DELAY_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_LEASE_MS: i64 = 24 * 60 * 60 * 1000;
const MIN_LEASE_MS: i64 = 100;
const MAX_DELIVERIES: i32 = 100;
const MAX_PAGE_SIZE: i64 = 1_000;
const MAX_BACKOFF_MS: i64 = 30_000;
const MAX_DLQ_PAGE_BYTES: usize = 1 << 20;
/// A claim that loses its candidate to another claimer looks again this many times before it reports an empty queue.
const CLAIM_ATTEMPTS: usize = 8;
/// id, owner, queue, payload, deliveries, max deliveries, epoch, token, worker, lease deadline.
type LeaseRow = (String, String, String, String, i32, i32, i64, String, String, String);
/// id, owner, queue, payload, deliveries, max deliveries, last error, created at.
type DeadRow = (String, String, String, String, i32, i32, Option<String>, String);
const ISO: &str = "'%Y-%m-%dT%H:%i:%s.%fZ'";

fn db(error: mysql_async::Error) -> Error {
    Error::Database(match &error {
        mysql_async::Error::Server(server) => format!("mysql errno {}", server.code),
        _ => "mysql connection error".to_string(),
    })
}

fn text(value: Option<&MyValue>) -> Option<String> {
    match value? {
        MyValue::Bytes(bytes) => String::from_utf8(bytes.clone()).ok(),
        MyValue::Int(n) => Some(n.to_string()),
        MyValue::UInt(n) => Some(n.to_string()),
        _ => None,
    }
}

fn int(value: Option<&MyValue>) -> Option<i64> {
    match value? {
        MyValue::Int(n) => Some(*n),
        MyValue::UInt(n) => i64::try_from(*n).ok(),
        MyValue::Bytes(bytes) => std::str::from_utf8(bytes).ok()?.parse().ok(),
        _ => None,
    }
}

fn json(value: Option<&MyValue>) -> Value {
    text(value).and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(Value::Null)
}

/// `now + n milliseconds` on the database clock.
fn after(ms: &str) -> String {
    format!("DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ({ms}) * 1000 MICROSECOND)")
}

#[derive(Clone, Debug)]
pub struct MysqlMessageStore {
    schema: String,
    mariadb: bool,
}

impl MysqlMessageStore {
    pub fn new(schema: impl Into<String>) -> Result<Self, Error> {
        let schema = schema.into();
        let bytes = schema.as_bytes();
        if bytes.len() > 63
            || !bytes.starts_with(b"aip_")
            || !bytes.get(4).is_some_and(u8::is_ascii_lowercase)
            || !bytes[5..].iter().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
        {
            return Err(Error::InvalidSchema);
        }
        Ok(Self { schema, mariadb: false })
    }

    /// MariaDB stores JSON as a checked `LONGTEXT`, so the text is bound as it is instead of `CAST(? AS JSON)`.
    #[must_use]
    pub fn for_mariadb(mut self) -> Self {
        self.mariadb = true;
        self
    }

    fn table(&self) -> String {
        format!("`{}`.`aip_message_queue`", self.schema)
    }

    fn json_param(&self) -> &'static str {
        if self.mariadb { "?" } else { "CAST(? AS JSON)" }
    }

    async fn changed<Q: Queryable>(&self, conn: &mut Q, sql: String, params: Vec<MyValue>) -> Result<bool, Error> {
        conn.exec_drop(sql, params).await.map_err(db)?;
        let changed: Option<u64> = conn.query_first("SELECT ROW_COUNT()").await.map_err(db)?;
        Ok(changed == Some(1))
    }

    /// Inserts a message. The idempotency key is scoped to `(owner, queue)` and a replay must carry the same JSON payload.
    pub async fn enqueue<Q: Queryable>(&self, conn: &mut Q, new: NewMessage) -> Result<Message, Error> {
        validate_message(&new)?;
        let payload = serde_json::to_string(&new.payload).map_err(|error| Error::Database(error.to_string()))?;
        if payload.len() > MAX_PAYLOAD_BYTES {
            return Err(Error::PayloadTooLarge);
        }
        let digest: String = Sha256::digest(payload.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
        let table = self.table();
        let id = uuid::Uuid::new_v4().to_string();
        let inserted = self
            .changed(
                conn,
                format!(
                    "INSERT IGNORE INTO {table} (id, owner, queue, idempotency_key, payload, payload_digest, state, max_deliveries, available_at, expires_at, created_at) \
                     VALUES (?, ?, ?, ?, {}, ?, 'READY', ?, {}, {}, UTC_TIMESTAMP(6))",
                    self.json_param(),
                    after("?"),
                    after("?")
                ),
                vec![
                    id.clone().into(),
                    new.owner.clone().into(),
                    new.queue.clone().into(),
                    new.key.clone().into(),
                    payload.into(),
                    digest.clone().into(),
                    new.max_deliveries.into(),
                    new.delay_ms.into(),
                    new.ttl_ms.into(),
                ],
            )
            .await?;
        let message_id = if inserted {
            id
        } else {
            let row: Option<(String, String)> = conn
                .exec_first(
                    format!("SELECT id, payload_digest FROM {table} WHERE owner = ? AND queue = ? AND idempotency_key = ? FOR UPDATE"),
                    (&new.owner, &new.queue, &new.key),
                )
                .await
                .map_err(db)?;
            let (existing, old_digest) = row.ok_or_else(|| Error::Database("conflicting message disappeared".into()))?;
            if old_digest != digest {
                return Err(Error::IdempotencyConflict);
            }
            existing
        };
        self.get(conn, &new.owner, &new.queue, &message_id).await?.ok_or_else(|| Error::Database("inserted message disappeared".into()))
    }

    /// Claims one ready message. Delivery is at least once: consumers must make side effects idempotent.
    pub async fn claim<Q: Queryable>(
        &self,
        conn: &mut Q,
        owner: &str,
        queue: &str,
        worker: &str,
        lease_ms: i64,
    ) -> Result<Option<MessageLease>, Error> {
        validate_scope(owner, queue)?;
        if !valid_worker(worker) || !(MIN_LEASE_MS..=MAX_LEASE_MS).contains(&lease_ms) {
            return Err(Error::InvalidLease);
        }
        let table = self.table();
        // Expired leases go back to READY, or to DEAD when the delivery budget is spent.
        let expired: Vec<String> = conn
            .exec(
                format!(
                    "SELECT id FROM {table} WHERE owner = ? AND queue = ? AND state = 'CLAIMED' AND lease_until <= UTC_TIMESTAMP(6) \
                     ORDER BY lease_until, id LIMIT 1000 FOR UPDATE SKIP LOCKED"
                ),
                (owner, queue),
            )
            .await
            .map_err(db)?;
        for id in expired {
            conn.exec_drop(
                format!(
                    "UPDATE {table} SET state = CASE WHEN deliveries >= max_deliveries THEN 'DEAD' ELSE 'READY' END, \
                       last_error = CASE WHEN deliveries >= max_deliveries THEN 'LEASE_EXPIRED' ELSE last_error END, \
                       available_at = CASE WHEN deliveries >= max_deliveries THEN available_at ELSE UTC_TIMESTAMP(6) END, \
                       lease_owner = NULL, lease_token = NULL, lease_until = NULL \
                     WHERE id = ? AND state = 'CLAIMED' AND lease_until <= UTC_TIMESTAMP(6)"
                ),
                (id,),
            )
            .await
            .map_err(db)?;
        }
        for _ in 0..CLAIM_ATTEMPTS {
            let candidate: Option<String> = conn
                .exec_first(
                    format!(
                        "SELECT id FROM {table} WHERE owner = ? AND queue = ? AND state = 'READY' AND available_at <= UTC_TIMESTAMP(6) \
                         AND expires_at > UTC_TIMESTAMP(6) ORDER BY available_at, created_at, id LIMIT 1 FOR UPDATE SKIP LOCKED"
                    ),
                    (owner, queue),
                )
                .await
                .map_err(db)?;
            let Some(id) = candidate else { return Ok(None) };
            let token = uuid::Uuid::new_v4().to_string();
            let won = self
                .changed(
                    conn,
                    format!(
                        "UPDATE {table} SET state = 'CLAIMED', deliveries = deliveries + 1, lease_epoch = lease_epoch + 1, lease_owner = ?, \
                           lease_token = ?, lease_until = LEAST(expires_at, {}) WHERE id = ? AND state = 'READY'",
                        after("?")
                    ),
                    vec![worker.into(), token.clone().into(), lease_ms.into(), id.into()],
                )
                .await?;
            if won {
                let row: Option<LeaseRow> = conn
                    .exec_first(
                        format!(
                            "SELECT id, owner, queue, CAST(payload AS CHAR), deliveries, max_deliveries, lease_epoch, lease_token, lease_owner, \
                               DATE_FORMAT(lease_until, {ISO}) FROM {table} WHERE lease_token = ?"
                        ),
                        (&token,),
                    )
                    .await
                    .map_err(db)?;
                let row = row.ok_or_else(|| Error::Database("claimed message disappeared".into()))?;
                return Ok(Some(MessageLease {
                    id: row.0,
                    owner: row.1,
                    queue: row.2,
                    payload: serde_json::from_str(&row.3).map_err(|error| Error::Database(error.to_string()))?,
                    deliveries: row.4,
                    max_deliveries: row.5,
                    epoch: row.6,
                    token: row.7,
                    worker: row.8,
                    lease_until: row.9,
                }));
            }
        }
        Ok(None)
    }

    /// Rows that still belong to this lease: same epoch, token and worker, `CLAIMED`, and the lease has not expired.
    fn live() -> &'static str {
        "id = ? AND owner = ? AND queue = ? AND state = 'CLAIMED' AND lease_epoch = ? AND lease_token = ? AND lease_owner = ? \
         AND lease_until > UTC_TIMESTAMP(6)"
    }

    fn live_params(lease: &MessageLease) -> Result<Vec<MyValue>, Error> {
        parse_lease_id(&lease.id)?;
        parse_lease_id(&lease.token)?;
        Ok(vec![
            lease.id.clone().into(),
            lease.owner.clone().into(),
            lease.queue.clone().into(),
            lease.epoch.into(),
            lease.token.clone().into(),
            lease.worker.clone().into(),
        ])
    }

    /// Acknowledges only the active lease; stale epochs and tokens cannot commit.
    pub async fn ack<Q: Queryable>(&self, conn: &mut Q, lease: &MessageLease) -> Result<bool, Error> {
        let sql = format!(
            "UPDATE {} SET state = 'ACKED', lease_owner = NULL, lease_token = NULL, lease_until = NULL, acknowledged_at = UTC_TIMESTAMP(6) WHERE {}",
            self.table(),
            Self::live()
        );
        self.changed(conn, sql, Self::live_params(lease)?).await
    }

    /// Nacks with capped exponential backoff. The final delivery moves to the dead-letter set.
    pub async fn nack<Q: Queryable>(&self, conn: &mut Q, lease: &MessageLease, error: &str) -> Result<bool, Error> {
        self.nack_after(conn, lease, error, retry_delay_ms(lease.deliveries)).await
    }

    /// Lets the caller's retry policy choose a bounded retry delay explicitly.
    pub async fn nack_after<Q: Queryable>(&self, conn: &mut Q, lease: &MessageLease, error: &str, delay_ms: i64) -> Result<bool, Error> {
        if error.is_empty() || error.len() > 512 || error.chars().any(char::is_control) || !(0..=MAX_DELAY_MS).contains(&delay_ms) {
            return Err(Error::InvalidMessage);
        }
        let sql = format!(
            "UPDATE {} SET state = CASE WHEN deliveries >= max_deliveries THEN 'DEAD' ELSE 'READY' END, \
               available_at = CASE WHEN deliveries >= max_deliveries THEN available_at ELSE {} END, \
               last_error = ?, lease_owner = NULL, lease_token = NULL, lease_until = NULL WHERE {}",
            self.table(),
            after("?"),
            Self::live()
        );
        let mut params: Vec<MyValue> = vec![delay_ms.into(), error.into()];
        params.extend(Self::live_params(lease)?);
        self.changed(conn, sql, params).await
    }

    /// A keyset page of the queue's dead-letter set whose aggregate payload stays within one MiB. `next_cursor` is the last
    /// returned id and continues the listing.
    pub async fn dead_letters_after<Q: Queryable>(
        &self,
        conn: &mut Q,
        owner: &str,
        queue: &str,
        after_id: Option<&str>,
        limit: i64,
    ) -> Result<DeadLetterPage, Error> {
        validate_scope(owner, queue)?;
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(Error::InvalidMessage);
        }
        let after_id = after_id.map(parse_id).transpose()?.unwrap_or_default();
        let table = self.table();
        let rows: Vec<DeadRow> = conn
            .exec(
                format!(
                    "SELECT id, owner, queue, CAST(payload AS CHAR), deliveries, max_deliveries, last_error, DATE_FORMAT(created_at, {ISO}) \
                     FROM {table} WHERE owner = ? AND queue = ? AND state = 'DEAD' AND expires_at > UTC_TIMESTAMP(6) AND id > ? \
                     ORDER BY id LIMIT {}",
                    limit + 1
                ),
                (owner, queue, after_id),
            )
            .await
            .map_err(db)?;
        let mut items = Vec::new();
        let mut bytes = 0usize;
        let mut more = rows.len() as i64 > limit;
        for (index, row) in rows.into_iter().take(limit as usize).enumerate() {
            let row_bytes = row.3.len() + row.1.len() + row.2.len() + row.6.as_ref().map_or(0, String::len) + 256;
            if bytes + row_bytes > MAX_DLQ_PAGE_BYTES && index > 0 {
                more = true;
                break;
            }
            bytes += row_bytes;
            items.push(DeadLetter {
                id: row.0,
                owner: row.1,
                queue: row.2,
                payload: serde_json::from_str(&row.3).map_err(|error| Error::Database(error.to_string()))?,
                deliveries: row.4,
                max_deliveries: row.5,
                last_error: row.6,
                created_at: row.7,
            });
        }
        let next_cursor = if more { items.last().map(|item| item.id.clone()) } else { None };
        Ok(DeadLetterPage { items, next_cursor })
    }

    /// Reopens one dead-letter entry after operator repair, with a fresh delivery budget. The payload stays the original body.
    pub async fn redrive<Q: Queryable>(&self, conn: &mut Q, owner: &str, queue: &str, id: &str, max_deliveries: i32) -> Result<bool, Error> {
        validate_scope(owner, queue)?;
        let id = parse_id(id)?;
        if !(1..=MAX_DELIVERIES).contains(&max_deliveries) {
            return Err(Error::InvalidMessage);
        }
        let sql = format!(
            "UPDATE {} SET state = 'READY', deliveries = 0, max_deliveries = ?, last_error = NULL, available_at = UTC_TIMESTAMP(6) \
             WHERE id = ? AND owner = ? AND queue = ? AND state = 'DEAD' AND expires_at > UTC_TIMESTAMP(6)",
            self.table()
        );
        self.changed(conn, sql, vec![max_deliveries.into(), id.into(), owner.into(), queue.into()]).await
    }

    /// Deletes expired rows in small batches; repeated calls drain large queues.
    pub async fn cleanup_expired<Q: Queryable>(&self, conn: &mut Q, limit: i64) -> Result<Vec<String>, Error> {
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(Error::InvalidMessage);
        }
        let table = self.table();
        let ids: Vec<String> = conn
            .exec(
                format!("SELECT id FROM {table} WHERE expires_at <= UTC_TIMESTAMP(6) ORDER BY expires_at, id LIMIT {limit} FOR UPDATE SKIP LOCKED"),
                (),
            )
            .await
            .map_err(db)?;
        for id in &ids {
            conn.exec_drop(format!("DELETE FROM {table} WHERE id = ?"), (id,)).await.map_err(db)?;
        }
        Ok(ids)
    }

    async fn get<Q: Queryable>(&self, conn: &mut Q, owner: &str, queue: &str, id: &str) -> Result<Option<Message>, Error> {
        let row: Option<mysql_async::Row> = conn
            .exec_first(
                format!(
                    "SELECT id, owner, queue, CAST(payload AS CHAR), state, deliveries, max_deliveries, last_error FROM {} \
                     WHERE id = ? AND owner = ? AND queue = ?",
                    self.table()
                ),
                (id, owner, queue),
            )
            .await
            .map_err(db)?;
        Ok(row.map(|row| Message {
            id: text(row.as_ref(0)).unwrap_or_default(),
            owner: text(row.as_ref(1)).unwrap_or_default(),
            queue: text(row.as_ref(2)).unwrap_or_default(),
            payload: json(row.as_ref(3)),
            state: text(row.as_ref(4)).unwrap_or_default(),
            deliveries: int(row.as_ref(5)).and_then(|n| i32::try_from(n).ok()).unwrap_or_default(),
            max_deliveries: int(row.as_ref(6)).and_then(|n| i32::try_from(n).ok()).unwrap_or_default(),
            last_error: text(row.as_ref(7)),
        }))
    }
}

/// AIP-owned additive infrastructure shared by bootstrap and adoption. Named `aip_message_queue` so it cannot be mistaken for
/// the PostgreSQL `aip_messages`.
pub(crate) fn message_store_ddl(meta_schema: &str) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_message_queue` (\
           id CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL PRIMARY KEY, \
           owner VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, queue VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, \
           idempotency_key VARCHAR(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, payload JSON NOT NULL, \
           payload_digest CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, state VARCHAR(8) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, \
           deliveries INT NOT NULL DEFAULT 0, max_deliveries INT NOT NULL, lease_epoch BIGINT NOT NULL DEFAULT 0, \
           lease_owner VARCHAR(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NULL, lease_token CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NULL, \
           lease_until DATETIME(6) NULL, available_at DATETIME(6) NOT NULL, expires_at DATETIME(6) NOT NULL, last_error VARCHAR(512) NULL, \
           acknowledged_at DATETIME(6) NULL, created_at DATETIME(6) NOT NULL, \
           UNIQUE KEY aip_message_queue_key (owner, queue, idempotency_key), \
           KEY aip_message_queue_claim (owner, queue, state, available_at, created_at, id), \
           KEY aip_message_queue_lease (owner, queue, state, lease_until, id), \
           KEY aip_message_queue_expiry (expires_at, id), UNIQUE KEY aip_message_queue_token (lease_token), \
           CONSTRAINT aip_message_queue_state CHECK (state IN ('READY','CLAIMED','ACKED','DEAD')), \
           CONSTRAINT aip_message_queue_deliveries CHECK (deliveries >= 0 AND max_deliveries BETWEEN 1 AND {MAX_DELIVERIES}), \
           CONSTRAINT aip_message_queue_lease CHECK ((state = 'CLAIMED' AND lease_owner IS NOT NULL AND lease_token IS NOT NULL AND lease_until IS NOT NULL) \
             OR (state <> 'CLAIMED' AND lease_owner IS NULL AND lease_token IS NULL AND lease_until IS NULL))) ENGINE=InnoDB"
    )
}

fn validate_message(message: &NewMessage) -> Result<(), Error> {
    validate_scope(&message.owner, &message.queue)?;
    if !valid_key(&message.key)
        || !(1..=MAX_DELIVERIES).contains(&message.max_deliveries)
        || !(1..=MAX_TTL_MS).contains(&message.ttl_ms)
        || !(0..=MAX_DELAY_MS).contains(&message.delay_ms)
    {
        return Err(Error::InvalidMessage);
    }
    Ok(())
}

fn validate_scope(owner: &str, queue: &str) -> Result<(), Error> {
    if !valid_owner(owner) || !valid_queue(queue) { Err(Error::InvalidMessage) } else { Ok(()) }
}

fn valid_owner(value: &str) -> bool {
    !value.is_empty() && value.len() <= 255 && !value.chars().any(char::is_control)
}

fn valid_queue(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn valid_key(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn valid_worker(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn retry_delay_ms(delivery: i32) -> i64 {
    let exponent = delivery.saturating_sub(1).clamp(0, 15) as u32;
    (1_000_i64.saturating_mul(1_i64 << exponent)).min(MAX_BACKOFF_MS)
}

fn parse_id(value: &str) -> Result<String, Error> {
    uuid::Uuid::parse_str(value).map(|id| id.to_string()).map_err(|_| Error::InvalidMessage)
}

fn parse_lease_id(value: &str) -> Result<String, Error> {
    uuid::Uuid::parse_str(value).map(|id| id.to_string()).map_err(|_| Error::InvalidLease)
}
