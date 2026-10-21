//! MySQL 8.4 / MariaDB 11.4 durable event streams (WBS 7.2.1.3): the PostgreSQL `aip_execution::events::Store` contract over InnoDB.
//!
//! Same contract, same types: an owner-scoped stream with a replay cursor, idempotent publish (a repeated key converges on its
//! cursor, the same key with another payload is a conflict even after the event left the replay window), a bounded replay window
//! with an explicit `CursorExpired` instead of a silent gap. What changes is how InnoDB gets there:
//!
//! * The stream row is the serialization point: `SELECT .. FOR UPDATE` on it orders concurrent publishers, like PostgreSQL's.
//! * `LATERAL` does not exist on MariaDB, so a scan reads the stream state and the page in one transaction (one snapshot) and
//!   applies the byte budget in Rust.
//! * Timestamps come from the database clock (`UTC_TIMESTAMP(6)`).
use aip_execution::events::{Error, Event, Page};
use mysql_async::prelude::Queryable;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const MAX_SCAN_LIMIT: i64 = 1000;
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;
pub const MAX_SCAN_BYTES: usize = 1024 * 1024;
const MAX_RETAINED_EVENTS: i64 = 1000;
/// Idempotency keys outlive the replay window by this many cursors, then are dropped so the ledger stays bounded.
const DEFAULT_LEDGER_WINDOW: i64 = 10_000;
/// The ledger is trimmed once per this many events of a stream, so the range delete cost is amortized.
const LEDGER_PRUNE_INTERVAL: i64 = 100;

fn db(error: mysql_async::Error) -> Error {
    Error::Database(match &error {
        mysql_async::Error::Server(server) => format!("mysql errno {}", server.code),
        _ => "mysql connection error".to_string(),
    })
}

#[derive(Clone, Debug)]
pub struct MysqlEventStore {
    schema: String,
    mariadb: bool,
    ledger_window: i64,
}

impl MysqlEventStore {
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
        Ok(Self { schema, mariadb: false, ledger_window: DEFAULT_LEDGER_WINDOW })
    }

    /// MariaDB stores JSON as a checked `LONGTEXT`, so the text is bound as it is instead of `CAST(? AS JSON)`.
    #[must_use]
    pub fn for_mariadb(mut self) -> Self {
        self.mariadb = true;
        self
    }

    /// How many cursors of idempotency keys are kept beyond the replay window.
    #[must_use]
    pub fn with_ledger_window(mut self, cursors: i64) -> Self {
        self.ledger_window = cursors.max(0);
        self
    }

    fn table(&self, name: &str) -> String {
        format!("`{}`.`{name}`", self.schema)
    }

    fn json_param(&self) -> &'static str {
        if self.mariadb { "?" } else { "CAST(? AS JSON)" }
    }

    /// Appends once per idempotency key inside the caller's transaction, so a job's outcome and its event become visible together.
    pub async fn publish_in<Q: Queryable>(&self, tx: &mut Q, owner: &str, stream: &str, key: &str, payload: &Value) -> Result<String, Error> {
        if !valid_owner(owner) || !valid_name(stream) || !valid_key(key) {
            return Err(Error::InvalidEvent);
        }
        let text = serde_json::to_string(payload).map_err(|_| Error::InvalidEvent)?;
        if text.len() > MAX_PAYLOAD_BYTES {
            return Err(Error::InvalidEvent);
        }
        let digest: String = Sha256::digest(text.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect();
        let (streams, events, keys) = (self.table("aip_event_streams"), self.table("aip_event_log"), self.table("aip_event_keys"));
        // The no-op update takes the row's exclusive lock at once. `INSERT IGNORE` takes a shared lock on a duplicate, and two
        // publishers that both hold it deadlock when they upgrade for the `FOR UPDATE` below.
        tx.exec_drop(format!("INSERT INTO {streams}(owner,stream) VALUES(?,?) ON DUPLICATE KEY UPDATE head=head"), (owner, stream))
            .await
            .map_err(db)?;
        let head: i64 = tx
            .exec_first(format!("SELECT head FROM {streams} WHERE owner=? AND stream=? FOR UPDATE"), (owner, stream))
            .await
            .map_err(db)?
            .ok_or(Error::InvalidEvent)?;
        let seen: Option<(i64, String)> = tx
            .exec_first(
                format!("SELECT cursor_value,payload_digest FROM {keys} WHERE owner=? AND stream=? AND idempotency_key=? FOR UPDATE"),
                (owner, stream, key),
            )
            .await
            .map_err(db)?;
        if let Some((cursor, old_digest)) = seen {
            return if old_digest == digest { Ok(cursor.to_string()) } else { Err(Error::IdempotencyConflict) };
        }
        let cursor = head.checked_add(1).ok_or(Error::InvalidEvent)?;
        let retained_from = cursor.saturating_sub(MAX_RETAINED_EVENTS - 1).max(1);
        tx.exec_drop(
            format!("UPDATE {streams} SET head=?,retained_from=GREATEST(retained_from,?) WHERE owner=? AND stream=?"),
            (cursor, retained_from, owner, stream),
        )
        .await
        .map_err(db)?;
        tx.exec_drop(
            format!(
                "INSERT INTO {events}(owner,stream,cursor_value,idempotency_key,payload,created_at) VALUES(?,?,?,?,{},UTC_TIMESTAMP(6))",
                self.json_param()
            ),
            (owner, stream, cursor, key, text.as_str()),
        )
        .await
        .map_err(db)?;
        tx.exec_drop(
            format!("INSERT INTO {keys}(owner,stream,idempotency_key,cursor_value,payload_digest) VALUES(?,?,?,?,?)"),
            (owner, stream, key, cursor, digest.as_str()),
        )
        .await
        .map_err(db)?;
        tx.exec_drop(format!("DELETE FROM {events} WHERE owner=? AND stream=? AND cursor_value<?"), (owner, stream, retained_from))
            .await
            .map_err(db)?;
        if cursor % LEDGER_PRUNE_INTERVAL == 0 {
            let floor = retained_from.saturating_sub(self.ledger_window);
            tx.exec_drop(format!("DELETE FROM {keys} WHERE owner=? AND stream=? AND cursor_value<?"), (owner, stream, floor)).await.map_err(db)?;
        }
        Ok(cursor.to_string())
    }

    /// A stable bounded page after the supplied decimal cursor. A cursor older than the retained data is an error rather than a
    /// silent partial replay. The caller's transaction gives the stream state and the rows one snapshot.
    pub async fn scan<Q: Queryable>(&self, tx: &mut Q, owner: &str, stream: &str, after: &str, limit: i64) -> Result<Page, Error> {
        if !valid_owner(owner) || !valid_name(stream) || !(1..=MAX_SCAN_LIMIT).contains(&limit) {
            return Err(Error::InvalidEvent);
        }
        let cursor = parse_cursor(after)?;
        let (streams, events) = (self.table("aip_event_streams"), self.table("aip_event_log"));
        let state: Option<(i64, i64)> =
            tx.exec_first(format!("SELECT head,retained_from FROM {streams} WHERE owner=? AND stream=?"), (owner, stream)).await.map_err(db)?;
        let Some((head, floor)) = state else {
            return if cursor == 0 { Ok(Page { events: Vec::new(), next_cursor: after.to_owned() }) } else { Err(Error::InvalidEvent) };
        };
        if cursor < floor.saturating_sub(1) {
            return Err(Error::CursorExpired);
        }
        if cursor > head {
            return Err(Error::InvalidEvent);
        }
        let rows: Vec<(i64, String, String)> = tx
            .exec(
                format!(
                    "SELECT cursor_value,idempotency_key,CAST(payload AS CHAR) FROM {events} WHERE owner=? AND stream=? AND cursor_value>? \
                     ORDER BY cursor_value LIMIT {limit}"
                ),
                (owner, stream, cursor),
            )
            .await
            .map_err(db)?;
        let mut found = Vec::with_capacity(rows.len());
        let mut bytes = 0;
        for (event_cursor, key, payload) in rows {
            bytes += payload.len();
            if bytes > MAX_SCAN_BYTES {
                break;
            }
            found.push(Event {
                cursor: event_cursor.to_string(),
                idempotency_key: key,
                payload: serde_json::from_str(&payload).map_err(|_| Error::InvalidEvent)?,
            });
        }
        let next_cursor = found.last().map(|event| event.cursor.clone()).unwrap_or_else(|| after.to_owned());
        Ok(Page { events: found, next_cursor })
    }

    /// Deletes replay payloads through a cursor while keeping the idempotency ledger and the stream head. Older clients then
    /// receive `CursorExpired`.
    pub async fn prune<Q: Queryable>(&self, tx: &mut Q, owner: &str, stream: &str, through: &str) -> Result<(), Error> {
        if !valid_owner(owner) || !valid_name(stream) {
            return Err(Error::InvalidEvent);
        }
        let cursor = parse_cursor(through)?;
        let (streams, events) = (self.table("aip_event_streams"), self.table("aip_event_log"));
        let row: Option<(i64, i64)> = tx
            .exec_first(format!("SELECT head,retained_from FROM {streams} WHERE owner=? AND stream=? FOR UPDATE"), (owner, stream))
            .await
            .map_err(db)?;
        if let Some((head, old_floor)) = row {
            let new_floor = cursor.saturating_add(1).min(head.saturating_add(1)).max(old_floor);
            tx.exec_drop(format!("DELETE FROM {events} WHERE owner=? AND stream=? AND cursor_value<?"), (owner, stream, new_floor))
                .await
                .map_err(db)?;
            tx.exec_drop(format!("UPDATE {streams} SET retained_from=? WHERE owner=? AND stream=?"), (new_floor, owner, stream)).await.map_err(db)?;
        }
        Ok(())
    }
}

/// AIP-owned additive infrastructure shared by bootstrap and adoption. `cursor` is a reserved word in MySQL, hence `cursor_value`.
pub(crate) fn event_store_ddl(meta_schema: &str) -> Vec<String> {
    vec![
        format!(
            "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_event_streams` (\
               owner VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, stream VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, \
               head BIGINT NOT NULL DEFAULT 0, retained_from BIGINT NOT NULL DEFAULT 1, PRIMARY KEY (owner, stream), \
               CONSTRAINT aip_event_streams_cursors CHECK (head >= 0 AND retained_from > 0)) ENGINE=InnoDB"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_event_log` (\
               owner VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, stream VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, \
               cursor_value BIGINT NOT NULL, idempotency_key VARCHAR(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, payload JSON NOT NULL, \
               created_at DATETIME(6) NOT NULL, PRIMARY KEY (owner, stream, cursor_value), CONSTRAINT aip_event_log_cursor CHECK (cursor_value > 0)) ENGINE=InnoDB"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_event_keys` (\
               owner VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, stream VARCHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, \
               idempotency_key VARCHAR(256) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, cursor_value BIGINT NOT NULL, \
               payload_digest CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, PRIMARY KEY (owner, stream, idempotency_key), \
               CONSTRAINT aip_event_keys_cursor CHECK (cursor_value > 0)) ENGINE=InnoDB"
        ),
    ]
}

fn valid_owner(value: &str) -> bool {
    !value.is_empty() && value.len() <= 255 && !value.chars().any(char::is_control)
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
