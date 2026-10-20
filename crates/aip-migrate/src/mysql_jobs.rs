//! MySQL 8.4 durable job store (WBS 7.2.1.3): the PostgreSQL `aip_execution::jobs::Store` state machine over InnoDB.
//!
//! Same contract, same types: idempotent enqueue (a repeated key converges on one run and a different request under the key is a
//! conflict), claim with a lease and a fencing epoch, heartbeat, finish, bounded retry with backoff, cancel, payload expiry,
//! operator list and redrive. What changes is how InnoDB gets there:
//!
//! * `FOR UPDATE SKIP LOCKED` picks the candidate, then the same transaction updates and re-reads it (`RETURNING` does not exist).
//! * Every deadline is computed from the database clock (`UTC_TIMESTAMP(6)`), never the caller's, like `clock_timestamp()`.
//! * The unique key `(owner, operation, contract, deployment, idempotency key)` is too wide for an InnoDB index, so a SHA-256 of
//!   the tuple is stored and unique instead; a duplicate insert (error 1062) means "this key already has a run".
//! * A lease epoch only grows. A worker whose lease expired and was taken over finds its epoch stale and every later
//!   heartbeat, finish, retry or fail of its attempt matches no row.
use aip_execution::{Error, JobResult, Lease, NewRun, Redrive, Run, RunSummary};
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Row, TxOpts, Value as MyValue};
use serde_json::Value;
use sha2::{Digest, Sha256};

const LEASE_MS: i64 = 30_000;
const MAX_PAYLOAD_BYTES: usize = 1 << 20;
const MAX_TIMEOUT_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_ATTEMPTS: i32 = 10;
const STATES: [&str; 6] = ["QUEUED", "RUNNING", "CANCEL_REQUESTED", "SUCCEEDED", "FAILED", "CANCELLED"];
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

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn run_budget_ms(timeout_ms: i64, max_attempts: i32, initial_delay_ms: i64) -> i64 {
    let retry_budget = (1..max_attempts).map(|attempt| (1000_i64.saturating_mul(1_i64 << (attempt - 1).min(15))).min(30_000)).sum::<i64>();
    initial_delay_ms.saturating_add(timeout_ms.saturating_mul(i64::from(max_attempts))).saturating_add(retry_budget)
}

fn parse_id(id: &str) -> Result<String, Error> {
    uuid::Uuid::parse_str(id).map(|id| id.to_string()).map_err(|_| Error::InvalidRun)
}

/// `now + n milliseconds` on the database clock.
fn after(ms: &str) -> String {
    format!("DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ({ms}) * 1000 MICROSECOND)")
}

#[derive(Clone, Debug)]
pub struct MysqlJobStore {
    schema: String,
    mariadb: bool,
}

impl MysqlJobStore {
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

    /// The same store on MariaDB 11.4: no `CAST(.. AS JSON)` (its JSON is `LONGTEXT` with a validity check, so the text is bound
    /// as it is) and no `FOR UPDATE OF <table>`. Everything else the store relies on (`SKIP LOCKED`, `JSON_LENGTH`, microsecond
    /// `UTC_TIMESTAMP`) exists there, which the live tests show.
    #[must_use]
    pub fn for_mariadb(mut self) -> Self {
        self.mariadb = true;
        self
    }

    pub(crate) fn is_mariadb(&self) -> bool {
        self.mariadb
    }

    /// A bound JSON document.
    fn json_param(&self) -> &'static str {
        if self.mariadb { "?" } else { "CAST(? AS JSON)" }
    }

    fn table(&self) -> String {
        format!("`{}`.`aip_jobs`", self.schema)
    }

    pub async fn enqueue<Q: Queryable>(&self, conn: &mut Q, new: NewRun) -> Result<Run, Error> {
        if new.owner.is_empty()
            || new.actor <= 0
            || new.operation.is_empty()
            || new.contract.is_empty()
            || new.deployment.is_empty()
            || new.implementation.is_empty()
            || new.key.is_empty()
            || new.key.len() > 256
            || new.timeout_ms <= 0
            || new.timeout_ms > MAX_TIMEOUT_MS
            || new.max_attempts <= 0
            || new.max_attempts > MAX_ATTEMPTS
            || new.ttl_ms <= 0
            || new.delay_ms < 0
            || new.delay_ms > MAX_TIMEOUT_MS
        {
            return Err(Error::InvalidRun);
        }
        let input_text = serde_json::to_string(&new.input).map_err(|error| Error::Database(error.to_string()))?;
        if input_text.len() > MAX_PAYLOAD_BYTES {
            return Err(Error::PayloadTooLarge);
        }
        let input_digest = hex_digest(input_text.as_bytes());
        let total_ms = run_budget_ms(new.timeout_ms, new.max_attempts, new.delay_ms);
        let key_digest = hex_digest(format!("{}\n{}\n{}\n{}\n{}", new.owner, new.operation, new.contract, new.deployment, new.key).as_bytes());
        let table = self.table();
        let id = uuid::Uuid::new_v4().to_string();
        let insert = format!(
            "INSERT INTO {table} (id, owner, actor_id, grant_data, operation, contract_digest, deployment_digest, implementation_digest, input, input_digest, \
               idempotency_key, key_digest, state, timeout_ms, max_attempts, result_ttl_ms, initial_delay_ms, run_deadline, next_run_at, created_at, trace_id) \
             VALUES (?, ?, ?, {json}, ?, ?, ?, ?, {json}, ?, ?, ?, 'QUEUED', ?, ?, ?, ?, {}, {}, UTC_TIMESTAMP(6), ?)",
            after("?"),
            after("?"),
            json = self.json_param()
        );
        let params: Vec<MyValue> = vec![
            id.clone().into(),
            new.owner.clone().into(),
            new.actor.into(),
            new.grant.to_string().into(),
            new.operation.clone().into(),
            new.contract.clone().into(),
            new.deployment.clone().into(),
            new.implementation.clone().into(),
            input_text.into(),
            input_digest.clone().into(),
            new.key.clone().into(),
            key_digest.clone().into(),
            new.timeout_ms.into(),
            new.max_attempts.into(),
            new.ttl_ms.into(),
            new.delay_ms.into(),
            total_ms.into(),
            new.delay_ms.into(),
            new.trace_id.clone().map_or(MyValue::NULL, Into::into),
        ];
        let run_id = match conn.exec_drop(insert, params).await {
            Ok(()) => id,
            // The key already has a run: converge on it if the request is the same one.
            Err(mysql_async::Error::Server(server)) if server.code == 1062 => {
                let row: Option<Row> = conn
                    .exec_first(
                        format!("SELECT id, input_digest, implementation_digest, actor_id, timeout_ms, max_attempts, result_ttl_ms, initial_delay_ms FROM {table} WHERE key_digest = ?"),
                        (key_digest,),
                    )
                    .await
                    .map_err(db)?;
                let row = row.ok_or_else(|| Error::Database("duplicate job key without a run".into()))?;
                // max_attempts is server retry policy, not part of the request: an operator changing it must not turn a provider's
                // redelivery into a conflict, so the stored run keeps its own.
                let same = text(row.as_ref(1)).as_deref() == Some(input_digest.as_str())
                    && text(row.as_ref(2)).as_deref() == Some(new.implementation.as_str())
                    && int(row.as_ref(3)) == Some(new.actor)
                    && int(row.as_ref(4)) == Some(new.timeout_ms)
                    && int(row.as_ref(6)) == Some(new.ttl_ms)
                    && int(row.as_ref(7)) == Some(new.delay_ms);
                if !same {
                    return Err(Error::IdempotencyConflict);
                }
                text(row.as_ref(0)).ok_or_else(|| Error::Database("job id missing".into()))?
            }
            Err(error) => return Err(db(error)),
        };
        self.status(conn, &run_id, &new.owner).await?.ok_or_else(|| Error::Database("inserted job disappeared".into()))
    }

    /// Take the next runnable job under a lease, or `None`. Runs in its own READ COMMITTED transaction: the candidate is picked
    /// with `FOR UPDATE SKIP LOCKED`, so concurrent workers never pick the same run, and the update and the re-read commit together.
    pub async fn claim(&self, conn: &mut Conn, worker: &str) -> Result<Option<Lease>, Error> {
        if worker.is_empty() || worker.len() > 256 {
            return Err(Error::InvalidRun);
        }
        let table = self.table();
        let mut options = TxOpts::default();
        options.with_isolation_level(mysql_async::IsolationLevel::ReadCommitted);
        let mut tx = conn.start_transaction(options).await.map_err(db)?;
        // Runs that can no longer be retried end here, whichever worker notices first.
        tx.query_drop(format!(
            "UPDATE {table} SET error = CASE WHEN state = 'CANCEL_REQUESTED' THEN 'WIRE.CANCELLATION_UNCONFIRMED' \
               WHEN run_deadline <= UTC_TIMESTAMP(6) THEN 'deadline_exceeded' ELSE 'attempts_exhausted' END, \
               lease_owner = NULL, lease_until = NULL, result_expires_at = {}, state = 'FAILED' \
             WHERE (state IN ('QUEUED', 'RUNNING') AND run_deadline <= UTC_TIMESTAMP(6)) \
                OR (state = 'RUNNING' AND lease_until <= UTC_TIMESTAMP(6) AND attempt >= max_attempts) \
                OR (state = 'CANCEL_REQUESTED' AND lease_until <= UTC_TIMESTAMP(6))",
            after("result_ttl_ms")
        ))
        .await
        .map_err(db)?;
        let candidate: Option<String> = tx
            .query_first(format!(
                "SELECT id FROM {table} WHERE run_deadline > UTC_TIMESTAMP(6) AND attempt < max_attempts \
                   AND ((state = 'QUEUED' AND next_run_at <= UTC_TIMESTAMP(6)) OR (state = 'RUNNING' AND lease_until <= UTC_TIMESTAMP(6))) \
                 ORDER BY next_run_at, created_at, id LIMIT 1 FOR UPDATE SKIP LOCKED"
            ))
            .await
            .map_err(db)?;
        let Some(id) = candidate else {
            tx.commit().await.map_err(db)?;
            return Ok(None);
        };
        tx.exec_drop(
            format!(
                "UPDATE {table} SET state = 'RUNNING', attempt = attempt + 1, lease_epoch = lease_epoch + 1, lease_owner = ?, \
                   attempt_deadline = LEAST(run_deadline, {timeout}), \
                   lease_until = LEAST(run_deadline, {lease}, {timeout}) \
                 WHERE id = ?",
                timeout = after("timeout_ms"),
                lease = after(&LEASE_MS.to_string())
            ),
            (worker, id.as_str()),
        )
        .await
        .map_err(db)?;
        let row: Option<Row> = tx
            .exec_first(
                format!(
                    "SELECT id, lease_epoch, lease_owner, owner, actor_id, CAST(grant_data AS CHAR), operation, contract_digest, deployment_digest, \
                       implementation_digest, CAST(input AS CHAR), attempt, DATE_FORMAT(run_deadline, {ISO}), trace_id FROM {table} WHERE id = ?"
                ),
                (id.as_str(),),
            )
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)?;
        row.map(|row| lease_from(&row)).transpose()
    }

    /// Rows that still belong to this lease: same epoch and worker, `RUNNING`, and no deadline has passed.
    fn live(&self) -> &'static str {
        "id = ? AND lease_epoch = ? AND lease_owner = ? AND owner = ? AND state = 'RUNNING' AND lease_until > UTC_TIMESTAMP(6) \
         AND attempt_deadline > UTC_TIMESTAMP(6) AND run_deadline > UTC_TIMESTAMP(6)"
    }

    fn live_params(lease: &Lease) -> Vec<MyValue> {
        vec![lease.run_id.to_string().into(), lease.epoch.into(), lease.worker.clone().into(), lease.owner.clone().into()]
    }

    async fn changed<Q: Queryable>(&self, conn: &mut Q, sql: String, params: Vec<MyValue>) -> Result<bool, Error> {
        conn.exec_drop(sql, params).await.map_err(db)?;
        let changed: Option<u64> = conn.query_first("SELECT ROW_COUNT()").await.map_err(db)?;
        Ok(changed == Some(1))
    }

    pub async fn heartbeat<Q: Queryable>(&self, conn: &mut Q, lease: &Lease) -> Result<bool, Error> {
        let sql =
            format!("UPDATE {} SET lease_until = LEAST(attempt_deadline, {}) WHERE {}", self.table(), after(&LEASE_MS.to_string()), self.live());
        self.changed(conn, sql, Self::live_params(lease)).await
    }

    pub async fn finish<Q: Queryable>(&self, conn: &mut Q, lease: &Lease, output: Value) -> Result<bool, Error> {
        let output = serde_json::to_string(&output).map_err(|error| Error::Database(error.to_string()))?;
        if output.len() > MAX_PAYLOAD_BYTES {
            return Err(Error::PayloadTooLarge);
        }
        let sql = format!(
            "UPDATE {} SET state = 'SUCCEEDED', output = {}, error = NULL, lease_owner = NULL, lease_until = NULL, result_expires_at = {} WHERE {}",
            self.table(),
            self.json_param(),
            after("result_ttl_ms"),
            self.live()
        );
        let mut params = vec![output.into()];
        params.extend(Self::live_params(lease));
        self.changed(conn, sql, params).await
    }

    pub async fn retry<Q: Queryable>(&self, conn: &mut Q, lease: &Lease, error: &str) -> Result<bool, Error> {
        self.retry_after(conn, lease, error, None).await
    }

    /// Like `retry`, with the wait before the next attempt chosen by the extension: replaces the default exponential backoff, is
    /// raised to one second and cut to one hour, and extends the run's deadline by what it adds over the default. The attempt limit
    /// still ends the run.
    pub async fn retry_after<Q: Queryable>(&self, conn: &mut Q, lease: &Lease, error: &str, after_ms: Option<i64>) -> Result<bool, Error> {
        let default_wait = "LEAST(30000, 1000 * POW(2, LEAST(attempt - 1, 15)))";
        let wait = format!("(CASE WHEN ? IS NULL THEN {default_wait} ELSE LEAST(3600000, GREATEST(1000, ?)) END)");
        let exhausted = "(attempt >= max_attempts OR run_deadline <= UTC_TIMESTAMP(6))";
        // The assignments read the row as it was: MySQL evaluates SET left to right and a later expression sees an earlier
        // assignment, so `state` is assigned last and everything else is decided from `exhausted` computed up front.
        let sql = format!(
            "UPDATE {} SET \
               next_run_at = CASE WHEN {exhausted} THEN next_run_at ELSE {} END, \
               run_deadline = CASE WHEN {exhausted} OR ? IS NULL THEN run_deadline \
                 ELSE DATE_ADD(run_deadline, INTERVAL GREATEST(0, {wait} - {default_wait}) * 1000 MICROSECOND) END, \
               error = ?, lease_owner = NULL, lease_until = NULL, \
               result_expires_at = CASE WHEN {exhausted} THEN {} ELSE NULL END, \
               state = CASE WHEN {exhausted} THEN 'FAILED' ELSE 'QUEUED' END \
             WHERE {}",
            self.table(),
            after(&wait),
            after("result_ttl_ms"),
            self.live()
        );
        let wait_param = || after_ms.map_or(MyValue::NULL, Into::into);
        // Parameter order follows the placeholders above: next_run_at's wait (2), run_deadline's IS NULL (1), its wait (2), error, live.
        let mut params: Vec<MyValue> = vec![wait_param(), wait_param(), wait_param(), wait_param(), wait_param(), error.into()];
        params.extend(Self::live_params(lease));
        self.changed(conn, sql, params).await
    }

    pub async fn fail<Q: Queryable>(&self, conn: &mut Q, lease: &Lease, code: &str) -> Result<bool, Error> {
        let sql = format!(
            "UPDATE {} SET state = 'FAILED', error = ?, lease_owner = NULL, lease_until = NULL, result_expires_at = {} WHERE {}",
            self.table(),
            after("result_ttl_ms"),
            self.live()
        );
        let mut params = vec![code.into()];
        params.extend(Self::live_params(lease));
        self.changed(conn, sql, params).await
    }

    pub async fn check_lease<Q: Queryable>(&self, conn: &mut Q, lease: &Lease) -> Result<bool, Error> {
        let sql = format!("SELECT COUNT(*) FROM {} WHERE {}", self.table(), self.live());
        let count: Option<i64> = conn.exec_first(sql, Self::live_params(lease)).await.map_err(db)?;
        Ok(count == Some(1))
    }

    const CANCEL_LIVE: &'static str = "id = ? AND lease_epoch = ? AND lease_owner = ? AND owner = ? AND state = 'CANCEL_REQUESTED' \
         AND lease_until > UTC_TIMESTAMP(6) AND attempt_deadline > UTC_TIMESTAMP(6)";

    pub async fn acknowledge_cancel<Q: Queryable>(&self, conn: &mut Q, lease: &Lease) -> Result<bool, Error> {
        let sql = format!(
            "UPDATE {} SET state = 'CANCELLED', lease_owner = NULL, lease_until = NULL, result_expires_at = {} WHERE {}",
            self.table(),
            after("result_ttl_ms"),
            Self::CANCEL_LIVE
        );
        self.changed(conn, sql, Self::live_params(lease)).await
    }

    pub async fn reject_cancel<Q: Queryable>(&self, conn: &mut Q, lease: &Lease, error: &str) -> Result<bool, Error> {
        let sql = format!(
            "UPDATE {} SET state = 'FAILED', error = ?, lease_owner = NULL, lease_until = NULL, result_expires_at = {} WHERE {}",
            self.table(),
            after("result_ttl_ms"),
            Self::CANCEL_LIVE
        );
        let mut params = vec![error.into()];
        params.extend(Self::live_params(lease));
        self.changed(conn, sql, params).await
    }

    pub async fn cancel<Q: Queryable>(&self, conn: &mut Q, id: &str, owner: &str) -> Result<Option<Run>, Error> {
        let id = parse_id(id)?;
        // Assignments run left to right and see earlier ones, so the columns that read `state` come before `state` itself.
        let sql = format!(
            "UPDATE {} SET cancel_requested_at = CASE WHEN state IN ('QUEUED', 'RUNNING') THEN UTC_TIMESTAMP(6) ELSE cancel_requested_at END, \
               result_expires_at = CASE WHEN state = 'QUEUED' THEN {} ELSE result_expires_at END, \
               state = CASE WHEN state = 'QUEUED' THEN 'CANCELLED' WHEN state = 'RUNNING' THEN 'CANCEL_REQUESTED' ELSE state END \
             WHERE id = ? AND owner = ?",
            self.table(),
            after("result_ttl_ms")
        );
        conn.exec_drop(sql, (id.as_str(), owner)).await.map_err(db)?;
        self.status(conn, &id, owner).await
    }

    pub async fn status<Q: Queryable>(&self, conn: &mut Q, id: &str, owner: &str) -> Result<Option<Run>, Error> {
        let id = parse_id(id)?;
        let row: Option<Row> = conn
            .exec_first(
                format!(
                    "SELECT id, operation, state, attempt, CASE WHEN result_expires_at > UTC_TIMESTAMP(6) THEN CAST(output AS CHAR) ELSE NULL END, error \
                     FROM {} WHERE id = ? AND owner = ?",
                    self.table()
                ),
                (id.as_str(), owner),
            )
            .await
            .map_err(db)?;
        Ok(row.map(|row| Run {
            id: text(row.as_ref(0)).unwrap_or_default(),
            operation: text(row.as_ref(1)).unwrap_or_default(),
            state: text(row.as_ref(2)).unwrap_or_default(),
            attempt: int(row.as_ref(3)).unwrap_or(0) as i32,
            output: text(row.as_ref(4)).and_then(|text| serde_json::from_str(&text).ok()),
            error: text(row.as_ref(5)),
        }))
    }

    pub async fn result<Q: Queryable>(&self, conn: &mut Q, id: &str, owner: &str) -> Result<Option<JobResult>, Error> {
        let id = parse_id(id)?;
        let row: Option<Row> = conn
            .exec_first(
                format!(
                    "SELECT id, operation, CAST(output AS CHAR) FROM {} WHERE id = ? AND owner = ? AND state = 'SUCCEEDED' AND output IS NOT NULL AND result_expires_at > UTC_TIMESTAMP(6)",
                    self.table()
                ),
                (id.as_str(), owner),
            )
            .await
            .map_err(db)?;
        Ok(row.map(|row| JobResult {
            id: text(row.as_ref(0)).unwrap_or_default(),
            operation: text(row.as_ref(1)).unwrap_or_default(),
            output: json(row.as_ref(2)),
        }))
    }

    /// Drop the payloads of finished runs whose result window ended; the row (and its key) stays so a late redelivery still converges.
    pub async fn expire_payloads(&self, conn: &mut Conn) -> Result<Vec<String>, Error> {
        let table = self.table();
        let mut tx = conn.start_transaction(TxOpts::default()).await.map_err(db)?;
        let ids: Vec<String> = tx
            .query(format!(
                "SELECT id FROM {table} WHERE state IN ('SUCCEEDED', 'FAILED', 'CANCELLED') AND result_expires_at <= UTC_TIMESTAMP(6) \
                   AND (JSON_LENGTH(input) > 0 OR JSON_LENGTH(grant_data) > 0 OR output IS NOT NULL) \
                 ORDER BY result_expires_at, id LIMIT 1000 FOR UPDATE SKIP LOCKED"
            ))
            .await
            .map_err(db)?;
        for id in &ids {
            tx.exec_drop(format!("DELETE FROM `{}`.`aip_effects` WHERE run_id = ?", self.schema), (id.as_str(),)).await.map_err(db)?;
            tx.exec_drop(format!("UPDATE {table} SET input = JSON_OBJECT(), grant_data = JSON_OBJECT(), output = NULL WHERE id = ?"), (id.as_str(),))
                .await
                .map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        Ok(ids)
    }

    /// Operator view across every owner, newest first. It never returns input, output or grant payloads.
    pub async fn list(&self, conn: &mut Conn, state: Option<&str>, operation: Option<&str>, limit: i64) -> Result<Vec<RunSummary>, Error> {
        if !(1..=500).contains(&limit) || state.is_some_and(|state| !STATES.contains(&state)) {
            return Err(Error::InvalidRun);
        }
        let rows: Vec<Row> = conn
            .exec(
                format!(
                    "SELECT id, owner, operation, idempotency_key, state, attempt, max_attempts, error, DATE_FORMAT(created_at, {ISO}), \
                       DATE_FORMAT(result_expires_at, {ISO}), (state = 'FAILED' AND result_expires_at > UTC_TIMESTAMP(6) AND JSON_LENGTH(grant_data) > 0) \
                     FROM {} WHERE (? IS NULL OR state = ?) AND (? IS NULL OR operation = ?) ORDER BY created_at DESC, id LIMIT {limit}",
                    self.table()
                ),
                (state, state, operation, operation),
            )
            .await
            .map_err(db)?;
        Ok(rows
            .iter()
            .map(|row| RunSummary {
                id: text(row.as_ref(0)).unwrap_or_default(),
                owner: text(row.as_ref(1)).unwrap_or_default(),
                operation: text(row.as_ref(2)).unwrap_or_default(),
                key: text(row.as_ref(3)).unwrap_or_default(),
                state: text(row.as_ref(4)).unwrap_or_default(),
                attempt: int(row.as_ref(5)).unwrap_or(0) as i32,
                max_attempts: int(row.as_ref(6)).unwrap_or(0) as i32,
                error: text(row.as_ref(7)),
                created_at: text(row.as_ref(8)).unwrap_or_default(),
                result_expires_at: text(row.as_ref(9)),
                redrivable: int(row.as_ref(10)) == Some(1),
            })
            .collect())
    }

    /// Put a FAILED run back in the queue as the same run: its id, key, owner and effect journal stay, so recorded effects replay,
    /// and `lease_epoch` keeps growing so a stale worker of an earlier attempt stays fenced.
    pub async fn redrive(&self, conn: &mut Conn, id: &str, deployment: &str) -> Result<Redrive, Error> {
        let id = parse_id(id)?;
        let table = self.table();
        let mut tx = conn.start_transaction(TxOpts::default()).await.map_err(db)?;
        let row: Option<Row> = tx
            .exec_first(
                format!(
                    "SELECT state, deployment_digest, (result_expires_at > UTC_TIMESTAMP(6) AND JSON_LENGTH(grant_data) > 0), timeout_ms, max_attempts \
                     FROM {table} WHERE id = ? FOR UPDATE"
                ),
                (id.as_str(),),
            )
            .await
            .map_err(db)?;
        let Some(row) = row else { return Ok(Redrive::NotFound) };
        if text(row.as_ref(0)).as_deref() != Some("FAILED") {
            return Ok(Redrive::NotFailed);
        }
        if int(row.as_ref(2)) != Some(1) {
            return Ok(Redrive::Expired);
        }
        if text(row.as_ref(1)).as_deref() != Some(deployment) {
            return Ok(Redrive::DeploymentChanged);
        }
        let budget = run_budget_ms(int(row.as_ref(3)).unwrap_or(0), int(row.as_ref(4)).unwrap_or(0) as i32, 0);
        tx.exec_drop(
            format!(
                "UPDATE {table} SET state = 'QUEUED', attempt = 0, error = NULL, output = NULL, lease_owner = NULL, lease_until = NULL, \
                   attempt_deadline = NULL, cancel_requested_at = NULL, result_expires_at = NULL, next_run_at = UTC_TIMESTAMP(6), run_deadline = {} \
                 WHERE id = ? AND state = 'FAILED'",
                after("?")
            ),
            (budget, id.as_str()),
        )
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(Redrive::Queued)
    }
}

fn lease_from(row: &Row) -> Result<Lease, Error> {
    let bad = |what: &str| Error::Database(format!("job lease row: {what}"));
    Ok(Lease {
        run_id: uuid::Uuid::parse_str(&text(row.as_ref(0)).ok_or_else(|| bad("id"))?).map_err(|_| bad("id"))?,
        epoch: int(row.as_ref(1)).ok_or_else(|| bad("epoch"))?,
        worker: text(row.as_ref(2)).ok_or_else(|| bad("worker"))?,
        owner: text(row.as_ref(3)).ok_or_else(|| bad("owner"))?,
        actor: int(row.as_ref(4)).ok_or_else(|| bad("actor"))?,
        grant: json(row.as_ref(5)),
        operation: text(row.as_ref(6)).ok_or_else(|| bad("operation"))?,
        contract: text(row.as_ref(7)).ok_or_else(|| bad("contract"))?,
        deployment: text(row.as_ref(8)).ok_or_else(|| bad("deployment"))?,
        implementation: text(row.as_ref(9)).ok_or_else(|| bad("implementation"))?,
        input: json(row.as_ref(10)),
        attempt: int(row.as_ref(11)).ok_or_else(|| bad("attempt"))? as i32,
        run_deadline: text(row.as_ref(12)).ok_or_else(|| bad("deadline"))?,
        trace_id: text(row.as_ref(13)),
    })
}
