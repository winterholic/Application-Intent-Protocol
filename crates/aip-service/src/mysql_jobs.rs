//! Durable MySQL execution behind the same catalog fence and principal binding as synchronous requests.
use super::*;
use aip_migrate::mysql_backend::MysqlBackend;
use aip_migrate::mysql_driver::{CommitFailure, FencedMysqlTransaction, MysqlDeploymentFence, MysqlDriver, mysql_async};
use aip_migrate::mysql_jobs::MysqlJobStore;
use mysql_async::{Transaction, Value as SqlValue, prelude::Queryable};
use spike_v2_read::backend::{BackendFuture, RelationalBackend};
use spike_v2_read::plan::{Caller, Reject};
use spike_v6_transport::auth::DurablePrincipal;
use spike_v6_transport::relational_jobs::RelationalJobs;
use spike_v6_transport::server::ReadExtensions;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy, Default)]
struct Messaging {
    enabled: bool,
    client_publish: bool,
    client_consume: bool,
}

pub(crate) struct MysqlJobs {
    driver: MysqlDriver,
    fence: MysqlDeploymentFence,
    schema: String,
    deployment: String,
    facts: Arc<Value>,
    wire: IdWire,
    backend: Arc<MysqlBackend>,
    workers: ReadExtensions,
    /// External HTTP integrations and outbound webhooks a job may call through `http.<name>`.
    broker: Option<Arc<aip_execution::integrations::Broker>>,
    /// Message queues are on, and whether clients may publish or consume through `/messages/*` themselves.
    messaging: Messaging,
    launch: spike_v4_worker::WorkerLaunchConfig,
    policies: std::collections::HashMap<String, spike_v6_transport::triggers::JobPolicy>,
    triggers: Arc<spike_v6_transport::triggers::Triggers>,
    concurrency: usize,
}

/// What a server trigger adds to a job submission.
struct Trigger {
    /// The outbox row this job delivers; its dispatch receipt commits with the job.
    outbox_id: Option<i64>,
}

/// id, source, source id, recipient id, created-at text, created-at microseconds since the epoch.
type OutboxRow = (i64, String, Option<String>, Option<String>, String, i64);

const SCHEDULE_TICK: Duration = Duration::from_secs(5);
const OUTBOX_TICK: Duration = Duration::from_secs(1);

fn db<T>(_: T) -> &'static str {
    "WIRE.DB_UNAVAILABLE"
}
fn store_error(error: aip_execution::Error) -> &'static str {
    match error {
        aip_execution::Error::InvalidRun => "WIRE.BAD_VALUE",
        aip_execution::Error::IdempotencyConflict => "WIRE.IDEMPOTENCY_MISMATCH",
        aip_execution::Error::PayloadTooLarge => "WIRE.PAYLOAD_TOO_LARGE",
        _ => "WIRE.DB_UNAVAILABLE",
    }
}
fn fence_error(error: aip_migrate::mysql_catalog::CatalogError) -> &'static str {
    if error.code == "MIGRATE.DRIFT_DETECTED" { "WIRE.DEPLOYMENT_CHANGED" } else { "WIRE.DB_UNAVAILABLE" }
}
fn owner(grant: &DurablePrincipal) -> String {
    spike_v1_fixture::digest(&json!([grant.issuer, grant.subject, grant.actor_id, 0]))
}
fn message_error(error: aip_execution::messages::Error) -> &'static str {
    use aip_execution::messages::Error;
    match error {
        Error::IdempotencyConflict => "WIRE.IDEMPOTENCY_MISMATCH",
        Error::Database(_) => "WIRE.DB_UNAVAILABLE",
        Error::PayloadTooLarge => "WIRE.PAYLOAD_TOO_LARGE",
        _ => "WIRE.BAD_VALUE",
    }
}
fn number(body: &Value, key: &str, default: i64, min: i64, max: i64) -> Result<i64> {
    body.get(key).map_or(Ok(default), |value| value.as_i64().filter(|n| (min..=max).contains(n)).ok_or("WIRE.BAD_VALUE"))
}
/// A body whose keys are exactly the required ones plus any of the optional ones.
fn shape(body: &Value, required: &[&str], optional: &[&str]) -> Result<()> {
    if !body.as_object().is_some_and(|object| {
        required.iter().all(|key| object.contains_key(*key))
            && object.keys().all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
    }) {
        return Err("WIRE.BAD_REQUEST");
    }
    Ok(())
}
fn exact(body: &Value, fields: &[&str]) -> bool {
    body.as_object().is_some_and(|body| body.len() == fields.len() && fields.iter().all(|key| body.contains_key(*key)))
}
fn text<'a>(body: &'a Value, key: &str) -> Result<&'a str> {
    body[key].as_str().ok_or("WIRE.BAD_REQUEST")
}
fn valid_key(key: &str) -> bool {
    spike_v6_transport::relational_jobs::valid_key(key)
}

impl MysqlJobs {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        driver: MysqlDriver,
        fence: MysqlDeploymentFence,
        schema: String,
        deployment: String,
        facts: Arc<Value>,
        wire: IdWire,
        backend: Arc<MysqlBackend>,
        workers: ReadExtensions,
        broker: Option<Arc<aip_execution::integrations::Broker>>,
        infrastructure: &spike_v6_transport::infrastructure::Infrastructure,
    ) -> std::result::Result<Self, Value> {
        let workers = workers.validate_with_writes(&facts, wire).map_err(|_| error("JOB_CONFIG", "job worker 구현 파일을 확인해야 함"))?;
        // Storage-backed capabilities are opened only as their MySQL adapters are connected. Refuse the contract up front.
        for (_, op) in facts["operations"].as_object().into_iter().flatten().filter(|(_, op)| op["kind"] == "job") {
            for (capability, _) in op["access"].as_object().into_iter().flatten() {
                if let Some(name) = capability.strip_prefix("http.") {
                    if !broker.as_ref().is_some_and(|broker| broker.contains(name)) {
                        return Err(error(
                            "JOB_CONFIG",
                            &format!("job이 선언한 HTTP 연동 `{name}`이 workers.integrations 또는 outbound_webhooks에 없음"),
                        ));
                    }
                } else if !["read.", "apply.", "jobs.", "events.", "messages."].iter().any(|prefix| capability.starts_with(prefix)) {
                    return Err(error("DB_ADAPTER_UNSUPPORTED", &format!("MySQL job capability 미지원: {capability}")));
                }
            }
        }
        Ok(Self {
            driver,
            fence,
            schema,
            deployment,
            facts,
            wire,
            backend,
            workers,
            broker,
            messaging: Messaging {
                enabled: infrastructure.messaging,
                client_publish: infrastructure.client_publish,
                client_consume: infrastructure.client_consume,
            },
            launch: infrastructure.worker_launch.clone(),
            policies: infrastructure.job_policies.clone(),
            triggers: infrastructure.triggers.clone(),
            concurrency: infrastructure.resource_limits.jobs,
        })
    }

    pub(crate) async fn check_storage(&self) -> Result<()> {
        let mut conn = self.driver.pool().get_conn().await.map_err(db)?;
        let engine: Option<String> = conn
            .exec_first("SELECT ENGINE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA=? AND TABLE_NAME='aip_effects'", (&self.schema,))
            .await
            .map_err(db)?;
        let key: Option<Option<String>> = conn
            .exec_first(
                "SELECT GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX SEPARATOR ',') FROM INFORMATION_SCHEMA.STATISTICS \
             WHERE TABLE_SCHEMA=? AND TABLE_NAME='aip_effects' AND INDEX_NAME='PRIMARY'",
                (&self.schema,),
            )
            .await
            .map_err(db)?;
        if engine.as_deref() != Some("InnoDB") || key.flatten().as_deref() != Some("run_id,effect_key") {
            return Err("WIRE.NOT_AVAILABLE");
        }
        let _: Vec<mysql_async::Row> = conn
            .query(format!("SELECT run_id,effect_key,request_digest,state,result FROM {} WHERE 1=0", self.table("aip_effects")))
            .await
            .map_err(db)?;
        if self.declares("events.") {
            for (table, columns) in [
                ("aip_event_streams", "owner,stream,head,retained_from"),
                ("aip_event_log", "owner,stream,cursor_value,idempotency_key,payload,created_at"),
                ("aip_event_keys", "owner,stream,idempotency_key,cursor_value,payload_digest"),
            ] {
                let engine: Option<String> = conn
                    .exec_first("SELECT ENGINE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA=? AND TABLE_NAME=?", (&self.schema, table))
                    .await
                    .map_err(db)?;
                if engine.as_deref() != Some("InnoDB") {
                    return Err("WIRE.NOT_AVAILABLE");
                }
                let _: Vec<mysql_async::Row> =
                    conn.query(format!("SELECT {columns} FROM {} WHERE 1=0", self.table(table))).await.map_err(|_| "WIRE.NOT_AVAILABLE")?;
            }
        }
        if self.messaging.enabled || self.declares("messages.") {
            let engine: Option<String> = conn
                .exec_first("SELECT ENGINE FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA=? AND TABLE_NAME='aip_message_queue'", (&self.schema,))
                .await
                .map_err(db)?;
            if engine.as_deref() != Some("InnoDB") {
                return Err("WIRE.NOT_AVAILABLE");
            }
            let _: Vec<mysql_async::Row> = conn
                .query(format!(
                    "SELECT id,owner,queue,idempotency_key,payload,payload_digest,state,deliveries,max_deliveries,lease_epoch,lease_owner,lease_token,\
                     lease_until,available_at,expires_at,last_error,acknowledged_at,created_at FROM {} WHERE 1=0",
                    self.table("aip_message_queue")
                ))
                .await
                .map_err(|_| "WIRE.NOT_AVAILABLE")?;
        }
        Ok(())
    }

    /// True when a job of the contract declares a capability with this prefix, so its infrastructure must exist.
    fn declares(&self, prefix: &str) -> bool {
        self.facts["operations"].as_object().into_iter().flatten().any(|(_, op)| {
            op["kind"] == "job" && op["access"].as_object().is_some_and(|access| access.keys().any(|capability| capability.starts_with(prefix)))
        })
    }

    /// True when a job declares exactly this capability.
    fn exposes(&self, capability: &str) -> bool {
        self.facts["operations"].as_object().into_iter().flatten().any(|(_, op)| op["kind"] == "job" && op["access"][capability] == true)
    }

    fn messages(&self) -> Result<aip_migrate::mysql_messages::MysqlMessageStore> {
        aip_migrate::mysql_messages::MysqlMessageStore::new(&self.schema).map_err(|_| "WIRE.DB_UNAVAILABLE")
    }

    /// Publishes inside the caller's fenced transaction. The per-owner quota is checked under a bucket lock so two publishers
    /// cannot both pass it; a replay of a key that already exists skips the quota, like on PostgreSQL.
    #[allow(clippy::too_many_arguments)]
    async fn publish_message(
        &self,
        tx: &mut Transaction<'static>,
        owner: &str,
        queue: &str,
        key: &str,
        payload: Value,
        delay_ms: i64,
        ttl_ms: i64,
        max_deliveries: i32,
    ) -> Result<Value> {
        let digest = spike_v1_fixture::digest(&json!(["aip-messages", self.schema, owner]));
        let bucket = u64::from_str_radix(&digest[..8], 16).map_err(db)? % 1000;
        let locked: Option<u64> =
            tx.exec_first(format!("SELECT bucket FROM {} WHERE bucket=? FOR UPDATE", self.table("aip_tx_locks")), (bucket,)).await.map_err(db)?;
        if locked.is_none() {
            return Err("WIRE.DB_UNAVAILABLE");
        }
        let usage: Option<(i64, i64, i64)> = tx
            .exec_first(
                format!(
                    "SELECT COUNT(*), CAST(COALESCE(SUM(LENGTH(CAST(payload AS CHAR))),0) AS SIGNED), CAST(COALESCE(MAX(queue=? AND idempotency_key=?),0) AS SIGNED) \
                     FROM {} WHERE owner=? AND expires_at>UTC_TIMESTAMP(6)",
                    self.table("aip_message_queue")
                ),
                (queue, key, owner),
            )
            .await
            .map_err(db)?;
        let (count, bytes, existing) = usage.ok_or("WIRE.DB_UNAVAILABLE")?;
        if existing == 0 && (count >= 1000 || bytes + payload.to_string().len() as i64 > 64 * 1024 * 1024) {
            return Err("WIRE.QUOTA_EXCEEDED");
        }
        let message = self
            .messages()?
            .enqueue(
                tx,
                aip_execution::messages::NewMessage {
                    owner: owner.into(),
                    queue: queue.into(),
                    key: key.into(),
                    payload,
                    max_deliveries,
                    ttl_ms,
                    delay_ms,
                },
            )
            .await
            .map_err(message_error)?;
        Ok(json!({"message":message}))
    }

    fn events(&self) -> Result<aip_migrate::mysql_events::MysqlEventStore> {
        aip_migrate::mysql_events::MysqlEventStore::new(&self.schema).map_err(|_| "WIRE.DB_UNAVAILABLE")
    }

    fn store(&self) -> Result<MysqlJobStore> {
        MysqlJobStore::new(&self.schema).map_err(store_error)
    }
    fn table(&self, table: &str) -> String {
        format!("`{}`.`{table}`", self.schema)
    }
    fn contract(&self) -> String {
        spike_v5_sdk::contract_fingerprint_with_all_extensions(&self.facts, self.wire)
    }
    fn policy(&self, name: &str) -> spike_v6_transport::triggers::JobPolicy {
        self.policies.get(name).copied().unwrap_or_default()
    }

    fn bundle(&self) -> Result<String> {
        let mut paths = std::fs::read_dir(&self.workers.dir)
            .map_err(db)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db)?;
        paths.sort();
        let mut files = vec![];
        let mut bytes = 0usize;
        for path in paths {
            if !path.extension().is_some_and(|ext| ext == "py" || ext == "mjs") {
                continue;
            }
            let path = path.canonicalize().map_err(db)?;
            if !path.starts_with(&self.workers.dir) {
                return Err("WIRE.IMPLEMENTATION_CHANGED");
            }
            let content = std::fs::read(&path).map_err(db)?;
            bytes = bytes.saturating_add(content.len());
            if bytes > 32 << 20 {
                return Err("WIRE.COST_EXCEEDED");
            }
            files.push(json!([
                path.file_name().and_then(|name| name.to_str()).ok_or("WIRE.BAD_CONFIGURATION")?,
                spike_v1_fixture::digest(&json!(content))
            ]));
        }
        Ok(spike_v1_fixture::digest(&json!([
            "aip-mysql-managed-execution/1",
            format!("{:?}", self.workers.lang),
            spike_v4_worker::worker_abi_digest(),
            files,
            format!("{:?}", self.launch.isolation),
            self.launch.runtime_executable,
            self.launch.read_only_paths,
            self.launch.environment.iter().collect::<std::collections::BTreeMap<_, _>>(),
            self.launch.cgroup_root,
            self.launch.resource_limits.map(|limits| json!({"memoryBytes":limits.memory_bytes,"pidsMax":limits.pids_max,
                "cpuQuotaMicros":limits.cpu_quota_micros,"cpuPeriodMicros":limits.cpu_period_micros}))
        ])))
    }

    async fn begin(&self, grant: &DurablePrincipal) -> Result<FencedMysqlTransaction> {
        let mut fenced = self.driver.begin_fenced_write_transaction(&self.fence, true).await.map_err(fence_error)?;
        let tx = fenced.as_mut().map_err(db)?;
        tx.query_drop("SET SESSION time_zone = '+00:00', SESSION innodb_lock_wait_timeout = 2, SESSION max_execution_time = 5000")
            .await
            .map_err(db)?;
        self.check_grant(tx, grant).await?;
        Ok(fenced)
    }

    async fn check_grant(&self, tx: &mut Transaction<'static>, grant: &DurablePrincipal) -> Result<()> {
        let row: Option<(i64, i64, i64)> = tx
            .exec_first(
                format!("SELECT actor_id,enabled,min_iat FROM {} WHERE issuer=? AND subject=? FOR SHARE", self.table("aip_principals")),
                (&grant.issuer, &grant.subject),
            )
            .await
            .map_err(db)?;
        if !row.is_some_and(|(actor, enabled, floor)| actor == grant.actor_id && enabled == 1 && floor == grant.min_iat && grant.issued_at >= floor) {
            return Err("WIRE.UNAUTHENTICATED");
        }
        if self.facts["actorMode"] != "principal" {
            let actor = self.facts["actor"].as_str().ok_or("WIRE.BAD_CONFIGURATION")?;
            let view = self.table(&sqlgen::snake(actor));
            let sql = if spike_v2_read::id_wire::id_kind(&self.facts, actor) == "uuid" {
                format!("SELECT 1 FROM {view} a JOIN {} i ON i.resource_id=a.id WHERE i.id=? FOR SHARE", self.table("aip_actor_identities"))
            } else {
                format!("SELECT 1 FROM {view} WHERE id=? FOR SHARE")
            };
            let present: Option<i64> = tx.exec_first(sql, (grant.actor_id,)).await.map_err(db)?;
            if present.is_none() {
                return Err("WIRE.UNAUTHENTICATED");
            }
        }
        Ok(())
    }

    async fn authorize(&self, tx: &mut Transaction<'static>, operation: &str, input: &Value, actor: i64) -> Result<Caller> {
        let now: String =
            tx.query_first("SELECT DATE_FORMAT(UTC_TIMESTAMP(6),'%Y-%m-%dT%H:%i:%s.%fZ')").await.map_err(db)?.ok_or("WIRE.DB_UNAVAILABLE")?;
        let caller = Caller { actor_id: Some(actor), now };
        let plan = spike_v2_read::plan::plan_operation_authorization_in(sqlgen::Dialect::MySql, &self.facts, operation, input, &caller, self.wire)
            .map_err(|error| error.code)?;
        let params: Vec<SqlValue> = plan.params.into_iter().map(|value| value.map_or(SqlValue::NULL, Into::into)).collect();
        let allowed: Option<i64> = tx.exec_first(plan.sql, params).await.map_err(db)?;
        if allowed != Some(1) {
            return Err("WIRE.ACCESS_DENIED");
        }
        Ok(caller)
    }

    async fn commit(fenced: FencedMysqlTransaction) -> Result<()> {
        fenced.commit_classified(|| ()).await.map_err(|failure| match failure {
            CommitFailure::Unknown => "WIRE.COMMIT_UNKNOWN",
            _ => "WIRE.DB_UNAVAILABLE",
        })
    }

    async fn locked_lease(&self, tx: &mut Transaction<'static>, lease: &aip_execution::Lease) -> Result<()> {
        let row: Option<i64> = tx
            .exec_first(
                format!(
                    "SELECT 1 FROM {} WHERE id=? AND lease_epoch=? AND lease_owner=? AND owner=? AND state='RUNNING' \
             AND lease_until>UTC_TIMESTAMP(6) AND attempt_deadline>UTC_TIMESTAMP(6) FOR UPDATE",
                    self.table("aip_jobs")
                ),
                (lease.run_id.to_string(), lease.epoch, &lease.worker, &lease.owner),
            )
            .await
            .map_err(db)?;
        if row.is_none() {
            return Err("WIRE.LEASE_LOST");
        }
        if lease.deployment != self.deployment || lease.contract != self.contract() {
            return Err("WIRE.DEPLOYMENT_CHANGED");
        }
        if lease.implementation != self.bundle()? {
            return Err("WIRE.IMPLEMENTATION_CHANGED");
        }
        Ok(())
    }

    async fn request(&self, path: &str, body: &Value, grant: &DurablePrincipal) -> Result<Value> {
        let mut fenced = self.begin(grant).await?;
        let tx = fenced.as_mut().map_err(db)?;
        let owner = owner(grant);
        let store = self.store()?;
        let response = match path {
            "/jobs/enqueue" => {
                if !exact(body, &["operation", "input", "key", "delayMs"]) {
                    return Err("WIRE.BAD_REQUEST");
                }
                let name = text(body, "operation")?;
                let key = text(body, "key")?;
                let delay = body["delayMs"].as_i64().ok_or("WIRE.BAD_REQUEST")?;
                let run = self.submit(tx, grant, name, &body["input"], key, delay, None).await?;
                json!({"ok":true,"run":run})
            }
            "/jobs/status" | "/jobs/cancel" | "/jobs/result" => {
                if !exact(body, &["id"]) {
                    return Err("WIRE.BAD_REQUEST");
                }
                let id = text(body, "id")?;
                let mut run = if path == "/jobs/cancel" { store.cancel(tx, id, &owner).await } else { store.status(tx, id, &owner).await }
                    .map_err(store_error)?
                    .ok_or("WIRE.NOT_FOUND")?;
                if path == "/jobs/result" {
                    let result = store.result(tx, id, &owner).await.map_err(store_error)?.ok_or(match run.state.as_str() {
                        "SUCCEEDED" => "WIRE.RESULT_EXPIRED",
                        "FAILED" => "WIRE.JOB_FAILED",
                        _ => "WIRE.JOB_NOT_READY",
                    })?;
                    let stored: Option<(String, String, String)> = tx
                        .exec_first(
                            format!(
                                "SELECT CAST(input AS CHAR),contract_digest,deployment_digest FROM {} WHERE id=? AND owner=?",
                                self.table("aip_jobs")
                            ),
                            (id, &owner),
                        )
                        .await
                        .map_err(db)?;
                    let (input, contract, deployment) = stored.ok_or("WIRE.NOT_FOUND")?;
                    if contract != self.contract() || deployment != self.deployment {
                        return Err("WIRE.DEPLOYMENT_CHANGED");
                    }
                    self.authorize(tx, &result.operation, &serde_json::from_str::<Value>(&input).map_err(db)?, grant.actor_id).await?;
                    json!({"ok":true,"id":result.id,"operation":result.operation,"output":result.output})
                } else {
                    if matches!(run.state.as_str(), "FAILED" | "CANCELLED") && self.pending(tx, id).await? {
                        run.error = Some(if path == "/jobs/cancel" { "WIRE.CANCELLATION_UNCONFIRMED" } else { "WIRE.EFFECT_UNKNOWN" }.into());
                    }
                    run.output = None;
                    json!({"ok":true,"run":run})
                }
            }
            "/events/read" => {
                if !exact(body, &["stream", "after", "limit"]) {
                    return Err("WIRE.BAD_REQUEST");
                }
                let stream = text(body, "stream")?;
                // A stream is exposed to clients only through a job that declares `events.<stream>`.
                if !self.exposes(&format!("events.{stream}")) {
                    return Err("WIRE.NOT_EXPOSED");
                }
                let limit = body["limit"].as_i64().filter(|n| (1..=100).contains(n)).ok_or("WIRE.BAD_VALUE")?;
                let page = self.events()?.scan(tx, &owner, stream, text(body, "after")?, limit).await.map_err(|error| match error {
                    aip_execution::events::Error::CursorExpired => "WIRE.CURSOR_EXPIRED",
                    aip_execution::events::Error::InvalidEvent => "WIRE.BAD_VALUE",
                    _ => "WIRE.DB_UNAVAILABLE",
                })?;
                json!({"ok":true,"events":page.events.iter().map(|e|json!({"cursor":e.cursor,"payload":e.payload})).collect::<Vec<_>>(),"cursor":page.next_cursor})
            }
            path if path.starts_with("/messages/") => self.messages_request(tx, &owner, path, body).await?,
            _ => return Err("WIRE.NOT_FOUND"),
        };
        Self::commit(fenced).await?;
        Ok(response)
    }

    /// The client side of the message queues. A queue is exposed only through a job that declares `messages.<queue>`, and
    /// publishing and consuming are separate switches that default to off.
    async fn messages_request(&self, tx: &mut Transaction<'static>, owner: &str, path: &str, body: &Value) -> Result<Value> {
        use aip_execution::messages::MessageLease;
        if !self.messaging.enabled {
            return Err("WIRE.NOT_EXPOSED");
        }
        match path {
            "/messages/enqueue" if !self.messaging.client_publish => return Err("WIRE.NOT_EXPOSED"),
            "/messages/claim" | "/messages/ack" | "/messages/nack" | "/messages/dead" | "/messages/redrive" if !self.messaging.client_consume => {
                return Err("WIRE.NOT_EXPOSED");
            }
            "/messages/enqueue" | "/messages/claim" | "/messages/ack" | "/messages/nack" | "/messages/dead" | "/messages/redrive" => {}
            _ => return Err("WIRE.NOT_FOUND"),
        }
        let queue = text(body, "queue")?;
        if !self.exposes(&format!("messages.{queue}")) {
            return Err("WIRE.NOT_EXPOSED");
        }
        let store = self.messages()?;
        let mut out = match path {
            "/messages/enqueue" => {
                shape(body, &["queue", "key", "payload"], &["delayMs", "ttlMs", "maxDeliveries"])?;
                self.publish_message(
                    tx,
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
                json!({"lease":store.claim(tx, owner, queue, text(body, "worker")?, number(body, "leaseMs", 30_000, 100, 300_000)?).await.map_err(message_error)?})
            }
            "/messages/ack" | "/messages/nack" => {
                shape(body, &["queue", "lease"], if path.ends_with("nack") { &["error"] } else { &[] })?;
                let mut lease: MessageLease = serde_json::from_value(body["lease"].clone()).map_err(|_| "WIRE.BAD_REQUEST")?;
                lease.owner = owner.into();
                lease.queue = queue.into();
                if path.ends_with("/ack") {
                    json!({"acknowledged":store.ack(tx, &lease).await.map_err(message_error)?})
                } else {
                    let error = body.get("error").map_or(Ok("WIRE.CONSUMER_FAILED"), |value| value.as_str().ok_or("WIRE.BAD_REQUEST"))?;
                    json!({"released":store.nack(tx, &lease, error).await.map_err(message_error)?})
                }
            }
            "/messages/dead" => {
                shape(body, &["queue"], &["limit", "after"])?;
                let after = body.get("after").map(|value| value.as_str().ok_or("WIRE.BAD_REQUEST")).transpose()?;
                serde_json::to_value(
                    store.dead_letters_after(tx, owner, queue, after, number(body, "limit", 100, 1, 100)?).await.map_err(message_error)?,
                )
                .map_err(|_| "WIRE.INTERNAL")?
            }
            _ => {
                shape(body, &["queue", "id"], &["maxDeliveries"])?;
                json!({"redriven":store.redrive(tx, owner, queue, text(body, "id")?, number(body, "maxDeliveries", 3, 1, 100)? as i32).await.map_err(message_error)?})
            }
        };
        out["ok"] = json!(true);
        Ok(out)
    }

    /// Accepts one durable job inside the caller's fenced transaction. A user enqueue and a server trigger share every check
    /// that protects the business data; a trigger only skips the two that guard server-minted keys and verified inputs.
    #[allow(clippy::too_many_arguments)]
    async fn submit(
        &self,
        tx: &mut Transaction<'static>,
        grant: &DurablePrincipal,
        name: &str,
        input: &Value,
        key: &str,
        delay: i64,
        trigger: Option<&Trigger>,
    ) -> Result<aip_execution::Run> {
        spike_v4_worker::validate_job(&self.facts, name, input, self.wire).map_err(|error| error.code)?;
        if !valid_key(key) || !(0..=86_400_000).contains(&delay) {
            return Err("WIRE.BAD_VALUE");
        }
        if trigger.is_none() {
            // Trigger keys are minted by the server, and a trigger's operation trusts input the server verified (signed body,
            // configured schedule input, committed outbox row). A user who passes the operation's `allow` may do neither.
            if spike_v6_transport::triggers::RESERVED_KEY_PREFIXES.iter().any(|prefix| key.starts_with(prefix)) {
                return Err("WIRE.BAD_VALUE");
            }
            if self.triggers.targets_operation(name) {
                return Err("WIRE.ACCESS_DENIED");
            }
        }
        self.authorize(tx, name, input, grant.actor_id).await?;
        let owner = owner(grant);
        let digest = spike_v1_fixture::digest(&json!(["aip-jobs", self.schema, owner]));
        let bucket = u64::from_str_radix(&digest[..8], 16).map_err(db)? % 1000;
        let locked: Option<u64> =
            tx.exec_first(format!("SELECT bucket FROM {} WHERE bucket=? FOR UPDATE", self.table("aip_tx_locks")), (bucket,)).await.map_err(db)?;
        if locked.is_none() {
            return Err("WIRE.DB_UNAVAILABLE");
        }
        let contract = self.contract();
        let replay: Option<i64> = tx
            .exec_first(
                format!(
                    "SELECT 1 FROM {} WHERE owner=? AND operation=? AND idempotency_key=? AND contract_digest=? AND deployment_digest=?",
                    self.table("aip_jobs")
                ),
                (&owner, name, key, &contract, &self.deployment),
            )
            .await
            .map_err(db)?;
        if replay.is_none() {
            let counts: Option<(i64, i64, i64)> = tx
                .exec_first(
                    format!(
                        "SELECT COUNT(*),COALESCE(SUM(state IN ('QUEUED','RUNNING','CANCEL_REQUESTED')),0), \
                         COALESCE(SUM(CASE WHEN state IN ('QUEUED','RUNNING','CANCEL_REQUESTED') OR result_expires_at>UTC_TIMESTAMP(6) \
                         THEN OCTET_LENGTH(CAST(input AS CHAR))+COALESCE(OCTET_LENGTH(CAST(output AS CHAR)),0) ELSE 0 END),0) FROM {} WHERE owner=?",
                        self.table("aip_jobs")
                    ),
                    (&owner,),
                )
                .await
                .map_err(db)?;
            let (total, active, bytes) = counts.ok_or("WIRE.DB_UNAVAILABLE")?;
            // Trigger traffic is unbounded over time. A retained-history cap would silently stop every trigger that shares the
            // service principal, while the active and payload bounds still limit its load.
            if (trigger.is_none() && total >= 10_000) || active >= 100 || bytes.saturating_add(input.to_string().len() as i64) > 64 * 1024 * 1024 {
                return Err("WIRE.QUOTA_EXCEEDED");
            }
        }
        let trace = aip_execution::trace_context::current().unwrap_or_else(aip_execution::trace_context::TraceContext::generate).job_value();
        let run = self
            .store()?
            .enqueue(
                tx,
                aip_execution::NewRun {
                    owner,
                    actor: grant.actor_id,
                    grant: serde_json::to_value(grant).map_err(db)?,
                    operation: name.into(),
                    contract,
                    deployment: self.deployment.clone(),
                    implementation: self.bundle()?,
                    input: input.clone(),
                    key: key.into(),
                    timeout_ms: self.facts["operations"][name]["deadlineMs"].as_i64().ok_or("WIRE.BAD_CONFIGURATION")?,
                    max_attempts: self.policy(name).max_attempts,
                    ttl_ms: 86_400_000,
                    delay_ms: delay,
                    trace_id: Some(trace),
                },
            )
            .await
            .map_err(store_error)?;
        if let Some(outbox_id) = trigger.and_then(|trigger| trigger.outbox_id) {
            // The receipt commits with the job, so an outbox row is never marked dispatched without its job, and a retry of the
            // same row converges on the same key.
            tx.exec_drop(
                format!(
                    "INSERT INTO {} (outbox_id, outbox_created_at, job_id) SELECT id, created_at, ? FROM {} WHERE id=? \
                     ON DUPLICATE KEY UPDATE outbox_id=outbox_id",
                    self.table("aip_outbox_dispatch"),
                    self.table("aip_outbox")
                ),
                (&run.id, outbox_id),
            )
            .await
            .map_err(db)?;
        }
        Ok(run)
    }

    /// The service principal's grant as of now. A disabled or unbound principal accepts nothing.
    async fn service_grant(&self, principal: &spike_v6_transport::triggers::ServicePrincipal) -> Result<DurablePrincipal> {
        let mut conn = self.driver.pool().get_conn().await.map_err(db)?;
        let row: Option<(i64, i64, i64, i64)> = conn
            .exec_first(
                format!("SELECT actor_id,enabled,min_iat,UNIX_TIMESTAMP() FROM {} WHERE issuer=? AND subject=?", self.table("aip_principals")),
                (&principal.issuer, &principal.subject),
            )
            .await
            .map_err(db)?;
        let (actor_id, _, min_iat, issued_at) = row.filter(|row| row.1 == 1).ok_or("WIRE.PRINCIPAL_UNAVAILABLE")?;
        Ok(DurablePrincipal { issuer: principal.issuer.clone(), subject: principal.subject.clone(), actor_id, min_iat, issued_at })
    }

    /// One trigger submission in its own fenced transaction. `begin` re-reads the principal under a shared lock, so a binding
    /// revoked after `service_grant` is still refused.
    async fn submit_trigger(
        &self,
        principal: &spike_v6_transport::triggers::ServicePrincipal,
        name: &str,
        input: &Value,
        key: &str,
        outbox_id: Option<i64>,
    ) -> Result<aip_execution::Run> {
        let grant = self.service_grant(principal).await?;
        let mut fenced = self.begin(&grant).await?;
        let tx = fenced.as_mut().map_err(db)?;
        let run = self.submit(tx, &grant, name, input, key, 0, Some(&Trigger { outbox_id })).await?;
        Self::commit(fenced).await?;
        Ok(run)
    }

    /// Fires the latest due slot of each schedule once. Older missed slots are skipped by design (coalesced), and the per-slot
    /// job key makes concurrent instances and restarts converge on one run.
    async fn fire_schedules(&self) {
        for schedule in &self.triggers.schedules {
            let Some(slot) = schedule.due_slot() else { continue };
            let key = spike_v6_transport::triggers::schedule_key(&schedule.name, slot);
            if let Err(code) = self.submit_trigger(&schedule.principal, &schedule.operation, &schedule.input, &key, None).await {
                eprintln!("{}", json!({"event":"schedule_submit_failed","schedule":schedule.name,"code":code}));
            }
        }
    }

    /// Turns committed `notify` rows of configured topics into jobs. Each topic is scanned separately so a handler that keeps
    /// failing cannot starve the others; each row becomes exactly one job through its key and one dispatch receipt.
    async fn relay_outbox(&self) -> Result<()> {
        for handler in &self.triggers.outbox {
            let rows: Vec<OutboxRow> = {
                let mut conn = self.driver.pool().get_conn().await.map_err(db)?;
                conn.query_drop("SET SESSION time_zone = '+00:00'").await.map_err(db)?;
                conn.exec(
                    format!(
                        "SELECT o.id, o.source, o.source_id, o.recipient_id, DATE_FORMAT(o.created_at, '%Y-%m-%dT%H:%i:%s.%fZ'), \
                         CAST(TIMESTAMPDIFF(MICROSECOND, '1970-01-01 00:00:00', o.created_at) AS SIGNED) FROM {} o \
                         WHERE o.topic=? AND NOT EXISTS (SELECT 1 FROM {} d WHERE d.outbox_id=o.id) ORDER BY o.id LIMIT 50",
                        self.table("aip_outbox"),
                        self.table("aip_outbox_dispatch")
                    ),
                    (&handler.topic,),
                )
                .await
                .map_err(db)?
            };
            for (id, source, source_id, recipient_id, created_at, created_us) in rows {
                let event = json!({"id":id.to_string(),"topic":handler.topic,"source":source,"sourceId":source_id,
                    "recipientId":recipient_id,"createdAt":created_at});
                let key = spike_v6_transport::triggers::outbox_key(id, created_us);
                if let Err(code) =
                    self.submit_trigger(&handler.principal, &handler.operation, &json!({"event":event.to_string()}), &key, Some(id)).await
                {
                    eprintln!("{}", json!({"event":"outbox_dispatch_failed","topic":handler.topic,"code":code}));
                    break;
                }
            }
        }
        Ok(())
    }

    async fn pending(&self, tx: &mut Transaction<'static>, id: &str) -> Result<bool> {
        let pending: Option<i64> = tx
            .exec_first(
                format!("SELECT EXISTS(SELECT 1 FROM {} WHERE run_id=? AND state IN ('PENDING','UNKNOWN'))", self.table("aip_effects")),
                (id,),
            )
            .await
            .map_err(db)?;
        Ok(pending == Some(1))
    }
}

impl RelationalJobs for MysqlJobs {
    fn handle<'a>(&'a self, path: &'a str, body: &'a Value, grant: &'a DurablePrincipal) -> BackendFuture<'a, Value> {
        Box::pin(async move {
            match self.request(path, body, grant).await {
                Ok(value) => value,
                Err(code) => json!({"ok":false,"code":code}),
            }
        })
    }
    fn supervise(self: Arc<Self>) -> BackendFuture<'static, ()> {
        Box::pin(async move { self.run().await })
    }
    fn submit_service<'a>(
        &'a self,
        principal: &'a spike_v6_transport::triggers::ServicePrincipal,
        operation: &'a str,
        input: &'a Value,
        key: &'a str,
    ) -> BackendFuture<'a, Result<aip_execution::Run>> {
        Box::pin(self.submit_trigger(principal, operation, input, key, None))
    }
}

impl MysqlJobs {
    async fn admission(&self, lease: &aip_execution::Lease, grant: &DurablePrincipal) -> Result<Caller> {
        let mut fenced = self.begin(grant).await?;
        let tx = fenced.as_mut().map_err(db)?;
        self.locked_lease(tx, lease).await?;
        let caller = self.authorize(tx, &lease.operation, &lease.input, lease.actor).await?;
        Self::commit(fenced).await?;
        Ok(caller)
    }

    async fn beat(&self, lease: &aip_execution::Lease, grant: &DurablePrincipal) -> Result<()> {
        let mut fenced = self.begin(grant).await?;
        let tx = fenced.as_mut().map_err(db)?;
        self.locked_lease(tx, lease).await?;
        self.authorize(tx, &lease.operation, &lease.input, lease.actor).await?;
        if !self.store()?.heartbeat(tx, lease).await.map_err(store_error)? {
            return Err("WIRE.LEASE_LOST");
        }
        Self::commit(fenced).await
    }

    async fn execute(self: Arc<Self>, lease: aip_execution::Lease) {
        let started = std::time::Instant::now();
        let grant = serde_json::from_value::<DurablePrincipal>(lease.grant.clone());
        let mut after = None;
        let result = async {
            let grant = grant.map_err(|_| "WIRE.UNAUTHENTICATED")?;
            let caller = self.admission(&lease, &grant).await?;
            let dir = self.workers.dir.to_str().ok_or("WIRE.BAD_CONFIGURATION")?;
            let mut worker =
                spike_v4_worker::Worker::try_start_configured(self.workers.lang, dir, self.launch.clone()).await.map_err(|_| "WIRE.WORKER_FAILED")?;
            let mut host = JobHost {
                runtime: self.clone(),
                lease: lease.clone(),
                grant: grant.clone(),
                prefix: String::new(),
                depth: 0,
                budget: Arc::new(AtomicUsize::new(0)),
                capabilities: self.facts["operations"][&lease.operation]["access"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(key, _)| key.clone())
                    .collect(),
            };
            let result = {
                let future = spike_v4_worker::invoke_job(&mut worker, &self.facts, &lease.operation, &lease.input, &caller, self.wire, &mut host);
                tokio::pin!(future);
                let monitor = async {
                    let mut timer = tokio::time::interval(Duration::from_millis(250));
                    let mut last = tokio::time::Instant::now();
                    loop {
                        timer.tick().await;
                        match self.beat(&lease, &grant).await {
                            Ok(()) => last = tokio::time::Instant::now(),
                            Err("WIRE.DB_UNAVAILABLE") if last.elapsed() < Duration::from_secs(12) => {}
                            Err(code) => return Err::<Value, _>(code),
                        }
                    }
                };
                tokio::select! {
                    output=&mut future=>output.map_err(|error| {after=spike_v4_worker::retry_after_ms(&error);error.code}),
                    output=monitor=>output,
                }
            };
            worker.stop().await;
            let output = result?;
            self.admission(&lease, &grant).await?;
            Ok(output)
        }
        .await;
        let code = result.as_ref().err().copied();
        let retry = code.is_some_and(|code| {
            matches!(code, "WIRE.DB_UNAVAILABLE" | "WIRE.WORKER_FAILED" | "WIRE.RETRY_LATER")
                || code == "WIRE.EXTENSION_ERROR" && self.policy(&lease.operation).retry_uncaught
        });
        if let Err(code) = self.settle(&lease, result, retry, after).await {
            eprintln!("{}", json!({"event":"mysql_job_outcome_rejected","id":lease.run_id.to_string(),"code":code}));
        }
        eprintln!(
            "{}",
            json!({"event":"mysql_job_attempt_finished","id":lease.run_id.to_string(),"attempt":lease.attempt,
            "elapsedMs":started.elapsed().as_millis(),"code":code})
        );
    }

    async fn settle(&self, lease: &aip_execution::Lease, mut output: Result<Value>, retry: bool, after: Option<i64>) -> Result<()> {
        // Recording a refusal or an observed process stop is allowed after revocation. No business capability runs here.
        let mut fenced = self.driver.begin_fenced_write_transaction(&self.fence, true).await.map_err(fence_error)?;
        let tx = fenced.as_mut().map_err(db)?;
        let state: Option<String> = tx
            .exec_first(
                format!("SELECT state FROM {} WHERE id=? AND lease_epoch=? AND lease_owner=? FOR UPDATE", self.table("aip_jobs")),
                (lease.run_id.to_string(), lease.epoch, &lease.worker),
            )
            .await
            .map_err(db)?;
        let pending = self.pending(tx, &lease.run_id.to_string()).await?;
        let store = self.store()?;
        let updated = if state.as_deref() == Some("CANCEL_REQUESTED") {
            if pending { store.reject_cancel(tx, lease, "WIRE.CANCELLATION_UNCONFIRMED").await } else { store.acknowledge_cancel(tx, lease).await }
        } else {
            if pending {
                output = Err("WIRE.EFFECT_UNKNOWN");
            }
            // A principal can be revoked or an implementation changed between the last callback and this COMMIT.
            // Keep its binding shared-locked until the result becomes visible.
            if output.is_ok() {
                let grant = serde_json::from_value::<DurablePrincipal>(lease.grant.clone()).map_err(|_| "WIRE.UNAUTHENTICATED")?;
                let checked = async {
                    self.check_grant(tx, &grant).await?;
                    self.locked_lease(tx, lease).await?;
                    self.authorize(tx, &lease.operation, &lease.input, lease.actor).await?;
                    Ok::<(), &'static str>(())
                }
                .await;
                if let Err(code) = checked {
                    if code == "WIRE.DB_UNAVAILABLE" {
                        return Err(code);
                    }
                    output = Err(code);
                }
            }
            match output {
                Ok(value) => store.finish(tx, lease, value).await,
                Err(code) if retry && !pending => store.retry_after(tx, lease, code, after).await,
                Err(code) => store.fail(tx, lease, code).await,
            }
        }
        .map_err(store_error)?;
        if !updated {
            return Err("WIRE.LEASE_LOST");
        }
        Self::commit(fenced).await
    }

    async fn run(self: Arc<Self>) {
        let worker = format!(
            "mysql-jobs-{}-{}",
            std::process::id(),
            spike_v1_fixture::digest(&json!(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()))
        );
        let mut tasks = tokio::task::JoinSet::new();
        let mut gc = tokio::time::Instant::now();
        let (mut scheduled, mut relayed): (Option<tokio::time::Instant>, Option<tokio::time::Instant>) = (None, None);
        loop {
            if !self.triggers.outbox.is_empty() && relayed.is_none_or(|at| at.elapsed() >= OUTBOX_TICK) {
                if let Err(code) = self.relay_outbox().await {
                    eprintln!("{}", json!({"event":"outbox_relay_failed","code":code}));
                }
                relayed = Some(tokio::time::Instant::now());
            }
            if !self.triggers.schedules.is_empty() && scheduled.is_none_or(|at| at.elapsed() >= SCHEDULE_TICK) {
                self.fire_schedules().await;
                scheduled = Some(tokio::time::Instant::now());
            }
            while tasks.len() < self.concurrency {
                let claim = async {
                    let mut conn = self.driver.pool().get_conn().await.map_err(db)?;
                    self.store()?.claim(&mut conn, &worker).await.map_err(store_error)
                }
                .await;
                match claim {
                    Ok(Some(lease)) => {
                        let runtime = self.clone();
                        tasks.spawn(async move {
                            let context = lease
                                .trace_id
                                .as_ref()
                                .map(|value| aip_execution::trace_context::TraceContext::from_job_value(value))
                                .unwrap_or_else(aip_execution::trace_context::TraceContext::generate);
                            aip_execution::trace_context::scope(async move {
                                aip_execution::trace_context::set(context);
                                runtime.execute(lease).await;
                            })
                            .await;
                        });
                    }
                    Ok(None) => break,
                    Err(code) => {
                        eprintln!("{}", json!({"event":"mysql_job_claim_failed","code":code}));
                        break;
                    }
                }
            }
            if gc.elapsed() > Duration::from_secs(30) {
                let sweep = async {
                    let mut conn = self.driver.pool().get_conn().await.map_err(db)?;
                    self.store()?.expire_payloads(&mut conn).await.map_err(store_error)?;
                    Ok::<(), &'static str>(())
                }
                .await;
                if let Err(code) = sweep {
                    eprintln!("{}", json!({"event":"mysql_job_gc_failed","code":code}));
                }
                gc = tokio::time::Instant::now();
            }
            tokio::select! {
                _=tokio::time::sleep(Duration::from_millis(100))=>{},
                Some(result)=tasks.join_next(),if !tasks.is_empty()=>{if result.is_err(){eprintln!("{{\"event\":\"mysql_job_task_failed\"}}");}},
            }
        }
        // JoinSet aborts its attempts when the server drops this supervisor. Worker owns kill-on-drop children.
    }
}

struct JobHost {
    runtime: Arc<MysqlJobs>,
    lease: aip_execution::Lease,
    grant: DurablePrincipal,
    prefix: String,
    depth: usize,
    budget: Arc<AtomicUsize>,
    capabilities: Vec<String>,
}
impl spike_v4_worker::JobHost for JobHost {
    fn call<'a>(&'a mut self, op: &'a str, name: &'a str, input: &'a Value) -> BackendFuture<'a, std::result::Result<Value, Reject>> {
        Box::pin(async move { self.dispatch(op, name, input).await.map_err(|code| Reject { code, msg: "MySQL managed capability 거부".into() }) })
    }
    fn job(&self) -> Value {
        json!({"id":self.lease.run_id.to_string(),"attempt":self.lease.attempt})
    }
}

fn replay_error(code: &str) -> &'static str {
    spike_v6_transport::relational_jobs::data_error(code)
}
impl JobHost {
    async fn dispatch(&mut self, op: &str, name: &str, input: &Value) -> Result<Value> {
        if self.depth > 8 || self.budget.fetch_add(1, Ordering::Relaxed) >= 64 {
            return Err("WIRE.COST_EXCEEDED");
        }
        if !self.capabilities.contains(&spike_v4_worker::job_capability(op, name)) {
            return Err("WIRE.ACCESS_NOT_DECLARED");
        }
        let rt = &self.runtime;
        let mut fenced = rt.begin(&self.grant).await?;
        let tx = fenced.as_mut().map_err(db)?;
        rt.locked_lease(tx, &self.lease).await?;
        let caller = rt.authorize(tx, &self.lease.operation, &self.lease.input, self.lease.actor).await?;
        if matches!(op, "read" | "aggregate") {
            let request = if op == "aggregate" {
                json!({"aggregate":name,"input":input})
            } else {
                if input["read"] != name {
                    return Err("WIRE.BAD_VALUE");
                }
                input.clone()
            };
            let plan = spike_v2_read::plan::plan_read_in(sqlgen::Dialect::MySql, &rt.facts, &request, &caller, rt.wire).map_err(|e| e.code)?;
            MysqlJobs::commit(fenced).await?;
            let rows = tokio::time::timeout(Duration::from_millis(plan.deadline_ms), rt.backend.read(&plan, rt.wire))
                .await
                .map_err(|_| "WIRE.DEADLINE_EXCEEDED")?
                .map_err(|e| e.code)?;
            return if op == "aggregate" {
                Ok(json!({"value":rows.first().cloned().unwrap_or(Value::Null)}))
            } else {
                Ok(json!({"rows":spike_v2_read::id_wire::encode_rows(rows,&plan.output_type,rt.wire).map_err(|e|e.code)?}))
            };
        }
        let key = text(input, "key")?;
        if !valid_key(key) {
            return Err("WIRE.BAD_VALUE");
        }
        let path = format!("{}{op}.{name}:{}", self.prefix, spike_v1_fixture::digest(&json!(key)));
        let effect = spike_v1_fixture::digest(&json!(path));
        let digest = spike_v1_fixture::digest(&json!([op, name, input, self.lease.implementation]));
        if op == "jobs" {
            rt.authorize(tx, name, &input["input"], self.lease.actor).await?;
        }
        let prior: Option<(String, String, Option<String>)> = tx
            .exec_first(
                format!("SELECT request_digest,state,CAST(result AS CHAR) FROM {} WHERE run_id=? AND effect_key=?", rt.table("aip_effects")),
                (self.lease.run_id.to_string(), &effect),
            )
            .await
            .map_err(db)?;
        if let Some((request, state, result)) = prior {
            if request != digest {
                return Err("WIRE.IDEMPOTENCY_MISMATCH");
            }
            if state == "SUCCEEDED" {
                let result = serde_json::from_str(&result.ok_or("WIRE.DB_UNAVAILABLE")?).map_err(db)?;
                MysqlJobs::commit(fenced).await?;
                return Ok(result);
            }
            if state == "FAILED" {
                let result: Value = serde_json::from_str(&result.ok_or("WIRE.DB_UNAVAILABLE")?).map_err(db)?;
                return Err(replay_error(result["code"].as_str().unwrap_or("")));
            }
            // A nested job reruns its steps, and a webhook subscriber dedupes on webhook-id, so a delivery whose outcome was lost
            // is sent again. Any other call whose outcome is unknown may have happened and is never repeated.
            let resendable = op == "jobs" || op == "http" && rt.broker.as_ref().is_some_and(|broker| broker.is_webhook(name));
            if !resendable {
                return Err("WIRE.EFFECT_UNKNOWN");
            }
        }
        tx.exec_drop(
            format!("INSERT IGNORE INTO {}(run_id,effect_key,request_digest,state) VALUES(?,?,?,'PENDING')", rt.table("aip_effects")),
            (self.lease.run_id.to_string(), &effect, &digest),
        )
        .await
        .map_err(db)?;
        if op == "apply" {
            let request = spike_v4_worker::write::job_apply_request(name, input, rt.wire).map_err(|e| e.code)?;
            tx.query_drop("SAVEPOINT aip_job_effect").await.map_err(db)?;
            let output = aip_migrate::mysql_write::apply_job_effect_in(
                tx,
                &rt.facts,
                &request,
                &caller,
                rt.wire,
                &format!("{}:{}", self.lease.run_id, &effect[..32]),
            )
            .await;
            let (state, value) = match &output {
                Ok(value) => ("SUCCEEDED", value.clone()),
                Err(error) => {
                    if matches!(error.code, "WIRE.DB_UNAVAILABLE" | "WIRE.INTERNAL" | "WIRE.CONFLICT" | "WIRE.DEADLINE_EXCEEDED") {
                        return Err(error.code);
                    }
                    tx.query_drop("ROLLBACK TO SAVEPOINT aip_job_effect").await.map_err(db)?;
                    ("FAILED", json!({"code":error.code}))
                }
            };
            tx.exec_drop(
                format!("UPDATE {} SET state=?,result=CAST(? AS JSON) WHERE run_id=? AND effect_key=?", rt.table("aip_effects")),
                (state, value.to_string(), self.lease.run_id.to_string(), &effect),
            )
            .await
            .map_err(db)?;
            MysqlJobs::commit(fenced).await?;
            return output.map_err(|error| error.code);
        }
        if op == "events" {
            // The event and its journal row commit together with the lease check, so a retry sees one cursor.
            let event_key = spike_v1_fixture::digest(&json!([self.lease.run_id.to_string(), self.prefix, key]));
            let cursor = rt.events()?.publish_in(tx, &self.lease.owner, name, &event_key, &input["payload"]).await.map_err(|error| match error {
                aip_execution::events::Error::IdempotencyConflict => "WIRE.IDEMPOTENCY_MISMATCH",
                aip_execution::events::Error::Database(_) => "WIRE.DB_UNAVAILABLE",
                _ => "WIRE.EVENT_INVALID",
            })?;
            let output = json!({"cursor":cursor});
            tx.exec_drop(
                format!("UPDATE {} SET state='SUCCEEDED',result=CAST(? AS JSON) WHERE run_id=? AND effect_key=?", rt.table("aip_effects")),
                (output.to_string(), self.lease.run_id.to_string(), &effect),
            )
            .await
            .map_err(db)?;
            MysqlJobs::commit(fenced).await?;
            return Ok(output);
        }
        if op == "messages" {
            if !rt.messaging.enabled {
                return Err("WIRE.NOT_EXPOSED");
            }
            // The message and its journal row commit together with the lease check, so a retry sees one message.
            let message_key = spike_v1_fixture::digest(&json!([self.lease.run_id.to_string(), self.prefix, key]));
            let output = rt.publish_message(tx, &self.lease.owner, name, &message_key, input["payload"].clone(), 0, 86_400_000, 3).await?;
            tx.exec_drop(
                format!("UPDATE {} SET state='SUCCEEDED',result=CAST(? AS JSON) WHERE run_id=? AND effect_key=?", rt.table("aip_effects")),
                (output.to_string(), self.lease.run_id.to_string(), &effect),
            )
            .await
            .map_err(db)?;
            MysqlJobs::commit(fenced).await?;
            return Ok(output);
        }
        if op == "http" {
            let broker = rt.broker.clone().ok_or("WIRE.NOT_EXPOSED")?;
            let webhook = broker.is_webhook(name).then(|| text(input, "url")).transpose()?;
            // The intent commits before the call leaves, so a crash after sending leaves a PENDING row instead of a silent repeat.
            MysqlJobs::commit(fenced).await?;
            // Err(Some(code)) is a certain not-sent (or safely resendable) outcome; Err(None) is unknown.
            let response: std::result::Result<Value, Option<&'static str>> = if let Some(url) = webhook {
                use aip_execution::integrations::Error as E;
                // Stable across attempts, so the subscriber's dedupe on webhook-id absorbs a resend.
                let id = format!("msg_{}", spike_v1_fixture::digest(&json!([self.lease.run_id.to_string(), effect])));
                match broker.deliver_webhook(name, url, &input["input"], &id).await {
                    Ok(status) if (200..300).contains(&status) => Ok(json!({"status":status,"id":id})),
                    Err(E::DestinationRejected) => Err(Some("WIRE.DESTINATION_REJECTED")),
                    Err(E::RequestTooLarge) => Err(Some("WIRE.BAD_VALUE")),
                    Ok(_) | Err(_) => Err(Some("WIRE.DELIVERY_FAILED")),
                }
            } else {
                let key = format!("{}:{effect}", self.lease.run_id);
                match broker.call_with_key(name, &input["input"], Some(&key)).await {
                    Ok(r) => Ok(json!({"status":r.status,"body":r.body,"retryAfterSeconds":r.retry_after_seconds})),
                    Err(aip_execution::integrations::Error::CircuitOpen) => Err(Some("WIRE.CIRCUIT_OPEN")),
                    Err(aip_execution::integrations::Error::NotSent) => Err(Some("WIRE.DELIVERY_FAILED")),
                    Err(aip_execution::integrations::Error::TokenUnavailable) => Err(Some("WIRE.INTEGRATION_AUTH_FAILED")),
                    Err(_) => Err(None),
                }
            };
            let mut fenced = rt.begin(&self.grant).await?;
            let tx = fenced.as_mut().map_err(db)?;
            rt.locked_lease(tx, &self.lease).await?;
            let table = rt.table("aip_effects");
            let run = self.lease.run_id.to_string();
            return match response {
                Ok(result) => {
                    tx.exec_drop(
                        format!("UPDATE {table} SET state='SUCCEEDED',result=CAST(? AS JSON) WHERE run_id=? AND effect_key=?"),
                        (result.to_string(), &run, &effect),
                    )
                    .await
                    .map_err(db)?;
                    MysqlJobs::commit(fenced).await?;
                    Ok(result)
                }
                Err(Some(code)) => {
                    // Drop the intent instead of leaving an unknown effect; the extension can catch the code or
                    // throw ctx.retryLater() and send the same key again.
                    tx.exec_drop(format!("DELETE FROM {table} WHERE run_id=? AND effect_key=? AND state='PENDING'"), (&run, &effect))
                        .await
                        .map_err(db)?;
                    MysqlJobs::commit(fenced).await?;
                    Err(code)
                }
                Err(None) => {
                    tx.exec_drop(format!("UPDATE {table} SET state='UNKNOWN' WHERE run_id=? AND effect_key=?"), (&run, &effect)).await.map_err(db)?;
                    MysqlJobs::commit(fenced).await?;
                    Err("WIRE.EFFECT_UNKNOWN")
                }
            };
        }
        if op == "jobs" {
            if self.depth >= 8 {
                return Err("WIRE.COST_EXCEEDED");
            }
            let caps = rt.facts["operations"][name]["access"].as_object().ok_or("WIRE.NOT_EXPOSED")?.keys().cloned().collect::<Vec<_>>();
            if caps.iter().any(|cap| !self.capabilities.contains(cap)) {
                return Err("WIRE.ACCESS_NOT_DECLARED");
            }
            let caller = rt.authorize(tx, name, &input["input"], self.lease.actor).await?;
            MysqlJobs::commit(fenced).await?;
            let mut worker = spike_v4_worker::Worker::try_start_configured(
                rt.workers.lang,
                rt.workers.dir.to_str().ok_or("WIRE.BAD_CONFIGURATION")?,
                rt.launch.clone(),
            )
            .await
            .map_err(|_| "WIRE.WORKER_FAILED")?;
            let mut child = JobHost {
                runtime: rt.clone(),
                lease: self.lease.clone(),
                grant: self.grant.clone(),
                prefix: format!("{path}/"),
                depth: self.depth + 1,
                budget: self.budget.clone(),
                capabilities: caps,
            };
            let output = if rt.facts["operations"][name]["kind"] == "read" {
                spike_v4_worker::invoke_operation(&mut worker, &rt.facts, name, &input["input"], &caller, rt.wire).await
            } else {
                spike_v4_worker::invoke_job(&mut worker, &rt.facts, name, &input["input"], &caller, rt.wire, &mut child).await
            };
            worker.stop().await;
            let mut fenced = rt.begin(&self.grant).await?;
            let tx = fenced.as_mut().map_err(db)?;
            rt.locked_lease(tx, &self.lease).await?;
            rt.authorize(tx, name, &input["input"], self.lease.actor).await?;
            let (state, value) = match &output {
                Ok(value) => ("SUCCEEDED", value.clone()),
                Err(error) => ("FAILED", json!({"code":error.code})),
            };
            tx.exec_drop(
                format!("UPDATE {} SET state=?,result=CAST(? AS JSON) WHERE run_id=? AND effect_key=?", rt.table("aip_effects")),
                (state, value.to_string(), self.lease.run_id.to_string(), effect),
            )
            .await
            .map_err(db)?;
            MysqlJobs::commit(fenced).await?;
            return output.map_err(|error| replay_error(error.code));
        }
        Err("WIRE.ACCESS_NOT_DECLARED")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    const META: &str = "aip_t_job_http";
    const SOURCE: &str = "aip_t_job_http_src";
    const DEFINITION: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Video {
 fields { id: Id; owner: Member; views: Int(0..100) default database }
 rows read when true
 transition increment { from views < 100; allow owner = actor; to views = views + 1 }
 expose apply increment { target id; bulk maxRows 1 }
 expose read { select id, views; sort id; budget { rows 20; depth 1; deadline 1s; cost 100 } }
}
operation job incrementJob {
 input { id: Video.Id }
 output { count: Int }
 allow actor != null
 access read.Video, Video.increment
 effect managed
 deadline 5s
 implementation "worker.jobView"
}
"#;
    struct Dir(std::path::PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    struct Paused(Arc<MysqlJobs>);
    impl RelationalJobs for Paused {
        fn handle<'a>(&'a self, path: &'a str, body: &'a Value, grant: &'a DurablePrincipal) -> BackendFuture<'a, Value> {
            self.0.handle(path, body, grant)
        }
        fn supervise(self: Arc<Self>) -> BackendFuture<'static, ()> {
            Box::pin(std::future::pending())
        }
    }
    fn grant(actor: i64) -> DurablePrincipal {
        DurablePrincipal { issuer: "https://test.example".into(), subject: format!("user-{actor}"), actor_id: actor, issued_at: 1, min_iat: 0 }
    }
    struct Auth;
    impl spike_v6_transport::auth::Authenticator for Auth {
        fn verify_session<'a>(
            &'a self,
            token: &'a str,
        ) -> BackendFuture<'a, std::result::Result<spike_v6_transport::auth::Session, spike_v6_transport::auth::AuthError>> {
            Box::pin(async move {
                let actor = token.parse::<i64>().map_err(|_| spike_v6_transport::auth::AuthError::BadSignature)?;
                spike_v6_transport::auth::Session::new_durable(grant(actor), i64::MAX / 1000)
            })
        }
    }
    async fn post(address: SocketAddr, path: &str, contract: &str, actor: Option<i64>, body: Value) -> Value {
        let auth = actor.map(|actor| format!("authorization: Bearer {actor}\r\n")).unwrap_or_default();
        let body = body.to_string();
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        socket
            .write_all(
                format!(
                    "POST {path} HTTP/1.1\r\nhost: localhost\r\n{auth}x-aip-contract: {contract}\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut output = String::new();
        tokio::time::timeout(Duration::from_secs(15), socket.read_to_string(&mut output)).await.unwrap().unwrap();
        serde_json::from_str(output.split_once("\r\n\r\n").unwrap().1).unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "requires a run-owned MySQL 8.4 fixture; direct HTTP persistence and callback journal, no worker subprocess"]
    async fn mysql_job_http_and_effect_journal_preserve_owner_lease_and_transaction_boundaries() {
        let url = std::env::var("AIP_TEST_MYSQL_URL").unwrap();
        aip_test_support::validate_mysql_admin_url(&url).unwrap();
        let admin = MysqlDriver::connect(&url).await.unwrap();
        let mut setup = admin.pool().get_conn().await.unwrap();
        for schema in [META, SOURCE] {
            setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
        }
        setup.query_drop(format!("CREATE DATABASE `{SOURCE}`")).await.unwrap();
        setup.query_drop(format!("CREATE TABLE `{SOURCE}`.member(id BIGINT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
        setup.query_drop(format!("CREATE TABLE `{SOURCE}`.video(id BIGINT PRIMARY KEY,owner_id BIGINT NOT NULL,views BIGINT NOT NULL DEFAULT 0,FOREIGN KEY(owner_id) REFERENCES `{SOURCE}`.member(id)) ENGINE=InnoDB")).await.unwrap();
        setup.query_drop(format!("INSERT INTO `{SOURCE}`.member VALUES(1),(2)")).await.unwrap();
        setup.query_drop(format!("INSERT INTO `{SOURCE}`.video VALUES(10,1,0),(11,2,0)")).await.unwrap();
        let facts = spike_v1_fixture::load_str(DEFINITION, spike_v1_fixture::Form::A).unwrap().execution;
        sqlgen::try_set_schema(META).unwrap();
        let digest = spike_v1_fixture::digest(&facts);
        let fingerprint = admin.ensure_meta_schema_bootstrap(META, SOURCE, &digest).await.unwrap();
        setup.query_drop(format!("CREATE VIEW `{META}`.member AS SELECT * FROM `{SOURCE}`.member")).await.unwrap();
        setup.query_drop(format!("CREATE VIEW `{META}`.video AS SELECT * FROM `{SOURCE}`.video")).await.unwrap();
        setup.query_drop(format!("INSERT INTO `{META}`.aip_principals(issuer,subject,actor_id) VALUES('https://test.example','user-1',1),('https://test.example','user-2',2)")).await.unwrap();
        setup.query_drop("DROP USER IF EXISTS 'aip_t_jobs_rt'@'%'").await.unwrap();
        setup.query_drop("CREATE USER 'aip_t_jobs_rt'@'%' IDENTIFIED BY 'jobs_test_only'").await.unwrap();
        for schema in [SOURCE, META] {
            setup
                .query_drop(format!("GRANT SELECT,INSERT,UPDATE,DELETE,CREATE TEMPORARY TABLES ON `{schema}`.* TO 'aip_t_jobs_rt'@'%'"))
                .await
                .unwrap();
        }
        let endpoint = url.split_once('@').unwrap().1.split('/').next().unwrap();
        let driver = MysqlDriver::from_url(&format!("mysql://aip_t_jobs_rt:jobs_test_only@{endpoint}/{SOURCE}")).unwrap();
        let fence = MysqlDeploymentFence::from_fingerprint(META, SOURCE, &digest, &fingerprint).unwrap();
        let backend = Arc::new(MysqlBackend::new(driver.clone(), fence.clone()));
        let dir = Dir(std::env::temp_dir().join(format!("aip-jobs-unit-{}", std::process::id())));
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(dir.0.join("worker.py"), "async def jobView(input, ctx):\n return {'count': 1}\n").unwrap();
        let workers = ReadExtensions { lang: spike_v6_transport::server::WorkerLang::Python, dir: dir.0.clone() };
        let mut infra = spike_v6_transport::infrastructure::Infrastructure::default();
        let runtime = Arc::new(
            MysqlJobs::new(
                driver.clone(),
                fence,
                META.into(),
                digest,
                Arc::new(facts.clone()),
                IdWire::DecimalString,
                backend.clone(),
                workers,
                None,
                &infra,
            )
            .unwrap(),
        );
        runtime.check_storage().await.unwrap();
        // Pause only the dispatcher. All HTTP, catalog, principal, store and effect queries below are real MySQL.
        infra.relational_jobs = Some(Arc::new(Paused(runtime.clone())));
        let server = spike_v6_transport::server::listen_relational(
            facts,
            IdWire::DecimalString,
            "127.0.0.1:0".parse().unwrap(),
            backend,
            spike_v6_transport::server::ServerOptions::default(),
            infra,
            Some(Arc::new(Auth)),
        )
        .await
        .unwrap();
        let address = server.address;
        let contract = runtime.contract();
        let request = json!({"operation":"incrementJob","input":{"id":"10"},"key":"작업 1","delayMs":0});
        assert_eq!(post(address, "/jobs/enqueue", &contract, None, request.clone()).await["code"], "WIRE.UNAUTHENTICATED");
        assert_eq!(post(address, "/jobs/enqueue", &"0".repeat(64), Some(1), request.clone()).await["code"], "WIRE.CONTRACT_MISMATCH");
        let accepted = post(address, "/jobs/enqueue", &contract, Some(1), request.clone()).await;
        assert_eq!(accepted["ok"], true, "{accepted}");
        let id = accepted["run"]["id"].as_str().unwrap().to_owned();
        assert_eq!(post(address, "/jobs/enqueue", &contract, Some(1), request.clone()).await["run"]["id"], id);
        let mut conflict = request.clone();
        conflict["input"]["id"] = json!("11");
        assert_eq!(post(address, "/jobs/enqueue", &contract, Some(1), conflict).await["code"], "WIRE.IDEMPOTENCY_MISMATCH");
        for path in ["/jobs/status", "/jobs/result", "/jobs/cancel"] {
            assert_eq!(post(address, path, &contract, Some(2), json!({"id":id})).await["code"], "WIRE.NOT_FOUND");
        }
        assert_eq!(post(address, "/jobs/result", &contract, Some(1), json!({"id":id})).await["code"], "WIRE.JOB_NOT_READY");
        let store = runtime.store().unwrap();
        let mut conn = driver.pool().get_conn().await.unwrap();
        let lease = store.claim(&mut conn, "test-dispatcher").await.unwrap().unwrap();
        let mut host = JobHost {
            runtime: runtime.clone(),
            lease: lease.clone(),
            grant: grant(1),
            prefix: String::new(),
            depth: 0,
            budget: Arc::new(AtomicUsize::new(0)),
            capabilities: vec!["apply.Video.increment".into(), "read.Video".into()],
        };
        let effect = json!({"ids":["10"],"key":"increment"});
        let first = host.dispatch("apply", "Video.increment", &effect).await.unwrap();
        assert_eq!(first["changed"], json!(["10"]));
        let read = host.dispatch("read", "Video", &json!({"read":"Video","select":["id","views"]})).await.unwrap();
        assert_eq!(read["rows"][0], json!({"id":"10","views":1}));
        assert_eq!(host.dispatch("apply", "Video.increment", &effect).await.unwrap(), first, "same attempt replays");
        assert_eq!(
            host.dispatch("apply", "Video.increment", &json!({"ids":["11"],"key":"increment"})).await.unwrap_err(),
            "WIRE.IDEMPOTENCY_MISMATCH"
        );
        assert_eq!(host.dispatch("apply", "Video.increment", &json!({"ids":["11"],"key":"denied"})).await.unwrap_err(), "WIRE.FORBIDDEN");
        assert!(store.retry(&mut conn, &lease, "WIRE.WORKER_FAILED").await.unwrap());
        setup.query_drop(format!("UPDATE `{META}`.aip_jobs SET next_run_at=UTC_TIMESTAMP(6)")).await.unwrap();
        let next = store.claim(&mut conn, "test-dispatcher-2").await.unwrap().unwrap();
        assert_eq!(next.epoch, lease.epoch + 1);
        assert_eq!(host.dispatch("apply", "Video.increment", &effect).await.unwrap_err(), "WIRE.LEASE_LOST");
        host.lease = next.clone();
        assert_eq!(host.dispatch("apply", "Video.increment", &effect).await.unwrap(), first, "a recovered attempt replays the committed effect");
        assert_eq!(
            host.dispatch("apply", "Video.increment", &json!({"ids":["11"],"key":"denied"})).await.unwrap_err(),
            "WIRE.FORBIDDEN",
            "business refusal is journaled"
        );
        let changed: Option<i64> = setup.query_first(format!("SELECT views FROM `{SOURCE}`.video WHERE id=10")).await.unwrap();
        assert_eq!(changed, Some(1));
        let denied: Option<i64> = setup.query_first(format!("SELECT views FROM `{SOURCE}`.video WHERE id=11")).await.unwrap();
        assert_eq!(denied, Some(0));
        runtime.settle(&next, Ok(json!({"count":1})), false, None).await.unwrap();
        let status = post(address, "/jobs/status", &contract, Some(1), json!({"id":id})).await;
        assert_eq!(status["run"]["state"], "SUCCEEDED");
        assert!(status["run"]["output"].is_null());
        assert_eq!(post(address, "/jobs/result", &contract, Some(1), json!({"id":id})).await["output"], json!({"count":1}));
        let mut delayed = request;
        delayed["key"] = json!("cancel");
        delayed["delayMs"] = json!(60000);
        let accepted = post(address, "/jobs/enqueue", &contract, Some(1), delayed).await;
        let cancelled = post(address, "/jobs/cancel", &contract, Some(1), json!({"id":accepted["run"]["id"]})).await;
        assert_eq!(cancelled["run"]["state"], "CANCELLED", "{cancelled}");
        setup.query_drop(format!("UPDATE `{META}`.aip_principals SET enabled=0 WHERE actor_id=1")).await.unwrap();
        assert_eq!(post(address, "/jobs/result", &contract, Some(1), json!({"id":id})).await["code"], "WIRE.UNAUTHENTICATED");
        setup.query_drop(format!("UPDATE `{META}`.aip_principals SET enabled=1 WHERE actor_id=1")).await.unwrap();
        setup.query_drop(format!("ALTER TABLE `{SOURCE}`.video ADD COLUMN drift_probe INT")).await.unwrap();
        assert_eq!(post(address, "/jobs/status", &contract, Some(1), json!({"id":id})).await["code"], "WIRE.DEPLOYMENT_CHANGED");
        setup.query_drop(format!("ALTER TABLE `{META}`.aip_effects DROP PRIMARY KEY")).await.unwrap();
        assert!(runtime.check_storage().await.is_err(), "an unkeyed effect journal cannot serve jobs");
        setup.query_drop(format!("DROP TABLE `{META}`.aip_effects")).await.unwrap();
        assert!(runtime.check_storage().await.is_err(), "an old deployment must provision its effect journal first");
        server.shutdown().await.unwrap();
        drop(conn);
        setup.query_drop("DROP USER 'aip_t_jobs_rt'@'%'").await.unwrap();
        for schema in [META, SOURCE] {
            setup.query_drop(format!("DROP DATABASE `{schema}`")).await.unwrap();
        }
    }

    const TRIGGER_META: &str = "aip_t_job_trig";
    const TRIGGER_SOURCE: &str = "aip_t_job_trig_src";
    const TRIGGER_DEFINITION: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Video {
 fields { id: Id; owner: Member; views: Int(0..100) default database }
 rows read when true
 transition increment { from views < 100; allow owner = actor; to views = views + 1; notify owner "video.viewed" }
 expose apply increment { target id; bulk maxRows 1 }
 expose read { select id, views; sort id; budget { rows 20; depth 1; deadline 1s; cost 100 } }
}
operation job incrementJob {
 input { id: Video.Id }
 output { count: Int }
 allow actor != null
 access read.Video, Video.increment
 effect managed
 deadline 5s
 implementation "worker.jobView"
}
operation job hook {
 input { body: Text(1..4096) }
 output { count: Int }
 allow actor != null
 access read.Video
 effect managed
 deadline 5s
 implementation "worker.jobView"
}
operation job tick {
 input { note: Text(1..10) }
 output { count: Int }
 allow actor != null
 access read.Video
 effect managed
 deadline 5s
 implementation "worker.jobView"
}
operation job onView {
 input { event: Text(1..4096) }
 output { count: Int }
 allow actor != null
 access read.Video
 effect managed
 deadline 5s
 implementation "worker.jobView"
}
"#;

    /// Delegates the trigger path as well, so the adapter is exercised through the same trait the transport calls.
    struct Delegating(Arc<MysqlJobs>);
    impl RelationalJobs for Delegating {
        fn handle<'a>(&'a self, path: &'a str, body: &'a Value, grant: &'a DurablePrincipal) -> BackendFuture<'a, Value> {
            self.0.handle(path, body, grant)
        }
        fn supervise(self: Arc<Self>) -> BackendFuture<'static, ()> {
            Box::pin(std::future::pending())
        }
        fn submit_service<'a>(
            &'a self,
            principal: &'a spike_v6_transport::triggers::ServicePrincipal,
            operation: &'a str,
            input: &'a Value,
            key: &'a str,
        ) -> BackendFuture<'a, Result<aip_execution::Run>> {
            self.0.submit_service(principal, operation, input, key)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "requires a run-owned MySQL 8.4 fixture; trigger submission, schedule slots and outbox relay, no worker subprocess"]
    async fn mysql_triggers_run_as_the_service_principal_and_converge_on_server_minted_keys() {
        use spike_v6_transport::triggers::{OutboxHandler, Schedule, ServicePrincipal, Signature, Triggers, Webhook};
        let (meta, source) = (TRIGGER_META, TRIGGER_SOURCE);
        let url = std::env::var("AIP_TEST_MYSQL_URL").unwrap();
        aip_test_support::validate_mysql_admin_url(&url).unwrap();
        let admin = MysqlDriver::connect(&url).await.unwrap();
        let mut setup = admin.pool().get_conn().await.unwrap();
        for schema in [meta, source] {
            setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
        }
        setup.query_drop(format!("CREATE DATABASE `{source}`")).await.unwrap();
        setup.query_drop(format!("CREATE TABLE `{source}`.member(id BIGINT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
        setup.query_drop(format!("CREATE TABLE `{source}`.video(id BIGINT PRIMARY KEY,owner_id BIGINT NOT NULL,views BIGINT NOT NULL DEFAULT 0,FOREIGN KEY(owner_id) REFERENCES `{source}`.member(id)) ENGINE=InnoDB")).await.unwrap();
        setup.query_drop(format!("INSERT INTO `{source}`.member VALUES(1),(2)")).await.unwrap();
        setup.query_drop(format!("INSERT INTO `{source}`.video VALUES(10,1,0),(11,2,0)")).await.unwrap();
        let facts = spike_v1_fixture::load_str(TRIGGER_DEFINITION, spike_v1_fixture::Form::A).unwrap().execution;
        sqlgen::try_set_schema(meta).unwrap();
        let digest = spike_v1_fixture::digest(&facts);
        let fingerprint = admin.ensure_meta_schema_bootstrap(meta, source, &digest).await.unwrap();
        setup.query_drop(format!("CREATE VIEW `{meta}`.member AS SELECT * FROM `{source}`.member")).await.unwrap();
        setup.query_drop(format!("CREATE VIEW `{meta}`.video AS SELECT * FROM `{source}`.video")).await.unwrap();
        setup
            .query_drop(format!("INSERT INTO `{meta}`.aip_principals(issuer,subject,actor_id) VALUES('https://test.example','user-1',1),('https://test.example','svc',2)"))
            .await
            .unwrap();
        setup.query_drop("DROP USER IF EXISTS 'aip_t_trig_rt'@'%'").await.unwrap();
        setup.query_drop("CREATE USER 'aip_t_trig_rt'@'%' IDENTIFIED BY 'trig_test_only'").await.unwrap();
        for schema in [source, meta] {
            setup
                .query_drop(format!("GRANT SELECT,INSERT,UPDATE,DELETE,CREATE TEMPORARY TABLES ON `{schema}`.* TO 'aip_t_trig_rt'@'%'"))
                .await
                .unwrap();
        }
        let endpoint = url.split_once('@').unwrap().1.split('/').next().unwrap();
        let driver = MysqlDriver::from_url(&format!("mysql://aip_t_trig_rt:trig_test_only@{endpoint}/{source}")).unwrap();
        let fence = MysqlDeploymentFence::from_fingerprint(meta, source, &digest, &fingerprint).unwrap();
        let backend = Arc::new(MysqlBackend::new(driver.clone(), fence.clone()));
        let dir = Dir(std::env::temp_dir().join(format!("aip-jobs-trigger-unit-{}", std::process::id())));
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(dir.0.join("worker.py"), "async def jobView(input, ctx):\n return {'count': 1}\n").unwrap();
        let workers = ReadExtensions { lang: spike_v6_transport::server::WorkerLang::Python, dir: dir.0.clone() };
        let service = ServicePrincipal { issuer: "https://test.example".into(), subject: "svc".into() };
        let mut triggers = Triggers::default();
        triggers.schedules.push(Schedule {
            name: "tick".into(),
            operation: "tick".into(),
            cron: spike_v6_transport::schedule::Cron::parse("* * * * *", "UTC").unwrap(),
            input: json!({"note":"x"}),
            principal: service.clone(),
            misfire_grace: Duration::from_secs(60),
        });
        triggers.outbox.push(OutboxHandler { topic: "video.viewed".into(), operation: "onView".into(), principal: service.clone() });
        triggers.webhooks.push(Webhook {
            name: "hook".into(),
            path: "/hooks/hook".into(),
            operation: "hook".into(),
            principal: service.clone(),
            secret: b"test-webhook-secret-0123456789".to_vec(),
            signature: Signature::HmacSha256 { header: "x-signature".into(), prefix: String::new(), base64: false },
            max_body_bytes: 4096,
        });
        triggers.validate(&facts, IdWire::DecimalString, &["/jobs"]).unwrap();
        let infra = spike_v6_transport::infrastructure::Infrastructure { triggers: Arc::new(triggers), ..Default::default() };
        let runtime = Arc::new(
            MysqlJobs::new(
                driver.clone(),
                fence,
                meta.into(),
                digest,
                Arc::new(facts.clone()),
                IdWire::DecimalString,
                backend.clone(),
                workers,
                None,
                &infra,
            )
            .unwrap(),
        );
        runtime.check_storage().await.unwrap();
        let jobs: Arc<dyn RelationalJobs> = Arc::new(Delegating(runtime.clone()));
        let scalar = |sql: String| {
            let driver = driver.clone();
            async move { driver.pool().get_conn().await.unwrap().query_first::<i64, _>(sql).await.unwrap().unwrap() }
        };
        let table = |name: &str| format!("`{meta}`.`{name}`");

        // A webhook delivery key accepts one job per delivery, as the configured service principal.
        let first = jobs.submit_service(&service, "hook", &json!({"body":"payload"}), "webhook:hook:d1").await.unwrap();
        assert_eq!(first.state, "QUEUED");
        let again = jobs.submit_service(&service, "hook", &json!({"body":"payload"}), "webhook:hook:d1").await.unwrap();
        assert_eq!(again.id, first.id, "a redelivery converges on one run");
        assert_eq!(
            jobs.submit_service(&service, "hook", &json!({"body":"other"}), "webhook:hook:d1").await.unwrap_err(),
            "WIRE.IDEMPOTENCY_MISMATCH"
        );
        assert!(
            jobs.submit_service(&service, "hook", &json!({"body":""}), "webhook:hook:d2").await.is_err(),
            "the operation's input contract still applies"
        );
        assert_eq!(scalar(format!("SELECT COUNT(*) FROM {} WHERE idempotency_key LIKE 'webhook:%'", table("aip_jobs"))).await, 1);

        // A user may neither claim a server-minted key nor enqueue an operation a trigger feeds with verified input.
        let user = grant(1);
        let enqueue = |operation: &str, key: &str, input: Value| {
            let body = json!({"operation":operation,"input":input,"key":key,"delayMs":0});
            let runtime = runtime.clone();
            let user = user.clone();
            async move { runtime.handle("/jobs/enqueue", &body, &user).await }
        };
        assert_eq!(enqueue("hook", "forged", json!({"body":"x"})).await["code"], "WIRE.ACCESS_DENIED");
        assert_eq!(enqueue("tick", "forged", json!({"note":"x"})).await["code"], "WIRE.ACCESS_DENIED");
        assert_eq!(enqueue("onView", "forged", json!({"event":"{}"})).await["code"], "WIRE.ACCESS_DENIED");
        assert_eq!(enqueue("incrementJob", "webhook:hook:d3", json!({"id":"10"})).await["code"], "WIRE.BAD_VALUE");
        assert_eq!(enqueue("incrementJob", "outbox:1:1", json!({"id":"10"})).await["code"], "WIRE.BAD_VALUE");
        assert_eq!(enqueue("incrementJob", "schedule:tick:1", json!({"id":"10"})).await["code"], "WIRE.BAD_VALUE");
        assert_eq!(enqueue("incrementJob", "ordinary", json!({"id":"10"})).await["ok"], true);

        // An unbound or disabled service principal accepts nothing, and takes effect without a restart.
        let stranger = ServicePrincipal { issuer: "https://test.example".into(), subject: "nobody".into() };
        assert_eq!(jobs.submit_service(&stranger, "hook", &json!({"body":"x"}), "webhook:hook:d4").await.unwrap_err(), "WIRE.PRINCIPAL_UNAVAILABLE");
        setup.query_drop(format!("UPDATE {} SET enabled=0 WHERE subject='svc'", table("aip_principals"))).await.unwrap();
        assert_eq!(jobs.submit_service(&service, "hook", &json!({"body":"x"}), "webhook:hook:d4").await.unwrap_err(), "WIRE.PRINCIPAL_UNAVAILABLE");
        setup.query_drop(format!("UPDATE {} SET enabled=1 WHERE subject='svc'", table("aip_principals"))).await.unwrap();
        assert!(jobs.submit_service(&service, "hook", &json!({"body":"x"}), "webhook:hook:d4").await.is_ok());

        // One run per schedule slot, however often the slot is evaluated. Retry when a minute boundary falls inside the check.
        let schedule = &infra.triggers.schedules[0];
        loop {
            let slot = schedule.due_slot();
            runtime.fire_schedules().await;
            runtime.fire_schedules().await;
            if let Some(slot) = slot.filter(|slot| Some(*slot) == schedule.due_slot()) {
                let key = spike_v6_transport::triggers::schedule_key("tick", slot);
                assert_eq!(scalar(format!("SELECT COUNT(*) FROM {} WHERE idempotency_key='{key}'", table("aip_jobs"))).await, 1);
                break;
            }
        }

        // Outbox rows become one job each and one receipt each, even with two relays racing. Other topics stay untouched.
        for topic in ["video.viewed", "video.viewed", "video.viewed", "other.topic"] {
            setup
                .query_drop(format!("INSERT INTO {}(topic,recipient_id,source,source_id) VALUES('{topic}','1','Video','10')", table("aip_outbox")))
                .await
                .unwrap();
        }
        let (a, b) = tokio::join!(runtime.relay_outbox(), runtime.relay_outbox());
        a.unwrap();
        b.unwrap();
        assert_eq!(scalar(format!("SELECT COUNT(*) FROM {} WHERE idempotency_key LIKE 'outbox:%'", table("aip_jobs"))).await, 3);
        assert_eq!(scalar(format!("SELECT COUNT(*) FROM {}", table("aip_outbox_dispatch"))).await, 3);
        assert_eq!(
            scalar(format!(
                "SELECT COUNT(*) FROM {} d JOIN {} j ON j.id=d.job_id WHERE j.operation='onView' AND j.owner=(SELECT owner FROM {} WHERE operation='hook' LIMIT 1)",
                table("aip_outbox_dispatch"),
                table("aip_jobs"),
                table("aip_jobs")
            ))
            .await,
            3,
            "each receipt names the job it created, owned by the service principal"
        );
        runtime.relay_outbox().await.unwrap();
        assert_eq!(
            scalar(format!("SELECT COUNT(*) FROM {} WHERE idempotency_key LIKE 'outbox:%'", table("aip_jobs"))).await,
            3,
            "a relayed row is not relayed again"
        );
        let input: String = setup
            .query_first(format!("SELECT CAST(input AS CHAR) FROM {} WHERE operation='onView' LIMIT 1", table("aip_jobs")))
            .await
            .unwrap()
            .unwrap();
        let event: Value = serde_json::from_str(serde_json::from_str::<Value>(&input).unwrap()["event"].as_str().unwrap()).unwrap();
        assert_eq!(
            (event["topic"].as_str(), event["source"].as_str(), event["recipientId"].as_str()),
            (Some("video.viewed"), Some("Video"), Some("1"))
        );

        // A rejected row keeps its receipt out: nothing is marked dispatched without its job.
        setup.query_drop(format!("UPDATE {} SET enabled=0 WHERE subject='svc'", table("aip_principals"))).await.unwrap();
        setup
            .query_drop(format!("INSERT INTO {}(topic,recipient_id,source,source_id) VALUES('video.viewed','1','Video','11')", table("aip_outbox")))
            .await
            .unwrap();
        runtime.relay_outbox().await.unwrap();
        assert_eq!(scalar(format!("SELECT COUNT(*) FROM {}", table("aip_outbox_dispatch"))).await, 3);
        setup.query_drop(format!("UPDATE {} SET enabled=1 WHERE subject='svc'", table("aip_principals"))).await.unwrap();
        runtime.relay_outbox().await.unwrap();
        assert_eq!(
            scalar(format!("SELECT COUNT(*) FROM {}", table("aip_outbox_dispatch"))).await,
            4,
            "the row is delivered once the principal is back"
        );

        setup.query_drop("DROP USER 'aip_t_trig_rt'@'%'").await.unwrap();
        for schema in [meta, source] {
            setup.query_drop(format!("DROP DATABASE `{schema}`")).await.unwrap();
        }
    }

    const HTTP_META: &str = "aip_t_job_extcall";
    const HTTP_SOURCE: &str = "aip_t_job_extcall_src";
    const HTTP_DEFINITION: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Video {
 fields { id: Id; owner: Member; views: Int(0..100) default database }
 rows read when true
 transition increment { from views < 100; allow owner = actor; to views = views + 1 }
 expose apply increment { target id; bulk maxRows 1 }
 expose read { select id, views; sort id; budget { rows 20; depth 1; deadline 1s; cost 100 } }
}
operation job notifyJob {
 input { id: Video.Id }
 output { count: Int }
 allow actor != null
 access http.mail, http.dead
 effect managed
 deadline 5s
 implementation "worker.jobView"
}
"#;

    /// A loopback HTTP peer that counts requests and either answers or hangs up after reading the request.
    async fn peer(answer: Arc<std::sync::atomic::AtomicBool>, requests: Arc<AtomicUsize>) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let (answer, requests) = (answer.clone(), requests.clone());
                tokio::spawn(async move {
                    let mut received = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        let Ok(read) = socket.read(&mut chunk).await else { return };
                        if read == 0 {
                            return;
                        }
                        received.extend_from_slice(&chunk[..read]);
                        let text = String::from_utf8_lossy(&received).to_string();
                        if let Some((head, body)) = text.split_once("\r\n\r\n") {
                            let length = head
                                .lines()
                                .find_map(|line| {
                                    line.to_ascii_lowercase().strip_prefix("content-length:").map(|value| value.trim().parse::<usize>().unwrap_or(0))
                                })
                                .unwrap_or(0);
                            if body.len() >= length {
                                break;
                            }
                        }
                    }
                    requests.fetch_add(1, Ordering::SeqCst);
                    if answer.load(Ordering::SeqCst) {
                        let body = r#"{"queued":true}"#;
                        let _ = socket
                            .write_all(
                                format!(
                                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                                    body.len()
                                )
                                .as_bytes(),
                            )
                            .await;
                    }
                    // Without an answer the connection just closes: the request left, the outcome is unknown.
                });
            }
        });
        port
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "requires a run-owned MySQL 8.4 fixture; external HTTP effects through the job host, no worker subprocess"]
    async fn mysql_http_effects_are_journaled_and_an_unknown_outcome_is_never_repeated() {
        let (meta, source) = (HTTP_META, HTTP_SOURCE);
        let url = std::env::var("AIP_TEST_MYSQL_URL").unwrap();
        aip_test_support::validate_mysql_admin_url(&url).unwrap();
        let admin = MysqlDriver::connect(&url).await.unwrap();
        let mut setup = admin.pool().get_conn().await.unwrap();
        for schema in [meta, source] {
            setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
        }
        setup.query_drop(format!("CREATE DATABASE `{source}`")).await.unwrap();
        setup.query_drop(format!("CREATE TABLE `{source}`.member(id BIGINT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
        setup.query_drop(format!("CREATE TABLE `{source}`.video(id BIGINT PRIMARY KEY,owner_id BIGINT NOT NULL,views BIGINT NOT NULL DEFAULT 0,FOREIGN KEY(owner_id) REFERENCES `{source}`.member(id)) ENGINE=InnoDB")).await.unwrap();
        setup.query_drop(format!("INSERT INTO `{source}`.member VALUES(1)")).await.unwrap();
        setup.query_drop(format!("INSERT INTO `{source}`.video VALUES(10,1,0)")).await.unwrap();
        let facts = spike_v1_fixture::load_str(HTTP_DEFINITION, spike_v1_fixture::Form::A).unwrap().execution;
        sqlgen::try_set_schema(meta).unwrap();
        let digest = spike_v1_fixture::digest(&facts);
        let fingerprint = admin.ensure_meta_schema_bootstrap(meta, source, &digest).await.unwrap();
        setup.query_drop(format!("CREATE VIEW `{meta}`.member AS SELECT * FROM `{source}`.member")).await.unwrap();
        setup.query_drop(format!("CREATE VIEW `{meta}`.video AS SELECT * FROM `{source}`.video")).await.unwrap();
        setup
            .query_drop(format!("INSERT INTO `{meta}`.aip_principals(issuer,subject,actor_id) VALUES('https://test.example','user-1',1)"))
            .await
            .unwrap();
        setup.query_drop("DROP USER IF EXISTS 'aip_t_ext_rt'@'%'").await.unwrap();
        setup.query_drop("CREATE USER 'aip_t_ext_rt'@'%' IDENTIFIED BY 'ext_test_only'").await.unwrap();
        for schema in [source, meta] {
            setup
                .query_drop(format!("GRANT SELECT,INSERT,UPDATE,DELETE,CREATE TEMPORARY TABLES ON `{schema}`.* TO 'aip_t_ext_rt'@'%'"))
                .await
                .unwrap();
        }
        let endpoint = url.split_once('@').unwrap().1.split('/').next().unwrap();
        let driver = MysqlDriver::from_url(&format!("mysql://aip_t_ext_rt:ext_test_only@{endpoint}/{source}")).unwrap();
        let fence = MysqlDeploymentFence::from_fingerprint(meta, source, &digest, &fingerprint).unwrap();
        let backend = Arc::new(MysqlBackend::new(driver.clone(), fence.clone()));
        let dir = Dir(std::env::temp_dir().join(format!("aip-jobs-extcall-unit-{}", std::process::id())));
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(dir.0.join("worker.py"), "async def jobView(input, ctx):\n return {'count': 1}\n").unwrap();
        let workers = ReadExtensions { lang: spike_v6_transport::server::WorkerLang::Python, dir: dir.0.clone() };

        let (answer, requests) = (Arc::new(std::sync::atomic::AtomicBool::new(true)), Arc::new(AtomicUsize::new(0)));
        let port = peer(answer.clone(), requests.clone()).await;
        let integration = |name: &str, port: u16| {
            serde_json::from_value::<crate::IntegrationConfig>(json!({"name":name,"url":format!("http://127.0.0.1:{port}/send"),"method":"POST",
                "timeout_ms":2000,"max_request_bytes":1024,"max_response_bytes":1024,"allow_loopback":true}))
            .unwrap()
        };
        let configs = [integration("mail", port), integration("dead", 1)];
        let broker = crate::build_broker_with_webhooks(&configs, &[]).await.unwrap();
        let infra = spike_v6_transport::infrastructure::Infrastructure::default();
        // A declared integration that is not configured is refused before the server starts.
        assert!(
            MysqlJobs::new(
                driver.clone(),
                fence.clone(),
                meta.into(),
                digest.clone(),
                Arc::new(facts.clone()),
                IdWire::DecimalString,
                backend.clone(),
                workers.clone(),
                None,
                &infra
            )
            .is_err()
        );
        let runtime = Arc::new(
            MysqlJobs::new(
                driver.clone(),
                fence,
                meta.into(),
                digest,
                Arc::new(facts.clone()),
                IdWire::DecimalString,
                backend,
                workers,
                broker,
                &infra,
            )
            .unwrap(),
        );
        runtime.check_storage().await.unwrap();

        let accepted =
            runtime.handle("/jobs/enqueue", &json!({"operation":"notifyJob","input":{"id":"10"},"key":"mail job","delayMs":0}), &grant(1)).await;
        assert_eq!(accepted["ok"], true, "{accepted}");
        let store = runtime.store().unwrap();
        let mut conn = driver.pool().get_conn().await.unwrap();
        let lease = store.claim(&mut conn, "test-dispatcher").await.unwrap().unwrap();
        let mut host = JobHost {
            runtime: runtime.clone(),
            lease: lease.clone(),
            grant: grant(1),
            prefix: String::new(),
            depth: 0,
            budget: Arc::new(AtomicUsize::new(0)),
            capabilities: vec!["http.mail".into(), "http.dead".into()],
        };
        let effects = |state: &str| format!("SELECT COUNT(*) FROM `{meta}`.`aip_effects` WHERE state='{state}'");
        let count = |sql: String| {
            let driver = driver.clone();
            async move { driver.pool().get_conn().await.unwrap().query_first::<i64, _>(sql).await.unwrap().unwrap() }
        };

        // A completed call is journaled with its answer and replayed, never sent twice.
        let call = json!({"key":"welcome","input":{"to":"member@example.test"}});
        let first = host.dispatch("http", "mail", &call).await.unwrap();
        assert_eq!((first["status"].clone(), first["body"].clone()), (json!(200), json!({"queued":true})), "{first}");
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(host.dispatch("http", "mail", &call).await.unwrap(), first, "the same effect replays its recorded answer");
        assert_eq!(requests.load(Ordering::SeqCst), 1, "a replay must not leave the process again");
        assert_eq!(
            host.dispatch("http", "mail", &json!({"key":"welcome","input":{"to":"someone-else@example.test"}})).await.unwrap_err(),
            "WIRE.IDEMPOTENCY_MISMATCH"
        );
        assert_eq!(count(effects("SUCCEEDED")).await, 1);

        // Declared capabilities only, and a key is required.
        assert_eq!(host.dispatch("http", "billing", &call).await.unwrap_err(), "WIRE.ACCESS_NOT_DECLARED");
        assert_eq!(host.dispatch("http", "mail", &json!({"input":{}})).await.unwrap_err(), "WIRE.BAD_REQUEST");

        // The request certainly never left: the intent is dropped so the same key may be sent again.
        assert_eq!(host.dispatch("http", "dead", &call).await.unwrap_err(), "WIRE.DELIVERY_FAILED");
        assert_eq!(
            count(format!("SELECT COUNT(*) FROM `{meta}`.`aip_effects` WHERE state='PENDING'")).await,
            0,
            "a certain not-sent leaves no intent behind"
        );

        // The request left and the answer was lost: the outcome is unknown, recorded as such, and never sent a second time.
        answer.store(false, Ordering::SeqCst);
        let lost = json!({"key":"receipt","input":{"to":"member@example.test"}});
        assert_eq!(host.dispatch("http", "mail", &lost).await.unwrap_err(), "WIRE.EFFECT_UNKNOWN");
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert_eq!(count(effects("UNKNOWN")).await, 1);
        answer.store(true, Ordering::SeqCst);
        assert_eq!(host.dispatch("http", "mail", &lost).await.unwrap_err(), "WIRE.EFFECT_UNKNOWN", "an unknown effect is never repeated");
        assert_eq!(requests.load(Ordering::SeqCst), 2, "the unknown effect must not be sent again");

        // The lease is checked when the answer is recorded: a recovered attempt cannot be written by the old one.
        setup.query_drop(format!("UPDATE `{meta}`.aip_jobs SET lease_epoch = lease_epoch + 1")).await.unwrap();
        assert_eq!(host.dispatch("http", "mail", &json!({"key":"after","input":{}})).await.unwrap_err(), "WIRE.LEASE_LOST");
        assert_eq!(requests.load(Ordering::SeqCst), 2, "a lost lease is detected before anything is sent");

        drop(conn);
        setup.query_drop("DROP USER 'aip_t_ext_rt'@'%'").await.unwrap();
        for schema in [meta, source] {
            setup.query_drop(format!("DROP DATABASE `{schema}`")).await.unwrap();
        }
    }
}

#[cfg(test)]
mod mysql_job_stores_tests;
