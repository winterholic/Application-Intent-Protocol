//! Outbox relay over MySQL (WBS 7.2.1.3): outbox rows written by `/apply` become durable jobs, once.
//!
//! The outbox row, the job and the relay's receipt live in the same database, so one transaction does what the PostgreSQL relay
//! has to approximate across two stores: claim the pending rows with `FOR UPDATE SKIP LOCKED`, enqueue each job under a key derived
//! from the row, write the receipt, commit. A crash anywhere leaves either all of it or none of it, concurrent relays never take the
//! same row, and the receipt's primary key refuses a second dispatch of a row even if a relay is misconfigured.
use crate::mysql_jobs::MysqlJobStore;
use aip_execution::{Error, NewRun};
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, IsolationLevel, Row, TxOpts, Value as MyValue};

/// One outbox row as the relay sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutboxEvent {
    pub id: i64,
    pub topic: String,
    pub source: String,
    pub source_id: Option<String>,
    pub recipient_id: Option<String>,
    /// `YYYY-MM-DDTHH:MM:SS.ffffffZ`
    pub created_at: String,
    /// Microseconds since the Unix epoch, for deterministic job keys.
    pub created_us: i64,
}

impl OutboxEvent {
    /// The idempotency key of the job this row becomes. Row id and creation time together: an id reused after a restore of the table
    /// would still not collide with an earlier dispatch.
    pub fn job_key(&self) -> String {
        format!("outbox:{}:{}", self.id, self.created_us)
    }
}

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

#[derive(Clone, Debug)]
pub struct MysqlOutboxRelay {
    schema: String,
    jobs: MysqlJobStore,
}

impl MysqlOutboxRelay {
    pub fn new(schema: impl Into<String>) -> Result<Self, Error> {
        let schema = schema.into();
        let jobs = MysqlJobStore::new(schema.clone())?;
        Ok(Self { schema, jobs })
    }

    /// Relay on MariaDB 11.4 (see [`MysqlJobStore::for_mariadb`]).
    #[must_use]
    pub fn for_mariadb(mut self) -> Self {
        self.jobs = self.jobs.for_mariadb();
        self
    }

    /// Dispatch up to `limit` pending rows of `topic`. `make_run` builds the job for a row; if it fails for any row the whole batch
    /// is rolled back (nothing dispatched, every row stays pending) and its error is returned. Returns the number dispatched.
    pub async fn relay_once<F, E>(&self, conn: &mut Conn, topic: &str, limit: u32, make_run: F) -> Result<usize, RelayError<E>>
    where
        F: Fn(&OutboxEvent) -> Result<NewRun, E>,
    {
        let (outbox, dispatch) = (format!("`{}`.`aip_outbox`", self.schema), format!("`{}`.`aip_outbox_dispatch`", self.schema));
        // MariaDB has no `OF <table>`: there the lock covers the one table the statement reads rows from for update.
        let locking = if self.jobs.is_mariadb() { "FOR UPDATE SKIP LOCKED" } else { "FOR UPDATE OF o SKIP LOCKED" };
        let mut options = TxOpts::default();
        options.with_isolation_level(IsolationLevel::ReadCommitted);
        let mut tx = conn.start_transaction(options).await.map_err(|e| RelayError::Store(db(e)))?;
        let rows: Vec<Row> = tx
            .exec(
                format!(
                    "SELECT o.id, o.source, o.source_id, o.recipient_id, DATE_FORMAT(o.created_at, '%Y-%m-%dT%H:%i:%s.%fZ'), \
                       CAST(TIMESTAMPDIFF(MICROSECOND, '1970-01-01 00:00:00', o.created_at) AS SIGNED) \
                     FROM {outbox} o WHERE o.topic = ? AND NOT EXISTS (SELECT 1 FROM {dispatch} d WHERE d.outbox_id = o.id) \
                     ORDER BY o.id LIMIT {limit} {locking}"
                ),
                (topic,),
            )
            .await
            .map_err(|e| RelayError::Store(db(e)))?;
        let mut dispatched = 0;
        for row in &rows {
            let event = OutboxEvent {
                id: text(row.as_ref(0)).and_then(|id| id.parse().ok()).ok_or_else(|| RelayError::Store(Error::Database("outbox id".into())))?,
                topic: topic.to_string(),
                source: text(row.as_ref(1)).unwrap_or_default(),
                source_id: text(row.as_ref(2)),
                recipient_id: text(row.as_ref(3)),
                created_at: text(row.as_ref(4)).unwrap_or_default(),
                created_us: text(row.as_ref(5)).and_then(|us| us.parse().ok()).unwrap_or(0),
            };
            let new = make_run(&event).map_err(RelayError::Rejected)?;
            let run = self.jobs.enqueue(&mut tx, new).await.map_err(RelayError::Store)?;
            // Another relay can commit a row's receipt after this statement's snapshot and before its lock, so the row comes back
            // here although it was dispatched. The deterministic job key already converged on that relay's run, so the receipt
            // is a no-op and the row does not count as dispatched by this batch.
            tx.exec_drop(
                format!("INSERT IGNORE INTO {dispatch} (outbox_id, outbox_created_at, job_id) VALUES (?, ?, ?)"),
                (event.id, event.created_at.replace('T', " ").trim_end_matches('Z'), run.id.as_str()),
            )
            .await
            .map_err(|e| RelayError::Store(db(e)))?;
            dispatched += usize::try_from(tx.affected_rows()).unwrap_or(0);
        }
        tx.commit().await.map_err(|e| RelayError::Store(db(e)))?;
        Ok(dispatched)
    }
}

/// Why a relay batch did not complete. Either way the batch left nothing behind.
#[derive(Debug, PartialEq)]
pub enum RelayError<E> {
    /// The caller's `make_run` refused a row.
    Rejected(E),
    /// The database failed.
    Store(Error),
}
