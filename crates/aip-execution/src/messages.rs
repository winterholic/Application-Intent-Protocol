use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};
use tokio_postgres::GenericClient;
use uuid::Uuid;

const TABLE: &str = "aip_messages";
const MAX_PAYLOAD_BYTES: usize = 512 << 10;
const MAX_TTL_MS: i64 = 365 * 24 * 60 * 60 * 1000;
const MAX_DELAY_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_LEASE_MS: i64 = 24 * 60 * 60 * 1000;
const MIN_LEASE_MS: i64 = 100;
const MAX_DELIVERIES: i32 = 100;
const MAX_POLL_WAIT_MS: i64 = 30_000;
const MAX_PAGE_SIZE: i64 = 1_000;
const MAX_BACKOFF_MS: i64 = 30_000;
const MAX_DLQ_PAGE_BYTES: i64 = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidSchema,
    InvalidMessage,
    InvalidLease,
    InvalidPoll,
    PayloadTooLarge,
    IdempotencyConflict,
    Database(String),
}

impl From<tokio_postgres::Error> for Error {
    fn from(error: tokio_postgres::Error) -> Self {
        Self::Database(format!("{error:?}"))
    }
}

#[derive(Clone, Debug)]
pub struct MessageStore {
    schema: String,
}

impl MessageStore {
    pub fn new(schema: impl Into<String>) -> Result<Self, Error> {
        let schema = schema.into();
        if !valid_schema(&schema) {
            return Err(Error::InvalidSchema);
        }
        Ok(Self { schema })
    }

    fn table(&self) -> String {
        format!("{}.{}", self.schema, TABLE)
    }

    /// Inserts a message in the caller's transaction. The idempotency key is
    /// scoped to `(owner, queue)` and a replay must carry the same JSON payload.
    pub async fn enqueue(&self, client: &impl GenericClient, new: NewMessage) -> Result<Message, Error> {
        validate_message(&new)?;
        let payload_bytes = serde_json::to_vec(&new.payload).map_err(|error| Error::Database(error.to_string()))?;
        if payload_bytes.len() > MAX_PAYLOAD_BYTES {
            return Err(Error::PayloadTooLarge);
        }
        let digest = digest(&payload_bytes);
        let table = self.table();
        let id = Uuid::new_v4();
        let inserted = client
            .query_opt(
                &format!(
                    "INSERT INTO {table} (id,owner,queue,idempotency_key,payload,payload_digest,state,max_deliveries,available_at,expires_at) \
                     VALUES ($1,$2,$3,$4,$5,$6,'READY',$7,clock_timestamp()+($8::bigint*interval '1 millisecond'),clock_timestamp()+($9::bigint*interval '1 millisecond')) \
                     ON CONFLICT (owner,queue,idempotency_key) DO NOTHING RETURNING id"
                ),
                &[&id, &new.owner, &new.queue, &new.key, &new.payload, &digest, &new.max_deliveries, &new.delay_ms, &new.ttl_ms],
            )
            .await?;
        let message_id = if let Some(row) = inserted {
            row.get::<_, Uuid>(0)
        } else {
            let row = client
                .query_opt(
                    &format!("SELECT id,payload_digest FROM {table} WHERE owner=$1 AND queue=$2 AND idempotency_key=$3"),
                    &[&new.owner, &new.queue, &new.key],
                )
                .await?
                .ok_or_else(|| Error::Database("conflicting message disappeared".into()))?;
            if row.get::<_, String>(1) != digest {
                return Err(Error::IdempotencyConflict);
            }
            row.get(0)
        };
        self.get(client, &new.owner, &new.queue, message_id).await?.ok_or_else(|| Error::Database("inserted message disappeared".into()))
    }

    /// Claims one ready message with a bounded `SKIP LOCKED` selection.
    /// Delivery is at least once: workers must make side effects idempotent.
    pub async fn claim(
        &self,
        client: &impl GenericClient,
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
        client
            .execute(
                &format!(
                    "WITH expired AS ( \
                       SELECT id FROM {table} WHERE owner=$1 AND queue=$2 AND state='CLAIMED' AND lease_until<=clock_timestamp() \
                       ORDER BY lease_until,id LIMIT 1000 FOR UPDATE SKIP LOCKED \
                     ) \
                     UPDATE {table} m SET state=CASE WHEN m.deliveries>=m.max_deliveries THEN 'DEAD' ELSE 'READY' END, \
                       last_error=CASE WHEN m.deliveries>=m.max_deliveries THEN 'LEASE_EXPIRED' ELSE m.last_error END, \
                       available_at=CASE WHEN m.deliveries>=m.max_deliveries THEN m.available_at ELSE clock_timestamp() END, \
                       lease_owner=NULL,lease_token=NULL,lease_until=NULL \
                     FROM expired e WHERE m.id=e.id"
                ),
                &[&owner, &queue],
            )
            .await?;

        let token = Uuid::new_v4();
        let row = client
            .query_opt(
                &format!(
                    "WITH candidate AS ( \
                       SELECT id FROM {table} WHERE owner=$1 AND queue=$2 AND state='READY' \
                         AND available_at<=clock_timestamp() AND expires_at>clock_timestamp() \
                       ORDER BY available_at,created_at,id FOR UPDATE SKIP LOCKED LIMIT 1 \
                     ) \
                     UPDATE {table} m SET state='CLAIMED',deliveries=m.deliveries+1,lease_epoch=m.lease_epoch+1, \
                       lease_owner=$3,lease_token=$4,lease_until=LEAST(m.expires_at,clock_timestamp()+($5::bigint*interval '1 millisecond')) \
                     FROM candidate c WHERE m.id=c.id \
                     RETURNING m.id,m.owner,m.queue,m.payload,m.deliveries,m.max_deliveries,m.lease_epoch,m.lease_token,m.lease_owner, \
                       to_char(m.lease_until AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')"
                ),
                &[&owner, &queue, &worker, &token, &lease_ms],
            )
            .await?;
        row.map(MessageLease::from_row).transpose()
    }

    /// Polls until a message is available or the caller's finite wait expires.
    #[allow(clippy::too_many_arguments)]
    pub async fn poll_claim(
        &self,
        client: &impl GenericClient,
        owner: &str,
        queue: &str,
        worker: &str,
        lease_ms: i64,
        wait_ms: i64,
        interval_ms: i64,
    ) -> Result<Option<MessageLease>, Error> {
        if !(0..=MAX_POLL_WAIT_MS).contains(&wait_ms) || !(10..=1_000).contains(&interval_ms) {
            return Err(Error::InvalidPoll);
        }
        let polls = (wait_ms + interval_ms - 1) / interval_ms;
        for poll in 0..=polls {
            if let Some(lease) = self.claim(client, owner, queue, worker, lease_ms).await? {
                return Ok(Some(lease));
            }
            if poll == polls {
                return Ok(None);
            }
            let remaining = wait_ms.saturating_sub(poll.saturating_mul(interval_ms));
            tokio::time::sleep(std::time::Duration::from_millis(interval_ms.min(remaining) as u64)).await;
        }
        Ok(None)
    }

    /// Acknowledges only the active lease; stale epochs/tokens cannot commit.
    pub async fn ack(&self, client: &impl GenericClient, lease: &MessageLease) -> Result<bool, Error> {
        let id = parse_lease_id(&lease.id)?;
        let token = parse_lease_id(&lease.token)?;
        let table = self.table();
        let changed = client
            .execute(
                &format!(
                    "UPDATE {table} SET state='ACKED',lease_owner=NULL,lease_token=NULL,lease_until=NULL,acknowledged_at=clock_timestamp() \
                     WHERE id=$1 AND owner=$2 AND queue=$3 AND state='CLAIMED' AND lease_epoch=$4 AND lease_token=$5 AND lease_owner=$6 AND lease_until>clock_timestamp()"
                ),
                &[&id, &lease.owner, &lease.queue, &lease.epoch, &token, &lease.worker],
            )
            .await?;
        Ok(changed == 1)
    }

    /// Nacks with capped exponential backoff. The final delivery moves to DLQ.
    pub async fn nack(&self, client: &impl GenericClient, lease: &MessageLease, error: &str) -> Result<bool, Error> {
        let delay_ms = retry_delay_ms(lease.deliveries);
        self.nack_after(client, lease, error, delay_ms).await
    }

    /// Allows a caller's retry policy to choose a bounded retry delay explicitly.
    pub async fn nack_after(&self, client: &impl GenericClient, lease: &MessageLease, error: &str, delay_ms: i64) -> Result<bool, Error> {
        if error.is_empty() || error.len() > 512 || error.chars().any(char::is_control) || !(0..=MAX_DELAY_MS).contains(&delay_ms) {
            return Err(Error::InvalidMessage);
        }
        let id = parse_lease_id(&lease.id)?;
        let token = parse_lease_id(&lease.token)?;
        let table = self.table();
        let changed = client
            .execute(
                &format!(
                    "UPDATE {table} SET \
                       state=CASE WHEN deliveries>=max_deliveries THEN 'DEAD' ELSE 'READY' END, \
                       available_at=CASE WHEN deliveries>=max_deliveries THEN available_at ELSE clock_timestamp()+($7::bigint*interval '1 millisecond') END, \
                       last_error=$8,lease_owner=NULL,lease_token=NULL,lease_until=NULL \
                     WHERE id=$1 AND owner=$2 AND queue=$3 AND state='CLAIMED' AND lease_epoch=$4 AND lease_token=$5 AND lease_owner=$6 AND lease_until>clock_timestamp()"
                ),
                &[&id, &lease.owner, &lease.queue, &lease.epoch, &token, &lease.worker, &delay_ms, &error],
            )
            .await?;
        Ok(changed == 1)
    }

    /// Lists a bounded page from the queue's dead-letter set.
    pub async fn dead_letters(&self, client: &impl GenericClient, owner: &str, queue: &str, limit: i64) -> Result<Vec<DeadLetter>, Error> {
        Ok(self.dead_letters_after(client, owner, queue, None, limit).await?.items)
    }

    /// Returns a keyset page whose aggregate payload stays within one MiB.
    /// `next_cursor` is the last returned UUID and can be supplied to continue.
    pub async fn dead_letters_after(
        &self,
        client: &impl GenericClient,
        owner: &str,
        queue: &str,
        after_id: Option<&str>,
        limit: i64,
    ) -> Result<DeadLetterPage, Error> {
        validate_scope(owner, queue)?;
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(Error::InvalidMessage);
        }
        let after_id = after_id.map(parse_id).transpose()?;
        let table = self.table();
        let rows = client
            .query(
                &format!(
                    "WITH candidates AS MATERIALIZED ( \
                       SELECT id,owner,queue,payload,deliveries,max_deliveries,last_error,created_at, \
                         (octet_length(payload::text)+octet_length(owner)+octet_length(queue)+COALESCE(octet_length(last_error),0)+256)::bigint AS row_bytes \
                       FROM {table} WHERE owner=$1 AND queue=$2 AND state='DEAD' AND expires_at>clock_timestamp() \
                         AND ($3::uuid IS NULL OR id>$3) ORDER BY id LIMIT $4 \
                     ), sized AS ( \
                       SELECT *,sum(row_bytes) OVER(ORDER BY id) AS page_bytes FROM candidates \
                     ), page AS ( \
                       SELECT * FROM sized WHERE page_bytes<=$5::bigint \
                     ), tail AS (SELECT id AS last_id FROM page ORDER BY id DESC LIMIT 1) \
                     SELECT p.id,p.owner,p.queue,p.payload,p.deliveries,p.max_deliveries,p.last_error, \
                       to_char(p.created_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'), \
                       EXISTS(SELECT 1 FROM {table} more,tail WHERE more.owner=$1 AND more.queue=$2 AND more.state='DEAD' \
                         AND more.expires_at>clock_timestamp() AND (tail.last_id IS NULL OR more.id>tail.last_id)) AS has_more \
                     FROM page p CROSS JOIN tail ORDER BY p.id"
                ),
                &[&owner, &queue, &after_id, &limit, &MAX_DLQ_PAGE_BYTES],
            )
            .await?;
        let mut items = Vec::with_capacity(rows.len());
        let mut has_more = false;
        for row in rows {
            has_more = row.get(8);
            items.push(DeadLetter::from_row(row)?);
        }
        let next_cursor = if has_more { items.last().map(|item| item.id.clone()) } else { None };
        Ok(DeadLetterPage { items, next_cursor })
    }

    /// Reopens one DLQ entry after operator repair. The caller chooses the new
    /// delivery budget and the payload remains the original immutable body.
    pub async fn redrive(&self, client: &impl GenericClient, owner: &str, queue: &str, id: &str, max_deliveries: i32) -> Result<bool, Error> {
        validate_scope(owner, queue)?;
        let id = parse_id(id)?;
        if !(1..=MAX_DELIVERIES).contains(&max_deliveries) {
            return Err(Error::InvalidMessage);
        }
        let table = self.table();
        let changed = client
            .execute(
                &format!(
                    "UPDATE {table} SET state='READY',deliveries=0,max_deliveries=$4,last_error=NULL,available_at=clock_timestamp() \
                     WHERE id=$1 AND owner=$2 AND queue=$3 AND state='DEAD' AND expires_at>clock_timestamp()"
                ),
                &[&id, &owner, &queue, &max_deliveries],
            )
            .await?;
        Ok(changed == 1)
    }

    /// Deletes expired rows in small batches; repeated calls drain large queues.
    pub async fn cleanup_expired(&self, client: &impl GenericClient, limit: i64) -> Result<Vec<Uuid>, Error> {
        if !(1..=MAX_PAGE_SIZE).contains(&limit) {
            return Err(Error::InvalidMessage);
        }
        let table = self.table();
        let rows = client
            .query(
                &format!(
                    "WITH expired AS (SELECT id FROM {table} WHERE expires_at<=clock_timestamp() ORDER BY expires_at,id LIMIT $1 FOR UPDATE SKIP LOCKED) \
                     DELETE FROM {table} m USING expired e WHERE m.id=e.id RETURNING m.id"
                ),
                &[&limit],
            )
            .await?;
        Ok(rows.into_iter().map(|row| row.get(0)).collect())
    }

    async fn get(&self, client: &impl GenericClient, owner: &str, queue: &str, id: Uuid) -> Result<Option<Message>, Error> {
        let table = self.table();
        let row = client
            .query_opt(
                &format!(
                    "SELECT id,owner,queue,payload,state,deliveries,max_deliveries,last_error FROM {table} WHERE id=$1 AND owner=$2 AND queue=$3"
                ),
                &[&id, &owner, &queue],
            )
            .await?;
        row.map(Message::from_row).transpose()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NewMessage {
    pub owner: String,
    pub queue: String,
    pub key: String,
    pub payload: Value,
    pub max_deliveries: i32,
    pub ttl_ms: i64,
    pub delay_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub id: String,
    pub owner: String,
    pub queue: String,
    pub payload: Value,
    pub state: String,
    pub deliveries: i32,
    pub max_deliveries: i32,
    pub last_error: Option<String>,
}

impl Message {
    fn from_row(row: tokio_postgres::Row) -> Result<Self, Error> {
        Ok(Self {
            id: row.get::<_, Uuid>(0).to_string(),
            owner: row.get(1),
            queue: row.get(2),
            payload: row.get(3),
            state: row.get(4),
            deliveries: row.get(5),
            max_deliveries: row.get(6),
            last_error: row.get(7),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MessageLease {
    pub id: String,
    pub owner: String,
    pub queue: String,
    pub payload: Value,
    pub deliveries: i32,
    pub max_deliveries: i32,
    pub epoch: i64,
    pub token: String,
    pub worker: String,
    pub lease_until: String,
}

impl MessageLease {
    fn from_row(row: tokio_postgres::Row) -> Result<Self, Error> {
        Ok(Self {
            id: row.get::<_, Uuid>(0).to_string(),
            owner: row.get(1),
            queue: row.get(2),
            payload: row.get(3),
            deliveries: row.get(4),
            max_deliveries: row.get(5),
            epoch: row.get(6),
            token: row.get::<_, Uuid>(7).to_string(),
            worker: row.get(8),
            lease_until: row.get(9),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeadLetter {
    pub id: String,
    pub owner: String,
    pub queue: String,
    pub payload: Value,
    pub deliveries: i32,
    pub max_deliveries: i32,
    pub last_error: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeadLetterPage {
    pub items: Vec<DeadLetter>,
    pub next_cursor: Option<String>,
}

impl DeadLetter {
    fn from_row(row: tokio_postgres::Row) -> Result<Self, Error> {
        Ok(Self {
            id: row.get::<_, Uuid>(0).to_string(),
            owner: row.get(1),
            queue: row.get(2),
            payload: row.get(3),
            deliveries: row.get(4),
            max_deliveries: row.get(5),
            last_error: row.get(6),
            created_at: row.get(7),
        })
    }
}

/// Provider-facing broker contract for deployments that replace PostgreSQL.
/// Each external backend owns its own retry, lease and deduplication semantics.
pub trait Broker: Send + Sync {
    fn name(&self) -> &'static str;
    fn enqueue<'a>(&'a self, message: NewMessage) -> BrokerFuture<'a, Result<Message, BrokerError>>;
    fn claim<'a>(
        &'a self,
        owner: &'a str,
        queue: &'a str,
        worker: &'a str,
        lease_ms: i64,
    ) -> BrokerFuture<'a, Result<Option<MessageLease>, BrokerError>>;
    fn ack<'a>(&'a self, lease: &'a MessageLease) -> BrokerFuture<'a, Result<bool, BrokerError>>;
    fn nack<'a>(&'a self, lease: &'a MessageLease, error: &'a str) -> BrokerFuture<'a, Result<bool, BrokerError>>;
}

pub type BrokerFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrokerError(pub String);

#[derive(Default)]
pub struct BrokerRegistry {
    providers: HashMap<String, Arc<dyn Broker>>,
}

impl BrokerRegistry {
    pub fn register(&mut self, broker: Arc<dyn Broker>) -> Result<(), Error> {
        let name = broker.name();
        if !valid_provider_name(name) || self.providers.contains_key(name) {
            return Err(Error::InvalidMessage);
        }
        self.providers.insert(name.to_owned(), broker);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Broker>> {
        self.providers.get(name).cloned()
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.providers.keys().cloned().collect();
        names.sort();
        names
    }
}

pub fn ddl(schema: &str) -> Result<String, Error> {
    if !valid_schema(schema) {
        return Err(Error::InvalidSchema);
    }
    Ok(format!(
        "CREATE TABLE {schema}.{TABLE} (\
           id uuid PRIMARY KEY, owner text NOT NULL, queue text NOT NULL, idempotency_key text NOT NULL, \
           payload jsonb NOT NULL, payload_digest text NOT NULL, \
           state text NOT NULL CHECK(state IN ('READY','CLAIMED','ACKED','DEAD')), \
           deliveries integer NOT NULL DEFAULT 0 CHECK(deliveries>=0), max_deliveries integer NOT NULL CHECK(max_deliveries BETWEEN 1 AND {MAX_DELIVERIES}), \
           lease_epoch bigint NOT NULL DEFAULT 0 CHECK(lease_epoch>=0), lease_owner text, lease_token uuid, lease_until timestamptz, \
           available_at timestamptz NOT NULL, expires_at timestamptz NOT NULL, last_error text, acknowledged_at timestamptz, created_at timestamptz NOT NULL DEFAULT clock_timestamp(), \
           CHECK ((state='CLAIMED' AND lease_owner IS NOT NULL AND lease_token IS NOT NULL AND lease_until IS NOT NULL) OR \
                  (state<>'CLAIMED' AND lease_owner IS NULL AND lease_token IS NULL AND lease_until IS NULL)), \
           UNIQUE(owner,queue,idempotency_key)\
         ); \
         CREATE INDEX {TABLE}_claim_idx ON {schema}.{TABLE}(owner,queue,available_at,created_at,id) WHERE state='READY'; \
         CREATE INDEX {TABLE}_lease_idx ON {schema}.{TABLE}(owner,queue,lease_until,id) WHERE state='CLAIMED'; \
         CREATE INDEX {TABLE}_expiry_idx ON {schema}.{TABLE}(expires_at,id); \
         CREATE INDEX {TABLE}_dlq_idx ON {schema}.{TABLE}(owner,queue,created_at,id) WHERE state='DEAD';"
    ))
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

fn valid_schema(value: &str) -> bool {
    value.len() <= 63
        && value.starts_with("aip_")
        && value.as_bytes().get(4).is_some_and(u8::is_ascii_lowercase)
        && value.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn valid_owner(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
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

fn valid_provider_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn digest(payload: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(payload);
    format!("{:x}", hash.finalize())
}

fn retry_delay_ms(delivery: i32) -> i64 {
    let exponent = delivery.saturating_sub(1).clamp(0, 15) as u32;
    (1_000_i64.saturating_mul(1_i64 << exponent)).min(MAX_BACKOFF_MS)
}

fn parse_id(value: &str) -> Result<Uuid, Error> {
    Uuid::parse_str(value).map_err(|_| Error::InvalidMessage)
}

fn parse_lease_id(value: &str) -> Result<Uuid, Error> {
    Uuid::parse_str(value).map_err(|_| Error::InvalidLease)
}
