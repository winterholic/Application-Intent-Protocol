//! Durable owner-scoped event streams with replay cursors and explicit gaps.

use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio_postgres::{Client, GenericClient};

const STREAMS: &str = "aip_event_streams";
const EVENTS: &str = "aip_events";
const KEYS: &str = "aip_event_keys";
pub const MAX_SCAN_LIMIT: i64 = 1000;
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;
pub const MAX_SCAN_BYTES: i64 = 1024 * 1024;
const MAX_RETAINED_EVENTS: i64 = 1000;
/// Idempotency keys outlive the replay window by this many cursors, then are dropped so the ledger stays bounded (O-20).
const DEFAULT_LEDGER_WINDOW: i64 = 10_000;
/// The ledger is trimmed once per this many events of a stream, so the range delete cost is amortized.
const LEDGER_PRUNE_INTERVAL: i64 = 100;

#[derive(Debug, PartialEq)]
pub enum Error {
    InvalidSchema,
    InvalidEvent,
    IdempotencyConflict,
    CursorExpired,
    Database(String),
}

impl From<tokio_postgres::Error> for Error {
    fn from(error: tokio_postgres::Error) -> Self {
        Self::Database(format!("{error:?}"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// Decimal string on the wire so JavaScript does not round large cursors.
    pub cursor: String,
    pub idempotency_key: String,
    pub payload: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub events: Vec<Event>,
    pub next_cursor: String,
}

#[derive(Clone, Debug)]
pub struct Store {
    schema: String,
    ledger_window: i64,
}

impl Store {
    pub fn new(schema: impl Into<String>) -> Result<Self, Error> {
        let schema = schema.into();
        if !valid_schema(&schema) {
            return Err(Error::InvalidSchema);
        }
        Ok(Self { schema, ledger_window: DEFAULT_LEDGER_WINDOW })
    }

    /// How many cursors of idempotency keys are kept beyond the replay window. A key older than that no longer
    /// deduplicates, so a publisher retrying that late appends a new event instead of getting the old cursor.
    pub fn with_ledger_window(mut self, cursors: i64) -> Self {
        self.ledger_window = cursors.max(0);
        self
    }

    fn table(&self, name: &str) -> String {
        format!("{}.{}", self.schema, name)
    }

    pub fn ddl(&self) -> String {
        format!(
            "CREATE TABLE IF NOT EXISTS {} (owner text NOT NULL, stream text NOT NULL, head bigint NOT NULL DEFAULT 0 CHECK(head >= 0), \
             retained_from bigint NOT NULL DEFAULT 1 CHECK(retained_from > 0), PRIMARY KEY(owner,stream)); \
             CREATE TABLE IF NOT EXISTS {} (owner text NOT NULL, stream text NOT NULL, cursor bigint NOT NULL CHECK(cursor > 0), \
             idempotency_key text NOT NULL, payload jsonb NOT NULL, created_at timestamptz NOT NULL DEFAULT clock_timestamp(), \
             PRIMARY KEY(owner,stream,cursor)); \
             CREATE TABLE IF NOT EXISTS {} (owner text NOT NULL, stream text NOT NULL, idempotency_key text NOT NULL, \
             cursor bigint NOT NULL CHECK(cursor > 0), payload_digest bytea NOT NULL, PRIMARY KEY(owner,stream,idempotency_key))",
            self.table(STREAMS),
            self.table(EVENTS),
            self.table(KEYS),
        )
    }

    /// Appends once per idempotency key. Reusing the key with another payload
    /// is a conflict, including after the event has left the replay window, until the key itself is
    /// trimmed (`with_ledger_window` cursors later).
    pub async fn publish(&self, client: &mut Client, owner: &str, stream: &str, idempotency_key: &str, payload: Value) -> Result<String, Error> {
        let tx = client.transaction().await?;
        match self.publish_in(&tx, owner, stream, idempotency_key, payload).await {
            Ok(cursor) => {
                tx.commit().await?;
                Ok(cursor)
            }
            Err(error) => {
                tx.rollback().await?;
                Err(error)
            }
        }
    }

    /// Appends using a caller-owned transaction, allowing a run outcome and
    /// its event to become visible atomically.
    pub async fn publish_in<C: GenericClient>(
        &self,
        client: &C,
        owner: &str,
        stream: &str,
        idempotency_key: &str,
        payload: Value,
    ) -> Result<String, Error> {
        if !valid_owner(owner) || !valid_name(stream) || !valid_key(idempotency_key) || payload_size(&payload) > MAX_PAYLOAD_BYTES {
            return Err(Error::InvalidEvent);
        }
        let digest = Sha256::digest(serde_json::to_vec(&payload).map_err(|_| Error::InvalidEvent)?).to_vec();
        let streams = self.table(STREAMS);
        let events = self.table(EVENTS);
        let keys = self.table(KEYS);
        client.execute(&format!("INSERT INTO {streams} (owner,stream) VALUES ($1,$2) ON CONFLICT DO NOTHING"), &[&owner, &stream]).await?;
        let head: i64 =
            client.query_one(&format!("SELECT head FROM {streams} WHERE owner=$1 AND stream=$2 FOR UPDATE"), &[&owner, &stream]).await?.get(0);
        if let Some(row) = client
            .query_opt(
                &format!("SELECT cursor,payload_digest FROM {keys} WHERE owner=$1 AND stream=$2 AND idempotency_key=$3"),
                &[&owner, &stream, &idempotency_key],
            )
            .await?
        {
            let old_digest: Vec<u8> = row.get(1);
            if old_digest != digest {
                return Err(Error::IdempotencyConflict);
            }
            let cursor: i64 = row.get(0);
            return Ok(cursor.to_string());
        }
        let cursor = head.checked_add(1).ok_or(Error::InvalidEvent)?;
        let retained_from = cursor.saturating_sub(MAX_RETAINED_EVENTS - 1).max(1);
        client
            .execute(
                &format!("UPDATE {streams} SET head=$3,retained_from=GREATEST(retained_from,$4) WHERE owner=$1 AND stream=$2"),
                &[&owner, &stream, &cursor, &retained_from],
            )
            .await?;
        client
            .execute(
                &format!("INSERT INTO {events} (owner,stream,cursor,idempotency_key,payload) VALUES ($1,$2,$3,$4,$5)"),
                &[&owner, &stream, &cursor, &idempotency_key, &payload],
            )
            .await?;
        client
            .execute(
                &format!("INSERT INTO {keys} (owner,stream,idempotency_key,cursor,payload_digest) VALUES ($1,$2,$3,$4,$5)"),
                &[&owner, &stream, &idempotency_key, &cursor, &digest],
            )
            .await?;
        client.execute(&format!("DELETE FROM {events} WHERE owner=$1 AND stream=$2 AND cursor<$3"), &[&owner, &stream, &retained_from]).await?;
        if cursor % LEDGER_PRUNE_INTERVAL == 0 {
            let ledger_floor = retained_from.saturating_sub(self.ledger_window);
            client.execute(&format!("DELETE FROM {keys} WHERE owner=$1 AND stream=$2 AND cursor<$3"), &[&owner, &stream, &ledger_floor]).await?;
        }
        Ok(cursor.to_string())
    }

    /// Scans a stable bounded page after the supplied decimal cursor. A cursor
    /// older than retained data is an error rather than a silent partial replay.
    pub async fn scan<C: GenericClient>(&self, client: &C, owner: &str, stream: &str, after: &str, limit: i64) -> Result<Page, Error> {
        if !valid_owner(owner) || !valid_name(stream) || !(1..=MAX_SCAN_LIMIT).contains(&limit) {
            return Err(Error::InvalidEvent);
        }
        let cursor = parse_cursor(after)?;
        let streams = self.table(STREAMS);
        let events = self.table(EVENTS);
        // State and rows share one PostgreSQL statement snapshot, so a concurrent
        // prune cannot make this scan return a page with a silent cursor gap.
        let rows = client
            .query(
                &format!(
                    "SELECT s.head,s.retained_from,p.cursor,p.idempotency_key,p.payload \
                     FROM {streams} s LEFT JOIN LATERAL ( \
                       SELECT cursor,idempotency_key,payload FROM ( \
                         SELECT cursor,idempotency_key,payload, \
                           sum(octet_length(payload::text)) OVER (ORDER BY cursor) AS page_bytes \
                         FROM {events} WHERE owner=s.owner AND stream=s.stream AND cursor>$3 \
                         ORDER BY cursor LIMIT $4 \
                       ) candidates WHERE page_bytes <= $5 ORDER BY cursor \
                     ) p ON true WHERE s.owner=$1 AND s.stream=$2 ORDER BY p.cursor"
                ),
                &[&owner, &stream, &cursor, &limit, &MAX_SCAN_BYTES],
            )
            .await?;
        let Some(state) = rows.first() else {
            return if cursor == 0 { Ok(Page { events: Vec::new(), next_cursor: after.to_owned() }) } else { Err(Error::InvalidEvent) };
        };
        let head: i64 = state.get(0);
        let floor: i64 = state.get(1);
        if cursor < floor.saturating_sub(1) {
            return Err(Error::CursorExpired);
        }
        if cursor > head {
            return Err(Error::InvalidEvent);
        }
        let mut found = Vec::with_capacity(rows.len());
        for row in rows {
            if let Some(event_cursor) = row.get::<_, Option<i64>>(2) {
                found.push(Event { cursor: event_cursor.to_string(), idempotency_key: row.get(3), payload: row.get(4) });
            }
        }
        let next_cursor = found.last().map(|event| event.cursor.clone()).unwrap_or_else(|| after.to_owned());
        Ok(Page { events: found, next_cursor })
    }

    /// Drops events older than `days` from up to `max_streams` streams, raising each stream's replay floor past them so an
    /// older cursor reads `CursorExpired` instead of a silent gap. Events of a stream are appended in cursor order under
    /// the stream's row lock, so "older than" is always a prefix of the stream. The idempotency ledger and stream heads
    /// stay, as with `prune`. Returns the number of events deleted. `created_at` has no index, so each pass reads the
    /// events table once; run it infrequently.
    pub async fn expire_older_than<C: GenericClient>(&self, client: &C, days: i64, max_streams: i64) -> Result<u64, Error> {
        if !(1..=36_500).contains(&days) || !(1..=10_000).contains(&max_streams) {
            return Err(Error::InvalidEvent);
        }
        let days = days as i32;
        let streams = self.table(STREAMS);
        let events = self.table(EVENTS);
        let deleted = client
            .execute(
                &format!(
                    "WITH old AS ( \
                       SELECT owner,stream,max(cursor) AS through FROM {events} \
                       WHERE created_at < clock_timestamp() - make_interval(days => $1) GROUP BY owner,stream LIMIT $2), \
                     locked AS ( \
                       SELECT s.owner,s.stream,old.through FROM {streams} s JOIN old ON old.owner=s.owner AND old.stream=s.stream \
                       FOR UPDATE OF s SKIP LOCKED), \
                     raised AS ( \
                       UPDATE {streams} s SET retained_from=GREATEST(s.retained_from, locked.through+1) FROM locked \
                       WHERE s.owner=locked.owner AND s.stream=locked.stream RETURNING s.owner,s.stream,locked.through) \
                     DELETE FROM {events} e USING raised WHERE e.owner=raised.owner AND e.stream=raised.stream AND e.cursor<=raised.through"
                ),
                &[&days, &max_streams],
            )
            .await?;
        Ok(deleted)
    }

    /// Deletes replay payloads through a cursor while keeping the idempotency
    /// ledger and the stream head. Older clients then receive CursorExpired.
    pub async fn prune(&self, client: &mut Client, owner: &str, stream: &str, through: &str) -> Result<(), Error> {
        if !valid_owner(owner) || !valid_name(stream) {
            return Err(Error::InvalidEvent);
        }
        let cursor = parse_cursor(through)?;
        let tx = client.transaction().await?;
        let streams = self.table(STREAMS);
        let events = self.table(EVENTS);
        let row =
            tx.query_opt(&format!("SELECT head,retained_from FROM {streams} WHERE owner=$1 AND stream=$2 FOR UPDATE"), &[&owner, &stream]).await?;
        if let Some(row) = row {
            let head: i64 = row.get(0);
            let old_floor: i64 = row.get(1);
            let new_floor = cursor.saturating_add(1).min(head.saturating_add(1)).max(old_floor);
            tx.execute(&format!("DELETE FROM {events} WHERE owner=$1 AND stream=$2 AND cursor<$3"), &[&owner, &stream, &new_floor]).await?;
            tx.execute(&format!("UPDATE {streams} SET retained_from=$3 WHERE owner=$1 AND stream=$2"), &[&owner, &stream, &new_floor]).await?;
        }
        tx.commit().await?;
        Ok(())
    }
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

fn valid_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn valid_key(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn parse_cursor(value: &str) -> Result<i64, Error> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Error::InvalidEvent);
    }
    value.parse::<i64>().map_err(|_| Error::InvalidEvent)
}

fn payload_size(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}
