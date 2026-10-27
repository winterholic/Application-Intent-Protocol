use crate::{auth::DurablePrincipal, server::Runtime};
use aip_execution::{Lease, NewRun, Store};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::{json, Value};
use spike_v2_read::{
    id_wire::IdWire,
    plan::{Caller, Reject},
};
use std::{sync::Arc, time::Duration};
use tokio_postgres::{types::ToSql, Client, GenericClient};

type Result<T> = std::result::Result<T, &'static str>;
/// Schedules are evaluated at least this often; replays of an already enqueued slot are cheap.
const SCHEDULE_TICK: Duration = Duration::from_secs(5);
const OUTBOX_TICK: Duration = Duration::from_secs(1);
const MESSAGES_PER_TICK: usize = 50;
const MESSAGE_LEASE_MS: i64 = 30_000;
/// How long consecutive heartbeats may fail before a running job is stopped. Well inside the 30s lease.
const HEARTBEAT_OUTAGE_TOLERANCE: Duration = Duration::from_secs(12);

fn db_error<T>(_: T) -> &'static str {
    "WIRE.DB_UNAVAILABLE"
}
/// 쓰기 요청(job 접수·파일·메시지)의 COMMIT 한 번. 이 요청의 결과는 호출자에게 그대로 간다.
/// COMMIT이 나가지 못했으면 커밋된 것이 없어 DB_UNAVAILABLE이다. 보낸 뒤의 연결 끊김·해석 불가 응답은 COMMIT_UNKNOWN이다.
/// 서버가 SQLSTATE와 함께 거절한 COMMIT도 기존대로 COMMIT_UNKNOWN으로 낸다(보수적. 넓히지도 좁히지도 않았다).
async fn commit_request(tx: tokio_postgres::Transaction<'_>) -> Result<()> {
    use spike_v3_write::CommitFailure;
    match spike_v3_write::send_commit(tx, || ()).await {
        Ok(()) => Ok(()),
        Err(CommitFailure::NotSent) => Err("WIRE.DB_UNAVAILABLE"),
        Err(CommitFailure::Unknown | CommitFailure::Rejected(_)) => Err("WIRE.COMMIT_UNKNOWN"),
    }
}
/// managed effect(데이터 apply·파일·이벤트·메시지)가 journal과 함께 커밋하는 COMMIT 한 번.
/// COMMIT을 보낸 뒤 연결이 끊기거나 응답을 해석할 수 없으면 쓰기가 커밋됐을 수 있다. 확장 오류로 뭉개지 않고 COMMIT_UNKNOWN으로 낸다.
/// COMMIT이 나가지 못했거나 서버가 거절한 경우는 기존대로 DB_UNAVAILABLE이다.
async fn commit_effect(tx: tokio_postgres::Transaction<'_>) -> Result<()> {
    use spike_v3_write::CommitFailure;
    match spike_v3_write::send_commit(tx, || ()).await {
        Ok(()) => Ok(()),
        Err(CommitFailure::Unknown) => Err("WIRE.COMMIT_UNKNOWN"),
        Err(CommitFailure::NotSent | CommitFailure::Rejected(_)) => Err("WIRE.DB_UNAVAILABLE"),
    }
}
fn step_error(code: &str) -> &'static str {
    match code {
        "WIRE.ACCESS_NOT_DECLARED" => "WIRE.ACCESS_NOT_DECLARED",
        "WIRE.COST_EXCEEDED" => "WIRE.COST_EXCEEDED",
        "WIRE.LEASE_LOST" => "WIRE.LEASE_LOST",
        "WIRE.EFFECT_UNKNOWN" => "WIRE.EFFECT_UNKNOWN",
        "WIRE.BAD_VALUE" => "WIRE.BAD_VALUE",
        _ => "WIRE.STEP_FAILED",
    }
}
/// Business outcomes of data capabilities stay distinguishable so an extension can branch or compensate.
pub(crate) fn data_error(code: &str) -> &'static str {
    match code {
        "WIRE.ACCESS_DENIED" => "WIRE.ACCESS_DENIED",
        "WIRE.NOT_FOUND" => "WIRE.NOT_FOUND",
        "WIRE.INVALID_STATE" => "WIRE.INVALID_STATE",
        "WIRE.CHECK_FAILED" => "WIRE.CHECK_FAILED",
        "WIRE.DUPLICATE_TARGET" => "WIRE.DUPLICATE_TARGET",
        "WIRE.CONFLICT" => "WIRE.CONFLICT",
        "WIRE.ALREADY_EXISTS" => "WIRE.ALREADY_EXISTS",
        "WIRE.INVARIANT_VIOLATED" => "WIRE.INVARIANT_VIOLATED",
        "WIRE.DEADLINE_EXCEEDED" => "WIRE.DEADLINE_EXCEEDED",
        "WIRE.NOT_EXPOSED" => "WIRE.NOT_EXPOSED",
        "WIRE.OUTPUT_TOO_LARGE" => "WIRE.OUTPUT_TOO_LARGE",
        "WIRE.FORBIDDEN" => "WIRE.FORBIDDEN",
        "WIRE.MISSING_TARGET" => "WIRE.MISSING_TARGET",
        "WIRE.TARGET_NOT_ALLOWED" => "WIRE.TARGET_NOT_ALLOWED",
        "WIRE.EFFECT_TARGET_MISSING" => "WIRE.EFFECT_TARGET_MISSING",
        "WIRE.NOT_SAME_SCOPE" => "WIRE.NOT_SAME_SCOPE",
        "WIRE.BULK_LIMIT" => "WIRE.BULK_LIMIT",
        "WIRE.ROWS_EXCEEDED" => "WIRE.ROWS_EXCEEDED",
        "WIRE.DUPLICATE" => "WIRE.DUPLICATE",
        "WIRE.BAD_REQUEST" => "WIRE.BAD_REQUEST",
        code => step_error(code),
    }
}
fn store_error(e: aip_execution::Error) -> &'static str {
    match e {
        aip_execution::Error::IdempotencyConflict => "WIRE.IDEMPOTENCY_MISMATCH",
        aip_execution::Error::InvalidRun => "WIRE.BAD_VALUE",
        // The input is too large once serialised; no retry will change that, so it must not look like a database outage.
        aip_execution::Error::PayloadTooLarge => "WIRE.PAYLOAD_TOO_LARGE",
        _ => "WIRE.DB_UNAVAILABLE",
    }
}
pub(crate) fn enabled(facts: &Value) -> bool {
    facts["operations"].as_object().is_some_and(|ops| ops.values().any(|op| op["kind"] == "job"))
}
fn declares_message_access(facts: &Value) -> bool {
    facts["operations"].as_object().is_some_and(|ops| {
        ops.values().any(|operation| {
            operation["kind"] == "job"
                && operation["access"].as_object().is_some_and(|access| {
                    access.iter().any(|(capability, enabled)| capability.starts_with("messages.") && enabled.as_bool() == Some(true))
                })
        })
    })
}
fn should_cleanup_messages(messaging_enabled: bool, facts: &Value) -> bool {
    messaging_enabled && declares_message_access(facts)
}
fn policy(rt: &Runtime, operation: &str) -> crate::triggers::JobPolicy {
    rt.infrastructure.job_policies.get(operation).copied().unwrap_or_default()
}

/// Infrastructure failures always retry. Extension failures retry only when the extension asked for it
/// (`WIRE.RETRY_LATER`) or the operator opted the job into retrying uncaught errors. Contract, permission,
/// cost and uncertain-effect errors never retry because another attempt would not change them.
fn retryable(rt: &Runtime, operation: &str, code: &str) -> bool {
    matches!(code, "WIRE.WORKER_FAILED" | "WIRE.DB_UNAVAILABLE" | "WIRE.RETRY_LATER")
        || (code == "WIRE.EXTENSION_ERROR" && policy(rt, operation).retry_uncaught)
}

fn schema(rt: &Runtime) -> Result<&str> {
    rt.deployment.as_ref().map(|f| f.schema.as_str()).ok_or("WIRE.NOT_AVAILABLE")
}
fn owner(g: &DurablePrincipal) -> String {
    // The revocation floor is deliberately left out: after revoke and re-bind the same person owns the same files and jobs.
    // The fixed 0 keeps the digest of a never-revoked principal, which is what existing stored data used.
    spike_v1_fixture::digest(&json!([g.issuer, g.subject, g.actor_id, 0]))
}
async fn connect(rt: &Runtime) -> Result<crate::pool::PooledConnection> {
    rt.db_pool.as_ref().ok_or("WIRE.DB_UNAVAILABLE")?.acquire().await.map_err(db_error)
}

pub(crate) fn bundle(rt: &Runtime) -> Result<String> {
    let config = rt.extensions.as_ref().ok_or("WIRE.NOT_AVAILABLE")?;
    let mut paths = std::fs::read_dir(&config.dir)
        .map_err(db_error)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(db_error)?;
    paths.sort();
    let mut files = vec![];
    let mut bytes = 0;
    for path in paths {
        if !path.extension().is_some_and(|e| e == "mjs" || e == "py") {
            continue;
        }
        let path = path.canonicalize().map_err(db_error)?;
        if !path.starts_with(&config.dir) {
            return Err("WIRE.IMPLEMENTATION_CHANGED");
        }
        let content = std::fs::read(&path).map_err(db_error)?;
        bytes += content.len();
        if bytes > 32 << 20 {
            return Err("WIRE.COST_EXCEEDED");
        }
        files.push(json!([path.file_name().and_then(|n| n.to_str()).ok_or("WIRE.BAD_CONFIGURATION")?, spike_v1_fixture::digest(&json!(content))]));
    }
    Ok(spike_v1_fixture::digest(&json!([
        "aip-managed-execution/1",
        format!("{:?}", config.lang),
        spike_v4_worker::worker_abi_digest(),
        files,
        rt.broker.as_ref().map(|b| b.fingerprint()),
        format!("{:?}", rt.infrastructure.worker_launch.isolation),
        rt.infrastructure.worker_launch.runtime_executable,
        rt.infrastructure.worker_launch.read_only_paths,
        rt.infrastructure.worker_launch.environment.iter().collect::<std::collections::BTreeMap<_, _>>(),
        rt.infrastructure.worker_launch.cgroup_root,
        rt.infrastructure.worker_launch.resource_limits.map(|limits| json!({
            "memoryBytes": limits.memory_bytes,
            "pidsMax": limits.pids_max,
            "cpuQuotaMicros": limits.cpu_quota_micros,
            "cpuPeriodMicros": limits.cpu_period_micros
        }))
    ])))
}

async fn guard(db: &impl GenericClient, rt: &Runtime, grant: &DurablePrincipal) -> Result<()> {
    let fence = rt.deployment.as_ref().ok_or("WIRE.NOT_AVAILABLE")?;
    match tokio::time::timeout(crate::server::DB_PREFLIGHT_TIMEOUT, rt.authenticator.as_ref().ok_or("WIRE.UNAUTHENTICATED")?.verify_durable(grant))
        .await
    {
        Ok(Ok(())) => {}
        Ok(Err(crate::auth::AuthError::Unavailable)) | Err(_) => return Err("WIRE.DB_UNAVAILABLE"),
        _ => return Err("WIRE.UNAUTHENTICATED"),
    }
    db.batch_execute("SET LOCAL lock_timeout='2s'; SET LOCAL statement_timeout='5s'").await.map_err(db_error)?;
    db.query_one("SELECT pg_advisory_xact_lock_shared(1095323725,hashtext($1))", &[&fence.schema]).await.map_err(db_error)?;
    let marker = db
        .query_opt(&format!("SELECT facts_digest,facts FROM {}.aip_migrate_meta WHERE singleton=true FOR SHARE", fence.schema), &[])
        .await
        .map_err(db_error)?;
    if marker.as_ref().is_none_or(|r| r.get::<_, String>(0) != fence.expected_digest) {
        return Err("WIRE.DEPLOYMENT_CHANGED");
    }
    let facts: Value = marker.unwrap().get(1);
    if !spike_v2_read::external::matches(db, &facts).await.map_err(db_error)? {
        return Err("WIRE.DEPLOYMENT_CHANGED");
    }
    let row = db
        .query_opt(
            &format!("SELECT actor_id,enabled,min_iat FROM {}.aip_principals WHERE issuer=$1 AND subject=$2 FOR SHARE", fence.schema),
            &[&grant.issuer, &grant.subject],
        )
        .await
        .map_err(db_error)?;
    if !row.is_some_and(|r| {
        r.get::<_, i64>(0) == grant.actor_id && r.get::<_, bool>(1) && r.get::<_, i64>(2) == grant.min_iat && grant.issued_at >= grant.min_iat
    }) {
        return Err("WIRE.UNAUTHENTICATED");
    }
    Ok(())
}
async fn authorize(db: &impl GenericClient, facts: &Value, name: &str, input: &Value, actor: i64, wire: IdWire) -> Result<Caller> {
    let now = db.query_one("SELECT clock_timestamp()::text", &[]).await.map_err(db_error)?.get(0);
    let caller = Caller { actor_id: Some(actor), now };
    let plan = spike_v2_read::plan::plan_operation_authorization(facts, name, input, &caller, wire).map_err(|e| e.code)?;
    let params: Vec<&(dyn ToSql + Sync)> = plan.params.iter().map(|p| p as _).collect();
    if !db.query_one(&plan.sql, &params).await.map_err(db_error)?.get::<_, bool>(0) {
        return Err("WIRE.ACCESS_DENIED");
    }
    Ok(caller)
}
async fn locked_lease(db: &impl GenericClient, rt: &Runtime, lease: &Lease) -> Result<()> {
    let s = schema(rt)?;
    db.query_opt(&format!("SELECT id FROM {s}.aip_jobs WHERE id=$1 AND lease_epoch=$2 AND lease_owner=$3 AND state='RUNNING' AND lease_until>clock_timestamp() AND attempt_deadline>clock_timestamp() FOR UPDATE"),&[&lease.run_id,&lease.epoch,&lease.worker]).await.map_err(db_error)?.ok_or("WIRE.LEASE_LOST")?;
    Ok(())
}
fn allowed(facts: &Value, domain: &str, name: &str) -> bool {
    facts["operations"].as_object().into_iter().flatten().any(|(_, op)| op["kind"] == "job" && op["access"][format!("{domain}.{name}")] == true)
}
fn file_json(f: &aip_execution::files::ManagedFile) -> Value {
    json!({"id":f.handle.to_string(),"bucket":f.bucket,"name":f.name,"mediaType":f.media_type,"size":f.bytes.len()})
}
fn exact(body: &Value, keys: &[&str]) -> bool {
    body.as_object().is_some_and(|o| o.len() == keys.len() && keys.iter().all(|k| o.contains_key(*k)))
}
fn text<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str().ok_or("WIRE.BAD_REQUEST")
}
fn decode(body: &Value) -> Result<Vec<u8>> {
    let b = B64.decode(text(body, "base64")?).map_err(|_| "WIRE.BAD_VALUE")?;
    if b.len() > 768 * 1024 - 1024 || B64.encode(&b) != body["base64"] {
        return Err("WIRE.PAYLOAD_TOO_LARGE");
    }
    Ok(b)
}
async fn file_budget(db: &impl GenericClient, s: &str, owner: &str, bucket: &str, name: &str, size: i64, object_key: &str) -> Result<()> {
    db.query_one("SELECT pg_advisory_xact_lock(hashtext($1))", &[&format!("aip-files:{s}:{owner}")]).await.map_err(db_error)?;
    db.execute(&format!("DELETE FROM {s}.aip_files WHERE owner=$1 AND expires_at<=clock_timestamp()"), &[&owner]).await.map_err(db_error)?;
    let row=db.query_one(&format!("SELECT count(*)::bigint,COALESCE(sum(octet_length(content)),0)::bigint,bool_or(bucket=$2 AND name=$3 AND object_key=$4) FROM {s}.aip_files WHERE owner=$1"),&[&owner,&bucket,&name,&object_key]).await.map_err(db_error)?;
    if row.get::<_, Option<bool>>(2) != Some(true) && (row.get::<_, i64>(0) >= 100 || row.get::<_, i64>(1) + size > 10 * 1024 * 1024) {
        return Err("WIRE.QUOTA_EXCEEDED");
    }
    Ok(())
}
async fn put_file(
    db: &impl GenericClient,
    s: &str,
    owner: &str,
    bucket: &str,
    input: &Value,
    run: Option<&Lease>,
    key_scope: &str,
) -> Result<aip_execution::files::ManagedFile> {
    let name = text(input, "name")?;
    let bytes = decode(input)?;
    let object_key = run.map(|r| spike_v1_fixture::digest(&json!([r.run_id.to_string(), key_scope]))).unwrap_or_default();
    file_budget(db, s, owner, bucket, name, bytes.len() as i64, &object_key).await?;
    let expires: i64 = if let Some(lease) = run {
        db.query_one(&format!("SELECT floor(extract(epoch FROM run_deadline))::bigint+86400 FROM {s}.aip_jobs WHERE id=$1"), &[&lease.run_id])
            .await
            .map_err(db_error)?
            .get(0)
    } else {
        db.query_one(&format!("SELECT COALESCE((SELECT floor(extract(epoch FROM expires_at))::bigint FROM {s}.aip_files WHERE owner=$1 AND bucket=$2 AND name=$3 AND object_key=$4),floor(extract(epoch FROM clock_timestamp()))::bigint+86400)"),&[&owner,&bucket,&name,&object_key]).await.map_err(db_error)?.get(0)
    };
    aip_execution::files::Store::new(s)
        .map_err(db_error)?
        .put_scoped(db, owner, bucket, name, text(input, "mediaType")?, &bytes, Some(expires), &object_key)
        .await
        .map_err(|e| match e {
            aip_execution::files::Error::FileConflict => "WIRE.IDEMPOTENCY_MISMATCH",
            aip_execution::files::Error::InvalidFile => "WIRE.BAD_VALUE",
            _ => "WIRE.DB_UNAVAILABLE",
        })
}
#[allow(clippy::too_many_arguments)]
async fn submit(
    db: &impl GenericClient,
    rt: &Runtime,
    facts: &Value,
    wire: IdWire,
    grant: &DurablePrincipal,
    name: &str,
    input: &Value,
    key: &str,
    delay: i64,
    service: bool,
) -> Result<aip_execution::Run> {
    spike_v4_worker::validate_job(facts, name, input, wire).map_err(|e| e.code)?;
    if !crate::valid_key(key) || !(0..=86_400_000).contains(&delay) {
        return Err("WIRE.BAD_VALUE");
    }
    // Trigger keys are minted by the server. A caller who is also the configured service subject must not be able to claim a
    // schedule slot, webhook delivery or outbox row in advance and turn the real trigger into an IDEMPOTENCY_MISMATCH.
    if !service && crate::triggers::RESERVED_KEY_PREFIXES.iter().any(|prefix| key.starts_with(prefix)) {
        return Err("WIRE.BAD_VALUE");
    }
    // O-19: a trigger's operation trusts its input because the server verified its source (HMAC body, schedule input, committed
    // outbox row). A user who passes the operation's `allow` could otherwise enqueue a forged body and skip that verification.
    if !service && rt.infrastructure.triggers.targets_operation(name) {
        return Err("WIRE.ACCESS_DENIED");
    }
    authorize(db, facts, name, input, grant.actor_id, wire).await?;
    let s = schema(rt)?;
    // Admission is bounded per principal, independently of HTTP and worker slots.
    let owner = owner(grant);
    db.query_one("SELECT pg_advisory_xact_lock(hashtext($1))", &[&format!("aip-jobs:{s}:{owner}")]).await.map_err(db_error)?;
    let active: i64 = db
        .query_one(&format!("SELECT count(*) FROM {s}.aip_jobs WHERE owner=$1 AND state IN ('QUEUED','RUNNING','CANCEL_REQUESTED')"), &[&owner])
        .await
        .map_err(db_error)?
        .get(0);
    let contract = spike_v5_sdk::contract_fingerprint_with_all_extensions(facts, wire);
    let deployment = &rt.deployment.as_ref().unwrap().expected_digest;
    let replay:bool=db.query_one(&format!("SELECT EXISTS(SELECT 1 FROM {s}.aip_jobs WHERE owner=$1 AND operation=$2 AND idempotency_key=$3 AND contract_digest=$4 AND deployment_digest=$5)"),&[&owner,&name,&key,&contract,&deployment]).await.map_err(db_error)?.get(0);
    let total: i64 = db.query_one(&format!("SELECT count(*) FROM {s}.aip_jobs WHERE owner=$1"), &[&owner]).await.map_err(db_error)?.get(0);
    // Trigger traffic is unbounded over time; a retained-history cap would silently stop every trigger
    // sharing the service principal. The active bound still limits concurrent load.
    if (active >= 100 || (!service && total >= 10_000)) && !replay {
        return Err("WIRE.QUOTA_EXCEEDED");
    }
    // Retained payload bytes (queued and unexpired inputs and outputs) are bounded like managed files and messages. Unlike the
    // row cap this applies to trigger principals too: payloads are cleared when a result expires, so the bound does not stall
    // triggers for long, and it stops one principal from filling the database with 1 MiB inputs.
    if !replay {
        let retained: i64 = db
            .query_one(
                &format!(
                    "SELECT coalesce(sum(octet_length(input::text)+coalesce(octet_length(output::text),0)),0)::bigint FROM {s}.aip_jobs \
                     WHERE owner=$1 AND (state IN ('QUEUED','RUNNING','CANCEL_REQUESTED') OR result_expires_at>clock_timestamp())"
                ),
                &[&owner],
            )
            .await
            .map_err(db_error)?
            .get(0);
        let incoming = serde_json::to_vec(input).map_or(i64::MAX, |bytes| bytes.len() as i64);
        if retained.saturating_add(incoming) > MAX_RETAINED_JOB_BYTES {
            return Err("WIRE.QUOTA_EXCEEDED");
        }
    }
    // The trace of the request that enqueues, and that request's span, so the run's span can be its child.
    let trace_id = aip_execution::trace_context::current().unwrap_or_else(aip_execution::trace_context::TraceContext::generate).job_value();
    Store::new(s)
        .map_err(store_error)?
        .enqueue(
            db,
            NewRun {
                owner,
                actor: grant.actor_id,
                grant: serde_json::to_value(grant).map_err(db_error)?,
                operation: name.into(),
                contract: spike_v5_sdk::contract_fingerprint_with_all_extensions(facts, wire),
                deployment: rt.deployment.as_ref().unwrap().expected_digest.clone(),
                implementation: bundle(rt)?,
                input: input.clone(),
                key: key.into(),
                timeout_ms: facts["operations"][name]["deadlineMs"].as_i64().ok_or("WIRE.BAD_CONFIGURATION")?,
                max_attempts: policy(rt, name).max_attempts,
                ttl_ms: 86_400_000,
                delay_ms: delay,
                trace_id: Some(trace_id),
            },
        )
        .await
        .map_err(store_error)
}

/// Enqueues a job for a server trigger as an operator-bound principal. The principal row is read at
/// submission time, so revoking or disabling it stops future triggers without a restart.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn submit_as_service(
    rt: &Runtime,
    facts: &Value,
    wire: IdWire,
    principal: &crate::triggers::ServicePrincipal,
    name: &str,
    input: &Value,
    key: &str,
    outbox_marker: Option<(&str, i64)>,
) -> Result<aip_execution::Run> {
    let marker = match outbox_marker {
        Some((infra, id)) => Marker::Outbox(infra, id),
        None => Marker::None,
    };
    submit_as_service_marked(rt, facts, wire, principal, name, input, key, marker).await
}

/// What else commits together with a trigger's job acceptance.
enum Marker<'a> {
    None,
    /// The outbox row is marked dispatched.
    Outbox(&'a str, i64),
    /// The consumed message is acked.
    Message(&'a aip_execution::messages::MessageLease),
}

/// The service principal's grant as of now. A disabled or unbound principal accepts nothing.
async fn service_grant(db: &impl GenericClient, s: &str, principal: &crate::triggers::ServicePrincipal) -> Result<DurablePrincipal> {
    let row = db
        .query_opt(
            &format!(
                "SELECT actor_id,enabled,min_iat,floor(extract(epoch FROM clock_timestamp()))::bigint FROM {s}.aip_principals WHERE issuer=$1 AND subject=$2"
            ),
            &[&principal.issuer, &principal.subject],
        )
        .await
        .map_err(db_error)?
        .filter(|row| row.get::<_, bool>(1))
        .ok_or("WIRE.PRINCIPAL_UNAVAILABLE")?;
    Ok(DurablePrincipal {
        issuer: principal.issuer.clone(),
        subject: principal.subject.clone(),
        actor_id: row.get(0),
        min_iat: row.get(2),
        issued_at: row.get(3),
    })
}

#[allow(clippy::too_many_arguments)]
async fn submit_as_service_marked(
    rt: &Runtime,
    facts: &Value,
    wire: IdWire,
    principal: &crate::triggers::ServicePrincipal,
    name: &str,
    input: &Value,
    key: &str,
    marker: Marker<'_>,
) -> Result<aip_execution::Run> {
    if !enabled(facts) || rt.extensions.is_none() {
        return Err("WIRE.NOT_AVAILABLE");
    }
    let s = schema(rt)?.to_string();
    let mut db = connect(rt).await?;
    let tx = db.transaction().await.map_err(db_error)?;
    let grant = service_grant(&tx, &s, principal).await?;
    guard(&tx, rt, &grant).await?;
    let run = submit(&tx, rt, facts, wire, &grant, name, input, key, 0, true).await?;
    if let Marker::Message(lease) = marker {
        // A lost lease means another consumer owns the message now; roll back so only one acks it.
        let store = aip_execution::messages::MessageStore::new(&s).map_err(db_error)?;
        if !store.ack(&tx, lease).await.map_err(db_error)? {
            return Err("WIRE.LEASE_LOST");
        }
    }
    if let Marker::Outbox(infra, outbox_id) = marker {
        // The dispatch marker commits with the job acceptance, so a row is never marked without its job.
        tx.execute(
            &format!(
                "INSERT INTO {infra}.aip_outbox_dispatch(outbox_id,outbox_created_at,run_id) SELECT id,created_at,$2::text::uuid FROM {s}.aip_outbox WHERE id=$1 ON CONFLICT DO NOTHING"
            ),
            &[&outbox_id, &run.id],
        )
        .await
        .map_err(db_error)?;
    }
    commit_request(tx).await?;
    db.release().await.map_err(db_error)?;
    Ok(run)
}

/// Fires the latest due slot of each schedule once. Older missed slots are skipped by design (coalesced),
/// and the per-slot job key makes concurrent instances and restarts converge on one run.
async fn fire_schedules(rt: &Runtime, facts: &Value, wire: IdWire) {
    for schedule in &rt.infrastructure.triggers.schedules {
        let Some(slot) = schedule.due_slot() else { continue };
        let key = crate::triggers::schedule_key(&schedule.name, slot);
        if let Err(code) = submit_as_service(rt, facts, wire, &schedule.principal, &schedule.operation, &schedule.input, &key, None).await {
            eprintln!("{}", json!({"event":"schedule_submit_failed","schedule":schedule.name,"code":code}));
        }
    }
}

/// Table recording which committed outbox rows already became jobs. It lives in the infrastructure
/// schema so the application schema keeps its migration fingerprint. The creation time is part of the
/// identity so a recreated application schema whose sequence restarts is not mistaken for old rows.
pub(crate) fn outbox_dispatch_ddl(infra: &str) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {infra}.aip_outbox_dispatch(outbox_id bigint NOT NULL, outbox_created_at timestamptz NOT NULL, run_id uuid NOT NULL, dispatched_at timestamptz NOT NULL DEFAULT clock_timestamp(), PRIMARY KEY(outbox_id,outbox_created_at))"
    )
}

/// Turns committed `notify` rows of configured topics into jobs. Each topic is scanned separately so a
/// handler that keeps failing cannot starve other topics. Delivery order across rows is not guaranteed;
/// each row becomes exactly one job via its key.
async fn relay_outbox(rt: &Runtime, facts: &Value, wire: IdWire) -> Result<()> {
    let s = schema(rt)?.to_string();
    let infra = aip_execution::infrastructure_schema(&s).map_err(|_| "WIRE.BAD_CONFIGURATION")?;
    for handler in &rt.infrastructure.triggers.outbox {
        let db = connect(rt).await?;
        let rows = db
            .query(
                &format!(
                    "SELECT o.id,o.source,o.source_id::text,o.recipient_id::text,o.created_at::text,(extract(epoch FROM o.created_at)*1000000)::bigint FROM {s}.aip_outbox o \
                     WHERE o.topic=$1 AND NOT EXISTS (SELECT 1 FROM {infra}.aip_outbox_dispatch d WHERE d.outbox_id=o.id AND d.outbox_created_at=o.created_at) \
                     ORDER BY o.id LIMIT 50"
                ),
                &[&handler.topic],
            )
            .await
            .map_err(db_error)?;
        db.release().await.map_err(db_error)?;
        for row in rows {
            let id: i64 = row.get(0);
            let event = json!({
                "id": id.to_string(),
                "topic": handler.topic,
                "source": row.get::<_, String>(1),
                "sourceId": row.get::<_, Option<String>>(2),
                "recipientId": row.get::<_, Option<String>>(3),
                "createdAt": row.get::<_, String>(4),
            });
            let input = json!({"event": event.to_string()});
            let key = crate::triggers::outbox_key(id, row.get(5));
            if let Err(code) = submit_as_service(rt, facts, wire, &handler.principal, &handler.operation, &input, &key, Some((&infra, id))).await {
                eprintln!("{}", json!({"event":"outbox_dispatch_failed","topic":handler.topic,"code":code}));
                break;
            }
        }
    }
    Ok(())
}

/// Turns ready messages of configured queues into jobs, at least once. The job key is the message id,
/// so a message redelivered after a crash between acceptance and ack converges on the same job.
/// A failed acceptance nacks with backoff; after `max_deliveries` the message moves to the DLQ.
async fn relay_messages(rt: &Runtime, facts: &Value, wire: IdWire, worker: &str) -> Result<()> {
    let s = schema(rt)?.to_string();
    let store = aip_execution::messages::MessageStore::new(&s).map_err(db_error)?;
    for handler in &rt.infrastructure.triggers.messages {
        let owner = match &handler.owner {
            Some(owner) => owner.clone(),
            None => {
                let db = connect(rt).await?;
                let grant = service_grant(&*db, &s, &handler.principal).await;
                db.release().await.map_err(db_error)?;
                match grant {
                    Ok(grant) => self::owner(&grant),
                    Err(code) => {
                        eprintln!("{}", json!({"event":"message_relay_failed","queue":handler.queue,"code":code}));
                        continue;
                    }
                }
            }
        };
        for _ in 0..MESSAGES_PER_TICK {
            let db = connect(rt).await?;
            let lease = store.claim(&*db, &owner, &handler.queue, worker, MESSAGE_LEASE_MS).await.map_err(db_error)?;
            db.release().await.map_err(db_error)?;
            let Some(lease) = lease else { break };
            let message = json!({"id":lease.id,"queue":lease.queue,"payload":lease.payload,"deliveries":lease.deliveries});
            let input = json!({"message": message.to_string()});
            let key = crate::triggers::message_key(&lease.id);
            let accepted =
                submit_as_service_marked(rt, facts, wire, &handler.principal, &handler.operation, &input, &key, Marker::Message(&lease)).await;
            if let Err(code) = accepted {
                eprintln!("{}", json!({"event":"message_dispatch_failed","queue":handler.queue,"code":code}));
                let db = connect(rt).await?;
                let _ = store.nack(&*db, &lease, code).await;
                db.release().await.map_err(db_error)?;
                break;
            }
        }
    }
    Ok(())
}

pub(crate) async fn validate_principal(rt: &Runtime, _facts: &Value, _wire: IdWire, session: &crate::auth::Session) -> Result<String> {
    if session.remaining_ms() == 0 {
        return Err("WIRE.TOKEN_EXPIRED");
    }
    let grant = session.durable.as_ref().ok_or("WIRE.UNAUTHENTICATED")?;
    let mut db = connect(rt).await?;
    let tx = db.transaction().await.map_err(db_error)?;
    guard(&tx, rt, grant).await?;
    tx.commit().await.map_err(db_error)?;
    db.release().await.map_err(db_error)?;
    Ok(owner(grant))
}

/// One run of a data subscription: the read of `query` as the subscriber, with every check a read has. The principal and
/// the deployment are verified first, so a subscriber who was revoked, or a deployment that changed, ends the
/// subscription instead of leaving it on a stale view. The result is the rows of the read in the wire's encoding.
pub(crate) async fn read_as_subscriber(
    rt: &Runtime,
    facts: &Value,
    wire: IdWire,
    session: &crate::auth::Session,
    query: &Value,
) -> std::result::Result<Value, &'static str> {
    if session.remaining_ms() == 0 {
        return Err("WIRE.TOKEN_EXPIRED");
    }
    let grant = session.durable.as_ref().ok_or("WIRE.UNAUTHENTICATED")?;
    let mut db = connect(rt).await?;
    let now: String = {
        let tx = db.transaction().await.map_err(db_error)?;
        guard(&tx, rt, grant).await?;
        let now = tx.query_one("SELECT clock_timestamp()::text", &[]).await.map_err(db_error)?.get(0);
        tx.commit().await.map_err(db_error)?;
        now
    };
    let caller = Caller { actor_id: Some(grant.actor_id), now };
    let plan = spike_v2_read::plan::plan_read_with_wire(facts, query, &caller, wire).map_err(|e| e.code)?;
    let rows = tokio::time::timeout(Duration::from_millis(plan.deadline_ms), spike_v2_read::execute(&mut db, &plan))
        .await
        .map_err(|_| "WIRE.DEADLINE_EXCEEDED")?
        .map_err(|e| e.code)?;
    let rows = spike_v2_read::id_wire::encode_rows(rows, &plan.output_type, wire).map_err(|e| e.code)?;
    let _ = db.release().await;
    Ok(Value::Array(rows))
}

pub(crate) async fn read_events(
    rt: &Runtime,
    facts: &Value,
    wire: IdWire,
    session: &crate::auth::Session,
    stream: &str,
    after: &str,
    limit: i64,
) -> Value {
    if session.remaining_ms() == 0 {
        return json!({"ok":false,"code":"WIRE.TOKEN_EXPIRED"});
    }
    let Some(grant) = session.durable.as_ref() else {
        return json!({"ok":false,"code":"WIRE.UNAUTHENTICATED"});
    };
    let mut db = match connect(rt).await {
        Ok(db) => db,
        Err(code) => return json!({"ok":false,"code":code}),
    };
    let mut response = handle(&mut db, rt, facts, wire, "/events/read", &json!({"stream":stream,"after":after,"limit":limit}), grant).await;
    let _ = db.release().await;
    response["contractFingerprint"] = json!(spike_v5_sdk::contract_fingerprint_with_all_extensions(facts, wire));
    response
}

pub(crate) async fn handle(db: &mut Client, rt: &Runtime, facts: &Value, wire: IdWire, path: &str, body: &Value, grant: &DurablePrincipal) -> Value {
    let operation = async {
        if !enabled(facts) || rt.extensions.is_none() {
            return Err("WIRE.NOT_AVAILABLE");
        }
        let tx = db.transaction().await.map_err(db_error)?;
        guard(&tx, rt, grant).await?;
        let s = schema(rt)?;
        let owner = owner(grant);
        let store = Store::new(s).map_err(store_error)?;
        let response = match path {
            "/jobs/enqueue" => {
                if !exact(body, &["operation", "input", "key", "delayMs"]) {
                    return Err("WIRE.BAD_REQUEST");
                }
                let run = submit(
                    &tx,
                    rt,
                    facts,
                    wire,
                    grant,
                    text(body, "operation")?,
                    &body["input"],
                    text(body, "key")?,
                    body["delayMs"].as_i64().ok_or("WIRE.BAD_REQUEST")?,
                    false,
                )
                .await?;
                json!({"ok":true,"run":run})
            }
            "/jobs/status" | "/jobs/cancel" | "/jobs/result" => {
                if !exact(body, &["id"]) {
                    return Err("WIRE.BAD_REQUEST");
                }
                let id = text(body, "id")?;
                if path == "/jobs/result" {
                    let run = store.status(&tx, id, &owner).await.map_err(store_error)?.ok_or("WIRE.NOT_FOUND")?;
                    let result = store.result(&tx, id, &owner).await.map_err(store_error)?.ok_or(if run.state == "SUCCEEDED" {
                        "WIRE.RESULT_EXPIRED"
                    } else if run.state == "FAILED" {
                        "WIRE.JOB_FAILED"
                    } else {
                        "WIRE.JOB_NOT_READY"
                    })?;
                    let row = tx
                        .query_one(
                            &format!("SELECT input,deployment_digest,contract_digest FROM {s}.aip_jobs WHERE id=$1::text::uuid AND owner=$2"),
                            &[&id, &owner],
                        )
                        .await
                        .map_err(db_error)?;
                    if row.get::<_, String>(1) != rt.deployment.as_ref().unwrap().expected_digest
                        || row.get::<_, String>(2) != spike_v5_sdk::contract_fingerprint_with_all_extensions(facts, wire)
                    {
                        return Err("WIRE.DEPLOYMENT_CHANGED");
                    }
                    authorize(&tx, facts, &result.operation, &row.get::<_, Value>(0), grant.actor_id, wire).await?;
                    json!({"ok":true,"id":result.id,"operation":result.operation,"output":result.output})
                } else {
                    let mut run = if path == "/jobs/cancel" { store.cancel(&tx, id, &owner).await } else { store.status(&tx, id, &owner).await }
                        .map_err(store_error)?
                        .ok_or("WIRE.NOT_FOUND")?;
                    if run.state == "FAILED" || run.state == "CANCELLED" {
                        let pending: bool = tx
                            .query_one(
                                &format!(
                                    "SELECT EXISTS(SELECT 1 FROM {s}.aip_effects WHERE run_id=$1::text::uuid AND state IN ('PENDING','UNKNOWN'))"
                                ),
                                &[&id],
                            )
                            .await
                            .map_err(db_error)?
                            .get(0);
                        if pending {
                            run.error = Some(
                                if path == "/jobs/cancel" || run.error.as_deref() == Some("WIRE.CANCELLATION_UNCONFIRMED") {
                                    "WIRE.CANCELLATION_UNCONFIRMED"
                                } else {
                                    "WIRE.EFFECT_UNKNOWN"
                                }
                                .into(),
                            );
                        }
                    }
                    run.output = None;
                    json!({"ok":true,"run":run})
                }
            }
            "/files/put" => {
                if !rt.enable_file_uploads {
                    return Err("WIRE.NOT_EXPOSED");
                }
                if !exact(body, &["bucket", "name", "mediaType", "base64"]) {
                    return Err("WIRE.BAD_REQUEST");
                }
                let bucket = text(body, "bucket")?;
                if !allowed(facts, "files", bucket) {
                    return Err("WIRE.NOT_EXPOSED");
                }
                let file = put_file(&tx, s, &owner, bucket, body, None, "").await?;
                json!({"ok":true,"file":file_json(&file)})
            }
            "/files/get" | "/files/delete" => {
                if !exact(body, &["id"]) {
                    return Err("WIRE.BAD_REQUEST");
                }
                let id = text(body, "id")?.parse().map_err(|_| "WIRE.BAD_VALUE")?;
                let files = aip_execution::files::Store::new(s).map_err(db_error)?;
                let file = files.get(&tx, &owner, &id).await.map_err(db_error)?.ok_or("WIRE.NOT_FOUND")?;
                if !allowed(facts, "files", &file.bucket) {
                    return Err("WIRE.NOT_EXPOSED");
                }
                if path == "/files/delete" {
                    files.delete(&tx, &owner, &id).await.map_err(db_error)?;
                    json!({"ok":true,"deleted":true})
                } else {
                    json!({"ok":true,"file":file_json(&file),"base64":B64.encode(&file.bytes)})
                }
            }
            "/files/mount-get" => {
                if !exact(body, &["id", "bucket"]) {
                    return Err("WIRE.BAD_REQUEST");
                }
                let bucket = text(body, "bucket")?;
                if !allowed(facts, "files", bucket) {
                    return Err("WIRE.NOT_EXPOSED");
                }
                let id = text(body, "id")?.parse().map_err(|_| "WIRE.BAD_VALUE")?;
                let file = aip_execution::files::Store::new(s)
                    .map_err(db_error)?
                    .get_in_bucket(&tx, &owner, &id, bucket)
                    .await
                    .map_err(db_error)?
                    .ok_or("WIRE.NOT_FOUND")?;
                json!({"ok":true,"file":file_json(&file),"base64":B64.encode(&file.bytes)})
            }
            "/events/read" => {
                if !exact(body, &["stream", "after", "limit"]) {
                    return Err("WIRE.BAD_REQUEST");
                }
                let stream = text(body, "stream")?;
                if !allowed(facts, "events", stream) {
                    return Err("WIRE.NOT_EXPOSED");
                }
                let limit = body["limit"].as_i64().filter(|n| (1..=100).contains(n)).ok_or("WIRE.BAD_VALUE")?;
                let page =
                    aip_execution::events::Store::new(s).map_err(db_error)?.scan(&tx, &owner, stream, text(body, "after")?, limit).await.map_err(
                        |e| match e {
                            aip_execution::events::Error::CursorExpired => "WIRE.CURSOR_EXPIRED",
                            aip_execution::events::Error::InvalidEvent => "WIRE.BAD_VALUE",
                            _ => "WIRE.DB_UNAVAILABLE",
                        },
                    )?;
                json!({"ok":true,"events":page.events.iter().map(|e|json!({"cursor":e.cursor,"payload":e.payload})).collect::<Vec<_>>(),"cursor":page.next_cursor})
            }
            path if path.starts_with("/messages/") => crate::messaging_transport::handle(&tx, rt, facts, s, &owner, path, body).await?,
            _ => return Err("WIRE.NOT_FOUND"),
        };
        commit_request(tx).await?;
        Ok(response)
    };
    match tokio::time::timeout(Duration::from_secs(5), operation).await {
        Ok(Ok(response)) => response,
        Ok(Err(code)) => json!({"ok":false,"code":code}),
        Err(_) => json!({"ok":false,"code":"WIRE.COMMIT_UNKNOWN"}),
    }
}

/// Per principal, matching the 64 MiB message quota. Counted at submission, so outputs written afterwards by jobs that are
/// already running can overshoot by at most the 100 active jobs' 1 MiB result limit.
const MAX_RETAINED_JOB_BYTES: i64 = 64 * 1024 * 1024;
const GC_BATCH: i64 = 1000;
const GC_BATCHES_PER_SWEEP: usize = 10;
const EVENTS_SWEEP_INTERVAL: Duration = Duration::from_secs(600);

/// What one collector sweep is asked to clean beyond jobs, files and effects.
struct Sweep<'a> {
    messages: bool,
    retention: crate::infrastructure::Retention,
    /// Topics with a configured handler. Their rows belong to the relay until dispatched.
    handled_topics: &'a [String],
    /// Event expiry reads the whole events table, so it runs on a slower cadence than the rest of the sweep.
    events: bool,
}

async fn collect_expired(db: &mut Client, s: &str, sweep: &Sweep<'_>) -> Result<()> {
    let tx = db.transaction().await.map_err(db_error)?;
    tx.batch_execute("SET LOCAL lock_timeout='2s'; SET LOCAL statement_timeout='5s'").await.map_err(db_error)?;
    tx.query_one("SELECT pg_advisory_xact_lock_shared(1095323725,hashtext($1))", &[&s]).await.map_err(db_error)?;
    if !tx.query_one("SELECT pg_try_advisory_xact_lock(1095323726,hashtext($1))", &[&s]).await.map_err(db_error)?.get::<_, bool>(0) {
        return Ok(());
    }
    Store::new(s).map_err(store_error)?.expire_payloads(&tx).await.map_err(store_error)?;
    tx.execute(&format!("UPDATE {s}.aip_effects SET result=NULL WHERE ctid IN (SELECT e.ctid FROM {s}.aip_effects e JOIN {s}.aip_jobs j ON e.run_id=j.id WHERE j.result_expires_at<=clock_timestamp() AND e.result IS NOT NULL LIMIT 1000)"),&[]).await.map_err(db_error)?;
    tx.execute(
        &format!("DELETE FROM {s}.aip_files WHERE handle IN (SELECT handle FROM {s}.aip_files WHERE expires_at<=clock_timestamp() LIMIT 1000)"),
        &[],
    )
    .await
    .map_err(db_error)?;
    if sweep.messages {
        aip_execution::messages::MessageStore::new(s)
            .map_err(|_| "WIRE.DB_UNAVAILABLE")?
            .cleanup_expired(&tx, 1000)
            .await
            .map_err(|_| "WIRE.DB_UNAVAILABLE")?;
    }
    // Trigger principals have no retained-history cap, so tombstones must be removed at least as fast as triggers create them.
    // A sweep runs every minute; several batches per sweep keep up with far more than the former 100 jobs per minute.
    for _ in 0..GC_BATCHES_PER_SWEEP {
        let old: Vec<String> = tx
            .query(
                &format!("SELECT id::text FROM {s}.aip_jobs WHERE state IN ('SUCCEEDED','FAILED','CANCELLED') AND result_expires_at<clock_timestamp()-interval '30 days' LIMIT {GC_BATCH} FOR UPDATE SKIP LOCKED"),
                &[],
            )
            .await
            .map_err(db_error)?
            .iter()
            .map(|row| row.get(0))
            .collect();
        if old.is_empty() {
            break;
        }
        tx.execute(&format!("DELETE FROM {s}.aip_effects WHERE run_id=ANY($1::text[]::uuid[])"), &[&old]).await.map_err(db_error)?;
        tx.execute(&format!("DELETE FROM {s}.aip_jobs WHERE id=ANY($1::text[]::uuid[])"), &[&old]).await.map_err(db_error)?;
        if (old.len() as i64) < GC_BATCH {
            break;
        }
    }
    if !sweep.handled_topics.is_empty() {
        // A dispatch row means the relay already turned the outbox row into a job; both are history once the job record is gone.
        let infra = aip_execution::infrastructure_schema(s).map_err(|_| "WIRE.BAD_CONFIGURATION")?;
        let days = i32::try_from(sweep.retention.outbox_dispatched_days).map_err(|_| "WIRE.BAD_CONFIGURATION")?;
        for _ in 0..GC_BATCHES_PER_SWEEP {
            let removed = tx
                .execute(
                    &format!(
                        "WITH old AS (SELECT outbox_id,outbox_created_at FROM {infra}.aip_outbox_dispatch WHERE dispatched_at<clock_timestamp()-make_interval(days=>$1) ORDER BY dispatched_at LIMIT {GC_BATCH} FOR UPDATE SKIP LOCKED), \
                         gone AS (DELETE FROM {s}.aip_outbox o USING old WHERE o.id=old.outbox_id AND o.created_at=old.outbox_created_at) \
                         DELETE FROM {infra}.aip_outbox_dispatch d USING old WHERE d.outbox_id=old.outbox_id AND d.outbox_created_at=old.outbox_created_at"
                    ),
                    &[&days],
                )
                .await
                .map_err(db_error)?;
            if (removed as i64) < GC_BATCH {
                break;
            }
        }
    }
    if let Some(days) = sweep.retention.outbox_unhandled_days {
        // Rows of a topic nothing consumes. They are written in id order, so the oldest unhandled rows come first and the
        // walk stops at the first one that is still young; a sweep never reads past the head of the table.
        let days = i32::try_from(days).map_err(|_| "WIRE.BAD_CONFIGURATION")?;
        for _ in 0..GC_BATCHES_PER_SWEEP {
            let removed = tx
                .execute(
                    &format!(
                        "WITH head AS (SELECT id,created_at FROM {s}.aip_outbox WHERE NOT (topic = ANY($1::text[])) ORDER BY id LIMIT {GC_BATCH}) \
                         DELETE FROM {s}.aip_outbox o USING head WHERE o.id=head.id AND head.created_at<clock_timestamp()-make_interval(days=>$2)"
                    ),
                    &[&sweep.handled_topics, &days],
                )
                .await
                .map_err(db_error)?;
            if (removed as i64) < GC_BATCH {
                break;
            }
        }
    }
    if sweep.events {
        if let Some(days) = sweep.retention.events_days {
            aip_execution::events::Store::new(s)
                .map_err(|_| "WIRE.BAD_CONFIGURATION")?
                .expire_older_than(&tx, i64::from(days), 1000)
                .await
                .map_err(|_| "WIRE.DB_UNAVAILABLE")?;
        }
    }
    tx.commit().await.map_err(db_error)
}

pub(crate) async fn supervisor(rt: Arc<Runtime>, facts: Arc<Value>, wire: IdWire) {
    let mut tasks = tokio::task::JoinSet::new();
    let cleanup_messages = should_cleanup_messages(rt.infrastructure.messaging, &facts);
    let handled_topics: Vec<String> = rt.infrastructure.triggers.outbox.iter().map(|handler| handler.topic.clone()).collect();
    let mut events_swept: Option<tokio::time::Instant> = None;
    let worker =
        format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    let mut gc = tokio::time::Instant::now();
    let mut scheduled: Option<tokio::time::Instant> = None;
    let mut relayed: Option<tokio::time::Instant> = None;
    loop {
        let triggers = &rt.infrastructure.triggers;
        if !(triggers.outbox.is_empty() && triggers.messages.is_empty()) && relayed.is_none_or(|at| at.elapsed() >= OUTBOX_TICK) {
            if let Err(code) = relay_outbox(&rt, &facts, wire).await {
                eprintln!("{}", json!({"event":"outbox_relay_failed","code":code}));
            }
            if let Err(code) = relay_messages(&rt, &facts, wire, &worker).await {
                eprintln!("{}", json!({"event":"message_relay_failed","code":code}));
            }
            relayed = Some(tokio::time::Instant::now());
        }
        if !rt.infrastructure.triggers.schedules.is_empty() && scheduled.is_none_or(|at| at.elapsed() >= SCHEDULE_TICK) {
            fire_schedules(&rt, &facts, wire).await;
            scheduled = Some(tokio::time::Instant::now());
        }
        if gc.elapsed() >= Duration::from_secs(60) {
            if let Ok(mut db) = connect(&rt).await {
                if let Ok(s) = schema(&rt) {
                    let events = events_swept.is_none_or(|at| at.elapsed() >= EVENTS_SWEEP_INTERVAL);
                    let sweep = Sweep { messages: cleanup_messages, retention: rt.infrastructure.retention, handled_topics: &handled_topics, events };
                    if collect_expired(&mut db, s, &sweep).await.is_err() {
                        eprintln!("{{\"event\":\"job_gc_failed\"}}");
                    } else {
                        if events {
                            events_swept = Some(tokio::time::Instant::now());
                        }
                        let _ = db.release().await;
                    }
                }
            }
            gc = tokio::time::Instant::now();
        }
        while tasks.len() < rt.infrastructure.resource_limits.jobs {
            let Ok(mut db) = connect(&rt).await else { break };
            let Ok(tx) = db.transaction().await else { break };
            let Ok(store) = schema(&rt).and_then(|s| Store::new(s).map_err(store_error)) else { return };
            if tx.query_one("SELECT pg_advisory_xact_lock_shared(1095323725,hashtext($1))", &[&schema(&rt).unwrap()]).await.is_err() {
                break;
            }
            if tx.batch_execute("SET LOCAL lock_timeout='2s'; SET LOCAL statement_timeout='5s'").await.is_err() {
                break;
            }
            let lease = match store.claim(&tx, &worker).await {
                Ok(Some(lease)) => lease,
                Ok(None) => {
                    if tx.commit().await.is_ok() {
                        let _ = db.release().await;
                    }
                    break;
                }
                Err(_) => break,
            };
            let admission = async {
                let grant: DurablePrincipal = serde_json::from_value(lease.grant.clone()).map_err(|_| "WIRE.UNAUTHENTICATED")?;
                guard(&tx, &rt, &grant).await?;
                if lease.deployment != rt.deployment.as_ref().unwrap().expected_digest || lease.implementation != bundle(&rt)? {
                    return Err("WIRE.IMPLEMENTATION_CHANGED");
                }
                authorize(&tx, &facts, &lease.operation, &lease.input, lease.actor, wire).await?;
                Ok(())
            }
            .await;
            if let Err(code) = admission {
                let update = if code == "WIRE.DB_UNAVAILABLE" { store.retry(&tx, &lease, code).await } else { store.fail(&tx, &lease, code).await };
                if update.is_err() || tx.commit().await.is_err() {
                    eprintln!("{{\"event\":\"job_admission_failed\"}}");
                    break;
                }
                let _ = db.release().await;
                continue;
            }
            if tx.commit().await.is_err() {
                break;
            }
            let _ = db.release().await;
            let (rt, facts) = (rt.clone(), facts.clone());
            tasks.spawn(async move {
                execute(rt, facts, wire, lease).await;
            });
        }
        tokio::select! {
            _=tokio::time::sleep(Duration::from_millis(100))=>{},
            Some(result)=tasks.join_next(),if !tasks.is_empty()=> {if result.is_err(){eprintln!("{{\"event\":\"job_task_failed\"}}");}},
        }
    }
}
async fn execute(rt: Arc<Runtime>, facts: Arc<Value>, wire: IdWire, lease: Lease) {
    // The run has its own active span. Nested ctx.jobs.run steps use it as their parent and keep the same durable lease.
    let trace_ctx = match &lease.trace_id {
        Some(stored) => aip_execution::trace_context::TraceContext::from_job_value(stored),
        None => aip_execution::trace_context::TraceContext::generate(),
    }
    .with_span_id(crate::otlp::random_span_id());
    aip_execution::trace_context::scope(async move {
        aip_execution::trace_context::set(trace_ctx);
        execute_inner(rt, facts, wire, lease).await;
    })
    .await;
}

fn export_job_span(rt: &Runtime, lease: &Lease, state: &str, code: Option<&str>, started: std::time::Instant) {
    export_job_execution_span(rt, lease, &lease.operation, 0, state, code, started);
}

fn export_job_execution_span(
    rt: &Runtime,
    lease: &Lease,
    operation: &str,
    depth: usize,
    state: &str,
    code: Option<&str>,
    started: std::time::Instant,
) {
    if let Some(otlp) = &rt.infrastructure.otlp_exporter {
        let end_time_nanos = crate::otlp::now_nanos();
        let duration_nanos = (started.elapsed().as_nanos() as u64).min(end_time_nanos);
        let start_time_nanos = end_time_nanos.saturating_sub(duration_nanos);
        let status_code = crate::otlp::SpanStatusCode::from_job_state(state);
        let mut attributes = vec![
            ("job.operation".to_string(), serde_json::Value::String(operation.to_owned())),
            ("job.id".to_string(), serde_json::Value::String(lease.run_id.to_string())),
            ("job.depth".to_string(), serde_json::json!(depth)),
            ("job.epoch".to_string(), serde_json::json!(lease.epoch)),
            ("job.state".to_string(), serde_json::Value::String(state.to_string())),
        ];
        if let Some(c) = code {
            attributes.push(("aip.code".to_string(), serde_json::Value::String(c.to_string())));
        }
        // The active execution scope holds this span and its actual parent: an HTTP request or a containing job step.
        let context = aip_execution::trace_context::current().unwrap_or_else(aip_execution::trace_context::TraceContext::generate);
        let span = crate::otlp::SpanData {
            trace_id: context.trace_id().to_string(),
            span_id: context.span_id().map_or_else(crate::otlp::random_span_id, str::to_owned),
            parent_span_id: context.parent_span_id().map(str::to_owned),
            name: format!("JOB {operation}"),
            kind: 1,
            start_time_nanos,
            end_time_nanos,
            attributes,
            status_code,
            status_message: code.map(ToString::to_string),
        };
        otlp.export_span(span);
    }
}

async fn execute_inner(rt: Arc<Runtime>, facts: Arc<Value>, wire: IdWire, lease: Lease) {
    let started = std::time::Instant::now();
    let Ok(store) = schema(&rt).and_then(|s| Store::new(s).map_err(store_error)) else { return };
    let grant: DurablePrincipal = match serde_json::from_value(lease.grant.clone()) {
        Ok(g) => g,
        Err(_) => return,
    };
    let admitted = async {
        let mut db = connect(&rt).await?;
        let tx = db.transaction().await.map_err(db_error)?;
        guard(&tx, &rt, &grant).await?;
        locked_lease(&tx, &rt, &lease).await?;
        if lease.deployment != rt.deployment.as_ref().unwrap().expected_digest || lease.implementation != bundle(&rt)? {
            return Err("WIRE.IMPLEMENTATION_CHANGED");
        }
        let caller = authorize(&tx, &facts, &lease.operation, &lease.input, lease.actor, wire).await?;
        tx.commit().await.map_err(db_error)?;
        db.release().await.map_err(db_error)?;
        Ok(caller)
    }
    .await;
    let caller = match admitted {
        Ok(c) => c,
        Err(code) => {
            let state = if code == "WIRE.DB_UNAVAILABLE" { "RETRY" } else { "FAILED" };
            crate::metrics::observe_job(&lease.operation, state, Some(code), started.elapsed());
            export_job_span(&rt, &lease, state, Some(code), started);
            if let Ok(db) = connect(&rt).await {
                let result = if code == "WIRE.DB_UNAVAILABLE" { store.retry(&*db, &lease, code).await } else { store.fail(&*db, &lease, code).await };
                if result.is_ok() {
                    let _ = db.release().await;
                }
            }
            return;
        }
    };
    let config = rt.extensions.as_ref().unwrap();
    let mut worker =
        match spike_v4_worker::Worker::try_start_configured(config.lang, config.dir.to_str().unwrap(), rt.infrastructure.worker_launch.clone()).await
        {
            Ok(w) => w,
            Err(_) => {
                crate::metrics::observe_job(&lease.operation, "RETRY", Some("WIRE.WORKER_FAILED"), started.elapsed());
                export_job_span(&rt, &lease, "RETRY", Some("WIRE.WORKER_FAILED"), started);
                if let Ok(db) = connect(&rt).await {
                    if store.retry(&*db, &lease, "WIRE.WORKER_FAILED").await.is_ok() {
                        let _ = db.release().await;
                    }
                }
                return;
            }
        };
    let mut host = Host {
        rt: rt.clone(),
        facts: facts.clone(),
        wire,
        lease: lease.clone(),
        grant: grant.clone(),
        prefix: String::new(),
        depth: 0,
        budget: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        capabilities: facts["operations"][&lease.operation]["access"].as_object().unwrap().keys().cloned().collect(),
    };
    // The wait `ctx.retryLater(reason, {afterMs})` asked for; the store bounds it.
    let mut retry_after_ms: Option<i64> = None;
    let result = {
        let future = spike_v4_worker::invoke_job(&mut worker, &facts, &lease.operation, &lease.input, &caller, wire, &mut host);
        tokio::pin!(future);
        let monitor = async {
            let mut timer = tokio::time::interval(Duration::from_millis(250));
            let mut last_beat = tokio::time::Instant::now();
            loop {
                timer.tick().await;
                let beat = async {
                    let mut db = connect(&rt).await?;
                    let tx = db.transaction().await.map_err(db_error)?;
                    guard(&tx, &rt, &grant).await?;
                    if !store.heartbeat(&tx, &lease).await.map_err(store_error)? {
                        return Err::<(), _>("WIRE.LEASE_LOST");
                    }
                    tx.commit().await.map_err(db_error)?;
                    db.release().await.map_err(db_error)?;
                    Ok(())
                }
                .await;
                match beat {
                    Ok(()) => last_beat = tokio::time::Instant::now(),
                    // A migration holding the deployment lock, a saturated pool or a restarting database fails single
                    // heartbeats. The lease lasts 30s, so keep the worker alive and try again until the last good beat
                    // is old enough that the lease could be at risk; only then is the job given back for a retry.
                    Err("WIRE.DB_UNAVAILABLE") if last_beat.elapsed() < HEARTBEAT_OUTAGE_TOLERANCE => {}
                    Err(code) => return Err::<(), &'static str>(code),
                }
            }
        };
        tokio::pin!(monitor);
        // Poll both futures while heartbeat waits for a capability transaction's row lock.
        tokio::select! {
            result=&mut future=>result.map_err(|e| { retry_after_ms = spike_v4_worker::retry_after_ms(&e); e.code }),
            result=&mut monitor=>result.and(Err("WIRE.LEASE_LOST")),
        }
    };
    worker.stop().await;
    let Ok(mut db) = connect(&rt).await else { return };
    let Ok(tx) = db.transaction().await else { return };
    if tx.batch_execute("SET LOCAL lock_timeout='2s'; SET LOCAL statement_timeout='5s'").await.is_err() {
        return;
    }
    if tx.query_one("SELECT pg_advisory_xact_lock_shared(1095323725,hashtext($1))", &[&schema(&rt).unwrap()]).await.is_err() {
        return;
    }
    // Only an observed process stop can acknowledge running cancellation.
    let s = schema(&rt).unwrap();
    if let Ok(Some(row)) = tx
        .query_opt(
            &format!("SELECT state FROM {s}.aip_jobs WHERE id=$1 AND lease_epoch=$2 AND lease_owner=$3 FOR UPDATE"),
            &[&lease.run_id, &lease.epoch, &lease.worker],
        )
        .await
    {
        if row.get::<_, String>(0) == "CANCEL_REQUESTED" {
            let pending = tx
                .query_one(
                    &format!("SELECT EXISTS(SELECT 1 FROM {s}.aip_effects WHERE run_id=$1 AND state IN ('PENDING','UNKNOWN'))"),
                    &[&lease.run_id],
                )
                .await;
            let updated = match pending {
                Ok(row) if !row.get::<_, bool>(0) => {
                    crate::metrics::observe_job(&lease.operation, "CANCELLED", None, started.elapsed());
                    export_job_span(&rt, &lease, "CANCELLED", None, started);
                    store.acknowledge_cancel(&tx, &lease).await
                }
                Ok(_) => {
                    crate::metrics::observe_job(&lease.operation, "FAILED", Some("WIRE.CANCELLATION_UNCONFIRMED"), started.elapsed());
                    export_job_span(&rt, &lease, "FAILED", Some("WIRE.CANCELLATION_UNCONFIRMED"), started);
                    store.reject_cancel(&tx, &lease, "WIRE.CANCELLATION_UNCONFIRMED").await
                }
                Err(_) => return,
            };
            if !matches!(updated, Ok(true)) {
                eprintln!("{{\"event\":\"job_cancel_unconfirmed\",\"id\":\"{}\"}}", lease.run_id);
            }

            let _ = tx.commit().await;
            let _ = db.release().await;
            return;
        }
    }
    let checked = async {
        guard(&tx, &rt, &grant).await?;
        locked_lease(&tx, &rt, &lease).await?;
        if bundle(&rt)? != lease.implementation {
            return Err("WIRE.IMPLEMENTATION_CHANGED");
        }
        authorize(&tx, &facts, &lease.operation, &lease.input, lease.actor, wire).await?;
        Ok(())
    }
    .await;
    let settled = async {
        checked?;
        let pending: bool = tx
            .query_one(&format!("SELECT EXISTS(SELECT 1 FROM {s}.aip_effects WHERE run_id=$1 AND state IN ('PENDING','UNKNOWN'))"), &[&lease.run_id])
            .await
            .map_err(db_error)?
            .get(0);
        if pending {
            return Err("WIRE.EFFECT_UNKNOWN");
        }
        result
    }
    .await;
    let (job_state, job_code) = match &settled {
        Ok(_) => ("SUCCEEDED", None),
        Err(code) if retryable(&rt, &lease.operation, code) => ("RETRY", Some(*code)),
        Err(code) => ("FAILED", Some(*code)),
    };
    crate::metrics::observe_job(&lease.operation, job_state, job_code, started.elapsed());
    export_job_span(&rt, &lease, job_state, job_code, started);
    let updated = match settled {
        Ok(out) => store.finish(&tx, &lease, out).await,
        Err(code) if retryable(&rt, &lease.operation, code) => store.retry_after(&tx, &lease, code, retry_after_ms).await,
        Err(code) => store.fail(&tx, &lease, code).await,
    };
    if !matches!(updated, Ok(true)) {
        eprintln!("{{\"event\":\"job_outcome_rejected\",\"id\":\"{}\",\"epoch\":{}}}", lease.run_id, lease.epoch);
    }
    if tx.commit().await.is_err() {
        eprintln!("{{\"event\":\"job_outcome_commit_unknown\",\"id\":\"{}\"}}", lease.run_id);
    } else {
        let _ = db.release().await;
    }
}

struct Host {
    rt: Arc<Runtime>,
    facts: Arc<Value>,
    wire: IdWire,
    lease: Lease,
    grant: DurablePrincipal,
    prefix: String,
    depth: usize,
    budget: Arc<std::sync::atomic::AtomicUsize>,
    capabilities: Vec<String>,
}
impl spike_v4_worker::JobHost for Host {
    fn call<'a>(
        &'a mut self,
        op: &'a str,
        name: &'a str,
        input: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::result::Result<Value, Reject>> + Send + 'a>> {
        Box::pin(async move { self.dispatch(op, name, input).await.map_err(|code| Reject { code, msg: "managed capability 호출 거부".into() }) })
    }
    fn job(&self) -> Value {
        json!({"id": self.lease.run_id.to_string(), "attempt": self.lease.attempt})
    }
}
impl Host {
    async fn dispatch(&mut self, op: &str, name: &str, input: &Value) -> Result<Value> {
        if self.depth > 8 || self.budget.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 64 {
            return Err("WIRE.COST_EXCEEDED");
        }
        if !self.capabilities.contains(&spike_v4_worker::job_capability(op, name)) {
            return Err("WIRE.ACCESS_NOT_DECLARED");
        }
        let read_only = op == "read" || op == "aggregate" || op == "files" && input["action"] == "get";
        let suffix = if read_only {
            String::new()
        } else {
            let key = text(input, "key")?;
            if !crate::valid_key(key) {
                return Err("WIRE.BAD_VALUE");
            }
            spike_v1_fixture::digest(&json!(key))
        };
        let effect_key = format!("{}{op}.{name}:{suffix}", self.prefix);
        let s = schema(&self.rt)?.to_string();
        let journal = crate::encryption::EffectJournal::new(&self.facts).map_err(|e| e.code)?;
        let effect = aip_execution::encrypted_journal::Effect { kind: op, name, input, implementation: &self.lease.implementation };
        let journal_key = format!("{}:{effect_key}", self.lease.run_id);
        let journal_context = aip_execution::encrypted_journal::Context { schema: &s, principal: &self.lease.owner, request_key: &journal_key };
        let digest = journal.marker(&effect, &journal_context).map_err(|e| e.code)?;
        let mut db = connect(&self.rt).await?;
        let tx = db.transaction().await.map_err(db_error)?;
        guard(&tx, &self.rt, &self.grant).await?;
        locked_lease(&tx, &self.rt, &self.lease).await?;
        let caller = authorize(&tx, &self.facts, &self.lease.operation, &self.lease.input, self.lease.actor, self.wire).await?;
        if bundle(&self.rt)? != self.lease.implementation {
            return Err("WIRE.IMPLEMENTATION_CHANGED");
        }
        if op == "read" || op == "aggregate" {
            // Reads have no effect journal. They run as the job principal with the same planner and budgets as /read.
            tx.commit().await.map_err(db_error)?;
            let output = data_read(&mut db, &self.facts, op, name, input, &caller, self.wire).await;
            db.release().await.map_err(db_error)?;
            return output;
        }
        if op == "jobs" {
            authorize(&tx, &self.facts, name, &input["input"], self.lease.actor, self.wire).await?;
        }
        if op == "files" && text(input, "action")? == "get" {
            let id = text(input, "id")?.parse().map_err(|_| "WIRE.BAD_VALUE")?;
            let f = aip_execution::files::Store::new(&s)
                .map_err(db_error)?
                .get(&tx, &self.lease.owner, &id)
                .await
                .map_err(db_error)?
                .filter(|f| f.bucket == name)
                .ok_or("WIRE.NOT_FOUND")?;
            let output = json!({"file":file_json(&f),"base64":B64.encode(&f.bytes)});
            tx.commit().await.map_err(db_error)?;
            db.release().await.map_err(db_error)?;
            return Ok(output);
        }
        if let Some(row) = tx
            .query_opt(
                &format!("SELECT request_digest,state,result FROM {s}.aip_effects WHERE run_id=$1 AND effect_key=$2"),
                &[&self.lease.run_id, &effect_key],
            )
            .await
            .map_err(db_error)?
        {
            if !journal.matches(&effect, &journal_context, &row.get::<_, String>(0)).map_err(|e| e.code)? {
                return Err("WIRE.IDEMPOTENCY_MISMATCH");
            }
            if row.get::<_, String>(1) == "SUCCEEDED" {
                let output = row.get(2);
                tx.commit().await.map_err(db_error)?;
                db.release().await.map_err(db_error)?;
                return Ok(output);
            }
            if row.get::<_, String>(1) == "FAILED" {
                let code = row.get::<_, Value>(2)["code"].as_str().unwrap_or("").to_string();
                return Err(if op == "apply" { data_error(&code) } else { step_error(&code) });
            }
            // A webhook subscriber dedupes on webhook-id, so a delivery whose outcome was lost is sent again.
            if op == "http" && !self.rt.broker.as_ref().is_some_and(|broker| broker.is_webhook(name)) {
                return Err("WIRE.EFFECT_UNKNOWN");
            }
        }
        tx.execute(
            &format!("INSERT INTO {s}.aip_effects(run_id,effect_key,request_digest,state) VALUES($1,$2,$3,'PENDING') ON CONFLICT DO NOTHING"),
            &[&self.lease.run_id, &effect_key, &digest],
        )
        .await
        .map_err(db_error)?;
        let output = match op {
            "apply" => {
                let request = spike_v4_worker::write::job_apply_request(name, input, self.wire).map_err(|e| e.code)?;
                let knobs = spike_v3_write::Knobs {
                    audit_via: Some("job"),
                    audit_key: Some(format!("{}:{effect_key}", self.lease.run_id)),
                    fresh_now_after_lock: true,
                    internal: true,
                    ..spike_v3_write::Knobs::default()
                };
                tx.batch_execute("SET LOCAL TimeZone='UTC'").await.map_err(db_error)?;
                // The transition, deferred checks and the effect journal commit in one transaction.
                let applied = match spike_v3_write::apply_in_with_wire(&tx, &self.facts, &request, &caller, &knobs, self.wire).await {
                    Ok(applied) => spike_v3_write::checks_in(&tx, &self.facts, &caller, &knobs).await.map(|_| applied),
                    Err(e) => Err(e),
                };
                let applied = match applied {
                    Ok(applied) => applied,
                    Err(e) => {
                        drop(tx);
                        return self.reject_data_effect(db, &effect_key, &digest, e.code).await;
                    }
                };
                let encode = |ids: Vec<spike_v2_read::id_wire::EntityId>| {
                    ids.into_iter().map(|id| spike_v2_read::id_wire::emit_entity_id(id, self.wire)).collect::<std::result::Result<Vec<_>, _>>()
                };
                json!({
                    "changed": encode(applied.changed).map_err(|e| e.code)?,
                    "unchanged": encode(applied.unchanged).map_err(|e| e.code)?
                })
            }
            "messages" => {
                if !self.rt.infrastructure.messaging {
                    return Err("WIRE.NOT_EXPOSED");
                }
                let key = spike_v1_fixture::digest(&json!([self.lease.run_id.to_string(), self.prefix, text(input, "key")?]));
                crate::messaging_transport::publish(&tx, &s, &self.lease.owner, name, &key, input["payload"].clone(), 0, 86_400_000, 3).await?
            }
            "files" => {
                let files = aip_execution::files::Store::new(&s).map_err(db_error)?;
                match text(input, "action")? {
                    "put" => {
                        let file = put_file(&tx, &s, &self.lease.owner, name, input, Some(&self.lease), &effect_key).await?;
                        file_json(&file)
                    }
                    "get" => {
                        let id = text(input, "id")?.parse().map_err(|_| "WIRE.BAD_VALUE")?;
                        let f =
                            files.get(&tx, &self.lease.owner, &id).await.map_err(db_error)?.filter(|f| f.bucket == name).ok_or("WIRE.NOT_FOUND")?;
                        json!({"file":file_json(&f),"base64":B64.encode(&f.bytes)})
                    }
                    _ => return Err("WIRE.BAD_REQUEST"),
                }
            }
            "events" => {
                let key = text(input, "key")?;
                if !crate::valid_key(key) {
                    return Err("WIRE.BAD_VALUE");
                }
                let key = spike_v1_fixture::digest(&json!([self.lease.run_id.to_string(), self.prefix, key]));
                let cursor = aip_execution::events::Store::new(&s)
                    .map_err(db_error)?
                    .publish_in(&tx, &self.lease.owner, name, &key, input["payload"].clone())
                    .await
                    .map_err(|e| {
                        if matches!(e, aip_execution::events::Error::IdempotencyConflict) {
                            "WIRE.IDEMPOTENCY_MISMATCH"
                        } else {
                            "WIRE.EVENT_INVALID"
                        }
                    })?;
                json!({"cursor":cursor})
            }
            "http" => {
                let broker = self.rt.broker.as_ref().ok_or("WIRE.NOT_EXPOSED")?;
                let webhook = broker.is_webhook(name).then(|| text(input, "url")).transpose()?;
                tx.commit().await.map_err(db_error)?;
                db.release().await.map_err(db_error)?;
                // Err(Some(code)) is a certain not-sent (or safely resendable) outcome; Err(None) is unknown.
                let response: std::result::Result<Value, Option<&'static str>> = if let Some(url) = webhook {
                    use aip_execution::integrations::Error as E;
                    // Stable across attempts, so the subscriber's dedupe on webhook-id absorbs a resend.
                    let id = format!("msg_{}", spike_v1_fixture::digest(&json!([self.lease.run_id.to_string(), effect_key])));
                    match broker.deliver_webhook(name, url, &input["input"], &id).await {
                        Ok(status) if (200..300).contains(&status) => Ok(json!({"status":status,"id":id})),
                        Err(E::DestinationRejected) => Err(Some("WIRE.DESTINATION_REJECTED")),
                        Err(E::RequestTooLarge) => Err(Some("WIRE.BAD_VALUE")),
                        Ok(_) | Err(_) => Err(Some("WIRE.DELIVERY_FAILED")),
                    }
                } else {
                    let key = format!("{}:{effect_key}", self.lease.run_id);
                    match broker.call_with_key(name, &input["input"], Some(&key)).await {
                        Ok(r) => Ok(json!({"status":r.status,"body":r.body,"retryAfterSeconds":r.retry_after_seconds})),
                        Err(aip_execution::integrations::Error::CircuitOpen) => Err(Some("WIRE.CIRCUIT_OPEN")),
                        Err(aip_execution::integrations::Error::NotSent) => Err(Some("WIRE.DELIVERY_FAILED")),
                        Err(aip_execution::integrations::Error::TokenUnavailable) => Err(Some("WIRE.INTEGRATION_AUTH_FAILED")),
                        Err(_) => Err(None),
                    }
                };
                let mut db = connect(&self.rt).await?;
                let tx = db.transaction().await.map_err(db_error)?;
                guard(&tx, &self.rt, &self.grant).await?;
                locked_lease(&tx, &self.rt, &self.lease).await?;
                let result = match response {
                    Ok(result) => result,
                    Err(Some(code)) => {
                        // Drop the intent instead of leaving an unknown effect; the extension can catch the code or
                        // throw ctx.retryLater() and send the same key again.
                        tx.execute(
                            &format!("DELETE FROM {s}.aip_effects WHERE run_id=$1 AND effect_key=$2 AND state='PENDING'"),
                            &[&self.lease.run_id, &effect_key],
                        )
                        .await
                        .map_err(db_error)?;
                        tx.commit().await.map_err(db_error)?;
                        db.release().await.map_err(db_error)?;
                        return Err(code);
                    }
                    Err(None) => {
                        tx.execute(
                            &format!("UPDATE {s}.aip_effects SET state='UNKNOWN' WHERE run_id=$1 AND effect_key=$2"),
                            &[&self.lease.run_id, &effect_key],
                        )
                        .await
                        .map_err(db_error)?;
                        tx.commit().await.map_err(db_error)?;
                        return Err("WIRE.EFFECT_UNKNOWN");
                    }
                };
                tx.execute(
                    &format!("UPDATE {s}.aip_effects SET state='SUCCEEDED',result=$3 WHERE run_id=$1 AND effect_key=$2"),
                    &[&self.lease.run_id, &effect_key, &result],
                )
                .await
                .map_err(db_error)?;
                tx.commit().await.map_err(db_error)?;
                db.release().await.map_err(db_error)?;
                return Ok(result);
            }
            "jobs" => {
                if self.depth >= 8 {
                    return Err("WIRE.COST_EXCEEDED");
                }
                let child_caps = self.facts["operations"][name]["access"].as_object().ok_or("WIRE.NOT_EXPOSED")?.keys().cloned().collect::<Vec<_>>();
                if child_caps.iter().any(|cap| !self.capabilities.contains(cap)) {
                    return Err("WIRE.ACCESS_NOT_DECLARED");
                }
                if !crate::valid_key(text(input, "key")?) {
                    return Err("WIRE.BAD_VALUE");
                }
                let caller = authorize(&tx, &self.facts, name, &input["input"], self.lease.actor, self.wire).await?;
                tx.commit().await.map_err(db_error)?;
                db.release().await.map_err(db_error)?;
                let config = self.rt.extensions.as_ref().unwrap();
                let mut worker = spike_v4_worker::Worker::try_start_configured(
                    config.lang,
                    config.dir.to_str().unwrap(),
                    self.rt.infrastructure.worker_launch.clone(),
                )
                .await
                .map_err(|_| "WIRE.WORKER_FAILED")?;
                let mut child = Host {
                    rt: self.rt.clone(),
                    facts: self.facts.clone(),
                    wire: self.wire,
                    lease: self.lease.clone(),
                    grant: self.grant.clone(),
                    prefix: format!("{effect_key}/"),
                    depth: self.depth + 1,
                    budget: self.budget.clone(),
                    capabilities: child_caps,
                };
                let parent = aip_execution::trace_context::current().unwrap_or_else(aip_execution::trace_context::TraceContext::generate);
                let child_trace = parent.child(crate::otlp::random_span_id());
                let started = std::time::Instant::now();
                let out = aip_execution::trace_context::scope(async {
                    aip_execution::trace_context::set(child_trace);
                    let out = if self.facts["operations"][name]["kind"] == "read" {
                        spike_v4_worker::invoke_operation(&mut worker, &self.facts, name, &input["input"], &caller, self.wire).await
                    } else {
                        spike_v4_worker::invoke_job(&mut worker, &self.facts, name, &input["input"], &caller, self.wire, &mut child).await
                    };
                    let (state, code) = match &out {
                        Ok(_) => ("SUCCEEDED", None),
                        Err(error) => ("FAILED", Some(error.code)),
                    };
                    // Nested steps share the durable lease/id but have their own span and active parent context.
                    export_job_execution_span(&self.rt, &self.lease, name, self.depth + 1, state, code, started);
                    out
                })
                .await;
                worker.stop().await;
                let mut db = connect(&self.rt).await?;
                let tx = db.transaction().await.map_err(db_error)?;
                guard(&tx, &self.rt, &self.grant).await?;
                locked_lease(&tx, &self.rt, &self.lease).await?;
                authorize(&tx, &self.facts, name, &input["input"], self.lease.actor, self.wire).await?;
                let (state, value) = match &out {
                    Ok(value) => ("SUCCEEDED", value.clone()),
                    Err(e) => ("FAILED", json!({"code":e.code})),
                };
                tx.execute(
                    &format!("UPDATE {s}.aip_effects SET state=$3,result=$4 WHERE run_id=$1 AND effect_key=$2"),
                    &[&self.lease.run_id, &effect_key, &state, &value],
                )
                .await
                .map_err(db_error)?;
                tx.commit().await.map_err(db_error)?;
                db.release().await.map_err(db_error)?;
                return out.map_err(|e| data_error(e.code));
            }
            _ => return Err("WIRE.ACCESS_NOT_DECLARED"),
        };
        tx.execute(
            &format!("UPDATE {s}.aip_effects SET state='SUCCEEDED',result=$3 WHERE run_id=$1 AND effect_key=$2"),
            &[&self.lease.run_id, &effect_key, &output],
        )
        .await
        .map_err(db_error)?;
        commit_effect(tx).await?;
        db.release().await.map_err(db_error)?;
        if op == "apply" {
            self.rt.infrastructure.data_changes.notify();
        }
        Ok(output)
    }
}

impl Host {
    /// Records a deterministic business rejection so a retried attempt replays it instead of re-running the transition.
    /// Transient failures leave no journal row because the rolled-back transaction changed nothing.
    async fn reject_data_effect(&self, db: crate::pool::PooledConnection, effect_key: &str, digest: &str, code: &'static str) -> Result<Value> {
        drop(db);
        if matches!(code, "WIRE.CONFLICT" | "WIRE.DEADLINE_EXCEEDED" | "WIRE.INTERNAL" | "WIRE.DB_UNAVAILABLE") {
            return Err(if code == "WIRE.CONFLICT" { "WIRE.CONFLICT" } else { "WIRE.DB_UNAVAILABLE" });
        }
        let s = schema(&self.rt)?.to_string();
        let mut db = connect(&self.rt).await?;
        let tx = db.transaction().await.map_err(db_error)?;
        guard(&tx, &self.rt, &self.grant).await?;
        locked_lease(&tx, &self.rt, &self.lease).await?;
        tx.execute(
            &format!(
                "INSERT INTO {s}.aip_effects(run_id,effect_key,request_digest,state,result) VALUES($1,$2,$3,'FAILED',$4) ON CONFLICT DO NOTHING"
            ),
            &[&self.lease.run_id, &effect_key, &digest, &json!({"code":code})],
        )
        .await
        .map_err(db_error)?;
        tx.commit().await.map_err(db_error)?;
        db.release().await.map_err(db_error)?;
        Err(data_error(code))
    }
}

async fn data_read(db: &mut Client, facts: &Value, op: &str, name: &str, input: &Value, caller: &Caller, wire: IdWire) -> Result<Value> {
    let request = if op == "aggregate" {
        if !input.is_object() {
            return Err("WIRE.BAD_VALUE");
        }
        json!({"aggregate": name, "input": input})
    } else {
        if input["read"].as_str() != Some(name) {
            return Err("WIRE.BAD_VALUE");
        }
        input.clone()
    };
    let plan = spike_v2_read::plan::plan_read_with_wire(facts, &request, caller, wire).map_err(|e| data_error(e.code))?;
    let rows = match tokio::time::timeout(Duration::from_millis(plan.deadline_ms), spike_v2_read::execute(db, &plan)).await {
        Ok(rows) => rows.map_err(|e| data_error(e.code))?,
        Err(_) => return Err("WIRE.DEADLINE_EXCEEDED"),
    };
    if op == "aggregate" {
        return Ok(json!({"value": rows.first().cloned().unwrap_or(Value::Null)}));
    }
    let rows = spike_v2_read::id_wire::encode_rows(rows, &plan.output_type, wire).map_err(|e| data_error(e.code))?;
    Ok(json!({"rows": rows}))
}

#[cfg(test)]
mod safety_tests {
    use super::*;

    /// 서버가 끝낸 backend의 연결로 열린 트랜잭션을 돌려준다. 클라이언트가 닫힘을 알아챈 뒤에만 돌려주므로 COMMIT은 나갈 수 없다.
    async fn transaction_on_a_terminated_backend(victim: &mut tokio_postgres::Client) -> tokio_postgres::Transaction<'_> {
        let control = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
        let tx = victim.transaction().await.unwrap();
        let pid: i32 = tx.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
        control.execute("SELECT pg_terminate_backend($1)", &[&pid]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !tx.client().is_closed() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("client must notice that its backend was terminated");
        tx
    }

    /// 이미 닫힌 연결에는 COMMIT이 나가지 못한다. 서버는 끊긴 연결의 열린 트랜잭션을 rollback하므로 결과가 불확실하지 않다.
    #[tokio::test]
    async fn commit_request_on_an_already_closed_connection_is_db_unavailable() {
        let mut victim = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
        let tx = transaction_on_a_terminated_backend(&mut victim).await;
        assert_eq!(commit_request(tx).await, Err("WIRE.DB_UNAVAILABLE"), "COMMIT never left the client, so the outcome is known");
    }

    #[tokio::test]
    async fn commit_effect_on_an_already_closed_connection_is_db_unavailable() {
        let mut victim = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
        let tx = transaction_on_a_terminated_backend(&mut victim).await;
        assert_eq!(commit_effect(tx).await, Err("WIRE.DB_UNAVAILABLE"), "COMMIT never left the client, so a job retry is safe");
    }

    #[tokio::test]
    async fn commit_request_that_succeeds_is_ok() {
        let mut db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
        let tx = db.transaction().await.unwrap();
        tx.batch_execute("SELECT 1").await.unwrap();
        assert_eq!(commit_request(tx).await, Ok(()));
    }

    #[test]
    fn message_gc_requires_enabled_infrastructure_and_declared_access() {
        let active = json!({"operations":{"dispatch":{"kind":"job","access":{"messages.notifications":true}}}});
        let inactive = json!({"operations":{"read":{"kind":"query","access":{"messages.notifications":true}}}});
        assert!(should_cleanup_messages(true, &active));
        assert!(!should_cleanup_messages(false, &active));
        assert!(!should_cleanup_messages(true, &inactive));
        assert!(!should_cleanup_messages(true, &json!({"operations":{}})));
    }

    #[tokio::test]
    async fn collector_removes_payloads_files_and_old_tombstones() {
        let s = format!("aip_job_gc_{}", std::process::id());
        let mut db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
        db.batch_execute(&format!(
            "CREATE SCHEMA {s}; {}; {}; {}; CREATE TABLE {s}.aip_effects(run_id uuid REFERENCES {s}.aip_jobs(id),result jsonb)",
            aip_execution::ddl(&s).unwrap(),
            aip_execution::files::Store::new(&s).unwrap().ddl(),
            aip_execution::messages::ddl(&s).unwrap()
        ))
        .await
        .unwrap();
        let store = Store::new(&s).unwrap();
        let mut ids = vec![];
        for key in ["expired", "old", "active"] {
            let run = store
                .enqueue(
                    &*db,
                    NewRun {
                        owner: "owner".into(),
                        actor: 1,
                        grant: json!({"subject":"private"}),
                        operation: "count".into(),
                        contract: "c".into(),
                        deployment: "d".into(),
                        implementation: "i".into(),
                        input: json!({"text":"private"}),
                        key: key.into(),
                        timeout_ms: 1000,
                        max_attempts: 1,
                        ttl_ms: 1000,
                        delay_ms: 0,
                        trace_id: None,
                    },
                )
                .await
                .unwrap();
            ids.push(run.id);
        }
        db.batch_execute(&format!("UPDATE {s}.aip_jobs SET state='SUCCEEDED',output='{{\"secret\":true}}',result_expires_at=clock_timestamp()-interval '1 second' WHERE idempotency_key='expired'; UPDATE {s}.aip_jobs SET state='FAILED',result_expires_at=clock_timestamp()-interval '31 days' WHERE idempotency_key='old'; INSERT INTO {s}.aip_effects SELECT id,'{{\"secret\":true}}' FROM {s}.aip_jobs; INSERT INTO {s}.aip_files(handle,owner,bucket,name,media_type,content,expires_at) VALUES(md5('file')::uuid,'owner','reports','old','text/plain','private',clock_timestamp()-interval '1 second')")).await.unwrap();
        let message_store = aip_execution::messages::MessageStore::new(&s).unwrap();
        message_store
            .enqueue(
                &*db,
                aip_execution::messages::NewMessage {
                    owner: "owner".into(),
                    queue: "notifications".into(),
                    key: "expired-message".into(),
                    payload: json!({"private":true}),
                    max_deliveries: 3,
                    ttl_ms: 60_000,
                    delay_ms: 0,
                },
            )
            .await
            .unwrap();
        db.execute(&format!("UPDATE {s}.aip_messages SET expires_at=clock_timestamp()-interval '1 second'"), &[]).await.unwrap();
        collect_expired(&mut db, &s, &Sweep { messages: false, retention: Default::default(), handled_topics: &[], events: false }).await.unwrap();
        assert_eq!(db.query_one(&format!("SELECT count(*) FROM {s}.aip_messages"), &[]).await.unwrap().get::<_, i64>(0), 1);
        collect_expired(&mut db, &s, &Sweep { messages: true, retention: Default::default(), handled_topics: &[], events: false }).await.unwrap();
        assert_eq!(db.query_one(&format!("SELECT count(*) FROM {s}.aip_messages"), &[]).await.unwrap().get::<_, i64>(0), 0);
        let rows =
            db.query(&format!("SELECT idempotency_key,input,grant_data,output FROM {s}.aip_jobs ORDER BY idempotency_key"), &[]).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get::<_, String>(0), "active");
        assert_eq!(rows[0].get::<_, Value>(1), json!({"text":"private"}));
        assert_eq!(rows[1].get::<_, Value>(1), json!({}));
        assert_eq!(rows[1].get::<_, Value>(2), json!({}));
        assert!(rows[1].get::<_, Option<Value>>(3).is_none());
        assert_eq!(db.query_one(&format!("SELECT count(*) FROM {s}.aip_effects WHERE result IS NOT NULL"), &[]).await.unwrap().get::<_, i64>(0), 1);
        assert_eq!(db.query_one(&format!("SELECT count(*) FROM {s}.aip_files"), &[]).await.unwrap().get::<_, i64>(0), 0);
        assert!(store.status(&*db, &ids[1], "owner").await.unwrap().is_none());
        db.batch_execute(&format!("DROP SCHEMA {s} CASCADE")).await.unwrap();
    }

    #[tokio::test]
    async fn managed_files_enforce_total_quota_and_reclaim_expired_content() {
        let s = format!("aip_file_quota_{}", std::process::id());
        let mut db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
        db.batch_execute(&format!("CREATE SCHEMA {s}; {}", aip_execution::files::Store::new(&s).unwrap().ddl())).await.unwrap();
        db.batch_execute(&format!("INSERT INTO {s}.aip_files(handle,owner,bucket,name,media_type,content) SELECT md5(i::text)::uuid,'owner','reports',i::text,'text/plain',''::bytea FROM generate_series(1,100) i")).await.unwrap();
        let tx = db.transaction().await.unwrap();
        assert_eq!(file_budget(&tx, &s, "owner", "reports", "new", 1, "").await, Err("WIRE.QUOTA_EXCEEDED"));
        assert!(file_budget(&tx, &s, "owner", "reports", "1", 1, "").await.is_ok(), "a replay doesn't consume additional capacity");
        tx.commit().await.unwrap();
        db.batch_execute(&format!("UPDATE {s}.aip_files SET expires_at=clock_timestamp()-interval '1 second'")).await.unwrap();
        let tx = db.transaction().await.unwrap();
        assert!(file_budget(&tx, &s, "owner", "reports", "new", 1, "").await.is_ok());
        tx.commit().await.unwrap();
        db.batch_execute(&format!("DROP SCHEMA {s} CASCADE")).await.unwrap();
    }

    /// O-20: expired history of busy (service) principals piled up faster than the old 100-per-minute sweep removed it,
    /// and the dispatch ledger plus the outbox rows it covers were never removed at all.
    #[tokio::test]
    async fn collector_keeps_up_with_bulk_history_and_trims_the_outbox_ledger() {
        let s = format!("aip_job_gc_bulk_{}", std::process::id());
        let infra = aip_execution::infrastructure_schema(&s).unwrap();
        let mut db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
        db.batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {infra} CASCADE; CREATE SCHEMA {s}; CREATE SCHEMA {infra}; {}; {}; CREATE TABLE {s}.aip_effects(run_id uuid REFERENCES {s}.aip_jobs(id),result jsonb); \
             CREATE TABLE {s}.aip_outbox (id bigserial PRIMARY KEY, topic text NOT NULL, recipient_id bigint, source text NOT NULL, source_id bigint, created_at timestamptz NOT NULL DEFAULT now()); {}",
            aip_execution::ddl(&s).unwrap(),
            aip_execution::files::Store::new(&s).unwrap().ddl(),
            outbox_dispatch_ddl(&infra)
        ))
        .await
        .unwrap();
        let store = Store::new(&s).unwrap();
        for index in 0..250 {
            store
                .enqueue(
                    &*db,
                    NewRun {
                        owner: "service".into(),
                        actor: 1,
                        grant: json!({}),
                        operation: "count".into(),
                        contract: "c".into(),
                        deployment: "d".into(),
                        implementation: "i".into(),
                        input: json!({}),
                        key: format!("old-{index}"),
                        timeout_ms: 1000,
                        max_attempts: 1,
                        ttl_ms: 1000,
                        delay_ms: 0,
                        trace_id: None,
                    },
                )
                .await
                .unwrap();
        }
        // 250 outbox rows: 200 dispatched long ago, 20 dispatched recently, 30 still pending.
        db.batch_execute(&format!(
            "UPDATE {s}.aip_jobs SET state='SUCCEEDED',result_expires_at=clock_timestamp()-interval '31 days'; \
             INSERT INTO {s}.aip_effects SELECT id,NULL FROM {s}.aip_jobs; \
             INSERT INTO {s}.aip_outbox(topic,source,source_id,created_at) SELECT 't','x',i,clock_timestamp()-interval '40 days' FROM generate_series(1,250) i; \
             INSERT INTO {infra}.aip_outbox_dispatch(outbox_id,outbox_created_at,run_id,dispatched_at) \
               SELECT id,created_at,(SELECT id FROM {s}.aip_jobs LIMIT 1),CASE WHEN id<=200 THEN clock_timestamp()-interval '31 days' ELSE clock_timestamp()-interval '1 day' END FROM {s}.aip_outbox WHERE id<=220"
        ))
        .await
        .unwrap();
        collect_expired(&mut db, &s, &Sweep { messages: false, retention: Default::default(), handled_topics: &["t".to_owned()], events: false })
            .await
            .unwrap();
        let count = |table: String| {
            let db = &db;
            async move { db.query_one(&format!("SELECT count(*) FROM {table}"), &[]).await.unwrap().get::<_, i64>(0) }
        };
        assert_eq!(count(format!("{s}.aip_jobs")).await, 0, "one sweep must clear more than 100 expired jobs");
        assert_eq!(count(format!("{s}.aip_effects")).await, 0);
        assert_eq!(count(format!("{infra}.aip_outbox_dispatch")).await, 20, "recent dispatch rows stay for replay protection");
        assert_eq!(count(format!("{s}.aip_outbox")).await, 50, "undelivered and recently delivered rows stay");
        db.batch_execute(&format!("DROP SCHEMA {s} CASCADE; DROP SCHEMA {infra} CASCADE")).await.unwrap();
    }
}

#[cfg(test)]
mod owner_identity_tests {
    use super::owner;
    use crate::auth::DurablePrincipal;

    fn grant(min_iat: i64) -> DurablePrincipal {
        DurablePrincipal { issuer: "https://issuer.example".into(), subject: "user-1".into(), actor_id: 42, issued_at: min_iat + 10, min_iat }
    }

    /// Revoking a principal raises `min_iat`; binding the same subject to the same actor again must give the person back
    /// their own files, jobs and events, so the owner cannot depend on the revocation floor.
    #[test]
    fn owner_survives_revoke_and_rebind() {
        assert_eq!(owner(&grant(0)), owner(&grant(1_790_000_000)));
    }

    #[test]
    fn owner_still_separates_subjects_actors_and_issuers() {
        let base = grant(0);
        for other in [
            DurablePrincipal { subject: "user-2".into(), ..grant(0) },
            DurablePrincipal { actor_id: 43, ..grant(0) },
            DurablePrincipal { issuer: "https://other.example".into(), ..grant(0) },
        ] {
            assert_ne!(owner(&base), owner(&other));
        }
    }

    /// Data written before this change was stored under the digest of a principal that had never been revoked.
    #[test]
    fn owner_of_a_never_revoked_principal_is_unchanged() {
        let legacy = spike_v1_fixture::digest(&serde_json::json!(["https://issuer.example", "user-1", 42, 0]));
        assert_eq!(owner(&grant(0)), legacy);
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;

    const TABLES: &str = "CREATE TABLE {s}.aip_effects(run_id uuid REFERENCES {s}.aip_jobs(id),result jsonb); \
        CREATE TABLE {s}.aip_outbox (id bigserial PRIMARY KEY, topic text NOT NULL, recipient_id bigint, source text NOT NULL, source_id bigint, created_at timestamptz NOT NULL DEFAULT now())";

    struct Fixture {
        db: spike_v2_read::OwnedConnection,
        s: String,
        infra: String,
    }

    impl Fixture {
        async fn new(name: &str) -> Self {
            let s = format!("aip_retention_{name}_{}", std::process::id());
            let infra = aip_execution::infrastructure_schema(&s).unwrap();
            let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
            db.batch_execute(&format!(
                "DROP SCHEMA IF EXISTS {s} CASCADE; DROP SCHEMA IF EXISTS {infra} CASCADE; CREATE SCHEMA {s}; CREATE SCHEMA {infra}; {}; {}; {}; {}; {}",
                aip_execution::ddl(&s).unwrap(),
                aip_execution::files::Store::new(&s).unwrap().ddl(),
                aip_execution::events::Store::new(&s).unwrap().ddl(),
                TABLES.replace("{s}", &s),
                outbox_dispatch_ddl(&infra)
            ))
            .await
            .unwrap();
            Self { db, s, infra }
        }

        async fn count(&self, table: &str) -> i64 {
            self.db
                .query_one(&format!("SELECT count(*) FROM {}", table.replace("{s}", &self.s).replace("{infra}", &self.infra)), &[])
                .await
                .unwrap()
                .get(0)
        }

        async fn sweep(&mut self, retention: crate::infrastructure::Retention, handled: &[&str], events: bool) {
            let handled: Vec<String> = handled.iter().map(|topic| (*topic).to_owned()).collect();
            let sweep = Sweep { messages: false, retention, handled_topics: &handled, events };
            collect_expired(&mut self.db, &self.s, &sweep).await.unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let (s, infra) = (self.s.clone(), self.infra.clone());
            let _ = std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                runtime.block_on(async move {
                    let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.unwrap();
                    let _ = db.batch_execute(&format!("DROP SCHEMA IF EXISTS {s} CASCADE; DROP SCHEMA IF EXISTS {infra} CASCADE")).await;
                });
            })
            .join();
        }
    }

    fn retention(dispatched: u32, unhandled: Option<u32>, events: Option<u32>) -> crate::infrastructure::Retention {
        crate::infrastructure::Retention { outbox_dispatched_days: dispatched, outbox_unhandled_days: unhandled, events_days: events }
    }

    /// Four kinds of outbox rows, each aged 10 days: dispatched and unhandled and young ones differ only in what the sweep may do.
    async fn seed_outbox(fixture: &Fixture) {
        fixture
            .db
            .batch_execute(&format!(
                "INSERT INTO {s}.aip_outbox(topic,source,source_id,created_at) VALUES \
                   ('handled','x',1,clock_timestamp()-interval '10 days'), \
                   ('handled','x',2,clock_timestamp()-interval '10 days'), \
                   ('orphan','x',3,clock_timestamp()-interval '10 days'), \
                   ('orphan','x',4,clock_timestamp()-interval '1 day')",
                s = fixture.s
            ))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn unhandled_outbox_rows_are_kept_by_default_and_removed_by_age_when_configured() {
        let mut fixture = Fixture::new("unhandled").await;
        seed_outbox(&fixture).await;
        fixture.sweep(retention(30, None, None), &["handled"], false).await;
        assert_eq!(fixture.count("{s}.aip_outbox").await, 4, "without outbox_unhandled_days nothing is removed");

        fixture.sweep(retention(30, Some(5), None), &["handled"], false).await;
        assert_eq!(fixture.count("{s}.aip_outbox WHERE topic='orphan'").await, 1, "the 10-day-old orphan row goes, the 1-day-old one stays");
        assert_eq!(fixture.count("{s}.aip_outbox WHERE topic='handled'").await, 2, "undispatched rows of a handled topic are still owed to a job");

        fixture.sweep(retention(30, Some(1), None), &[], false).await;
        assert_eq!(fixture.count("{s}.aip_outbox").await, 0, "with no handlers every topic is unhandled; handled rows older than a day go too");
    }

    #[tokio::test]
    async fn unhandled_sweep_walks_in_id_order_and_stops_at_the_first_young_row() {
        let mut fixture = Fixture::new("walk").await;
        fixture
            .db
            .batch_execute(&format!(
                "INSERT INTO {s}.aip_outbox(topic,source,source_id,created_at) SELECT 'orphan','x',i,clock_timestamp()-interval '10 days' FROM generate_series(1,2500) i; \
                 INSERT INTO {s}.aip_outbox(topic,source,source_id,created_at) SELECT 'orphan','x',i,clock_timestamp() FROM generate_series(1,40) i",
                s = fixture.s
            ))
            .await
            .unwrap();
        fixture.sweep(retention(30, Some(5), None), &[], false).await;
        assert_eq!(fixture.count("{s}.aip_outbox").await, 40, "all 2,500 old rows go in one sweep (3 batches), the 40 young rows stay");
    }

    #[tokio::test]
    async fn dispatched_outbox_retention_is_configurable_and_never_touches_pending_rows() {
        let mut fixture = Fixture::new("dispatched").await;
        seed_outbox(&fixture).await;
        // Row 1 was dispatched 10 days ago; row 2 is still pending.
        let run = Store::new(&fixture.s)
            .unwrap()
            .enqueue(
                &*fixture.db,
                NewRun {
                    owner: "service".into(),
                    actor: 1,
                    grant: json!({}),
                    operation: "count".into(),
                    contract: "c".into(),
                    deployment: "d".into(),
                    implementation: "i".into(),
                    input: json!({}),
                    key: "dispatched".into(),
                    timeout_ms: 1000,
                    max_attempts: 1,
                    ttl_ms: 86_400_000,
                    delay_ms: 0,
                    trace_id: None,
                },
            )
            .await
            .unwrap();
        fixture
            .db
            .execute(
                &format!(
                    "INSERT INTO {}.aip_outbox_dispatch(outbox_id,outbox_created_at,run_id,dispatched_at) \
                     SELECT id,created_at,$1::text::uuid,clock_timestamp()-interval '10 days' FROM {}.aip_outbox WHERE topic='handled' AND source_id=1",
                    fixture.infra, fixture.s
                ),
                &[&run.id],
            )
            .await
            .unwrap();
        fixture.sweep(retention(30, None, None), &["handled"], false).await;
        assert_eq!(fixture.count("{infra}.aip_outbox_dispatch").await, 1, "dispatched 10 days ago is inside the default 30");
        fixture.sweep(retention(7, None, None), &["handled"], false).await;
        assert_eq!(fixture.count("{infra}.aip_outbox_dispatch").await, 0, "past a 7-day setting the dispatch record goes");
        assert_eq!(fixture.count("{s}.aip_outbox WHERE topic='handled'").await, 1, "its outbox row goes with it; the pending row stays");
    }

    #[tokio::test]
    async fn events_are_expired_by_age_only_when_configured_and_only_on_the_slow_cadence() {
        let mut fixture = Fixture::new("events").await;
        let store = aip_execution::events::Store::new(&fixture.s).unwrap();
        for n in 1..=3 {
            store.publish(&mut fixture.db, "owner", "orders", &format!("k{n}"), json!({"n":n})).await.unwrap();
        }
        fixture
            .db
            .batch_execute(&format!("UPDATE {}.aip_events SET created_at=clock_timestamp()-interval '40 days' WHERE cursor<=2", fixture.s))
            .await
            .unwrap();

        fixture.sweep(retention(30, None, None), &[], true).await;
        assert_eq!(fixture.count("{s}.aip_events").await, 3, "events_days unset keeps everything");
        fixture.sweep(retention(30, None, Some(30)), &[], false).await;
        assert_eq!(fixture.count("{s}.aip_events").await, 3, "the events sweep only runs when this pass is its turn");
        fixture.sweep(retention(30, None, Some(30)), &[], true).await;
        assert_eq!(fixture.count("{s}.aip_events").await, 1);
        let expired = store.scan(&*fixture.db, "owner", "orders", "1", 10).await;
        assert!(matches!(expired, Err(aip_execution::events::Error::CursorExpired)), "an older cursor reads as expired, not as a gap: {expired:?}");
        assert_eq!(store.scan(&*fixture.db, "owner", "orders", "2", 10).await.unwrap().events.len(), 1);
    }
}
