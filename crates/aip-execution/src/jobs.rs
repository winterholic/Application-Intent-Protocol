use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio_postgres::GenericClient;
use uuid::Uuid;

const TABLE: &str = "aip_jobs";
const LEASE_MS: i64 = 30_000;
const MAX_PAYLOAD_BYTES: usize = 1 << 20;
const MAX_TIMEOUT_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_ATTEMPTS: i32 = 10;

#[derive(Debug, PartialEq)]
pub enum Error {
    InvalidSchema,
    InvalidRun,
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
pub struct Store {
    schema: String,
}

impl Store {
    pub fn new(schema: impl Into<String>) -> Result<Self, Error> {
        let schema = schema.into();
        validate_schema(&schema)?;
        Ok(Self { schema })
    }

    fn table(&self) -> String {
        format!("{}.{}", self.schema, TABLE)
    }

    pub async fn enqueue(&self, client: &impl GenericClient, new: NewRun) -> Result<Run, Error> {
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
        let input_bytes = serde_json::to_vec(&new.input).map_err(|error| Error::Database(error.to_string()))?;
        if input_bytes.len() > MAX_PAYLOAD_BYTES {
            return Err(Error::PayloadTooLarge);
        }
        let input_digest = hex_digest(&input_bytes);
        let total_ms = run_budget_ms(new.timeout_ms, new.max_attempts, new.delay_ms);
        let table = self.table();
        let id = Uuid::new_v4();
        let inserted = client
            .query_opt(
                &format!(
                    "INSERT INTO {table} (id,owner,actor_id,grant_data,operation,contract_digest,deployment_digest,implementation_digest,input,input_digest,idempotency_key,state,timeout_ms,max_attempts,result_ttl_ms,initial_delay_ms,next_run_at,run_deadline) \
                     VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,'QUEUED',$12,$13,$14,$15,clock_timestamp()+($15::bigint*interval '1 millisecond'),clock_timestamp()+($16::bigint*interval '1 millisecond')) \
                     ON CONFLICT (owner,operation,contract_digest,deployment_digest,idempotency_key) DO NOTHING RETURNING id"
                ),
                &[&id, &new.owner, &new.actor, &new.grant, &new.operation, &new.contract, &new.deployment, &new.implementation, &new.input, &input_digest, &new.key, &new.timeout_ms, &new.max_attempts, &new.ttl_ms, &new.delay_ms, &total_ms],
            )
            .await?;
        let run_id = if let Some(row) = inserted {
            row.get::<_, Uuid>(0)
        } else {
            let row = client
                .query_one(
                    &format!(
                        "SELECT id,input_digest,implementation_digest,actor_id,timeout_ms,max_attempts,result_ttl_ms,initial_delay_ms \
                         FROM {table} WHERE owner=$1 AND operation=$2 AND contract_digest=$3 AND deployment_digest=$4 AND idempotency_key=$5"
                    ),
                    &[&new.owner, &new.operation, &new.contract, &new.deployment, &new.key],
                )
                .await?;
            // max_attempts is server retry policy, not part of the request. An operator changing it must
            // not turn a provider's webhook redelivery into a conflict; the stored run keeps its own.
            let same = row.get::<_, String>(1) == input_digest
                && row.get::<_, String>(2) == new.implementation
                && row.get::<_, i64>(3) == new.actor
                && row.get::<_, i64>(4) == new.timeout_ms
                && row.get::<_, i64>(6) == new.ttl_ms
                && row.get::<_, i64>(7) == new.delay_ms;
            if !same {
                return Err(Error::IdempotencyConflict);
            }
            row.get(0)
        };
        self.status(client, &run_id.to_string(), &new.owner).await?.ok_or_else(|| Error::Database("inserted job disappeared".into()))
    }

    pub async fn claim(&self, client: &impl GenericClient, worker: &str) -> Result<Option<Lease>, Error> {
        if worker.is_empty() || worker.len() > 256 {
            return Err(Error::InvalidRun);
        }
        let table = self.table();
        client
            .execute(
                &format!(
                    "UPDATE {table} SET state='FAILED',error=CASE WHEN state='CANCEL_REQUESTED' THEN 'CANCELLATION_UNCONFIRMED' WHEN run_deadline<=clock_timestamp() THEN 'deadline_exceeded' ELSE 'attempts_exhausted' END,lease_owner=NULL,lease_until=NULL,result_expires_at=clock_timestamp()+(result_ttl_ms::text||' milliseconds')::interval \
                     WHERE (state IN ('QUEUED','RUNNING') AND run_deadline<=clock_timestamp()) \
                        OR (state='RUNNING' AND lease_until<=clock_timestamp() AND attempt>=max_attempts) \
                        OR (state='CANCEL_REQUESTED' AND lease_until<=clock_timestamp())"
                ),
                &[],
            )
            .await?;
        let row = client
            .query_opt(
                &format!(
                    "WITH candidate AS ( \
                       SELECT id FROM {table} WHERE run_deadline>clock_timestamp() AND attempt<max_attempts \
                         AND ((state='QUEUED' AND next_run_at<=clock_timestamp()) OR (state='RUNNING' AND lease_until<=clock_timestamp())) \
                       ORDER BY next_run_at,created_at,id FOR UPDATE SKIP LOCKED LIMIT 1 \
                     ) \
                     UPDATE {table} j SET state='RUNNING',attempt=j.attempt+1,lease_epoch=j.lease_epoch+1,lease_owner=$1, \
                       attempt_deadline=LEAST(j.run_deadline,clock_timestamp()+(j.timeout_ms*interval '1 millisecond')), \
                       lease_until=LEAST(j.run_deadline,clock_timestamp()+($2::bigint*interval '1 millisecond'),clock_timestamp()+(j.timeout_ms*interval '1 millisecond')) \
                     FROM candidate c WHERE j.id=c.id \
                     RETURNING j.id,j.lease_epoch,j.lease_owner,j.owner,j.actor_id,j.grant_data,j.operation,j.contract_digest,j.deployment_digest,j.implementation_digest,j.input,j.attempt,j.run_deadline::text"
                ),
                &[&worker, &LEASE_MS],
            )
            .await?;
        row.map(Lease::from_row).transpose()
    }

    pub async fn heartbeat(&self, client: &impl GenericClient, lease: &Lease) -> Result<bool, Error> {
        let table = self.table();
        let changed = client
            .execute(
                &format!(
                    "UPDATE {table} SET lease_until=LEAST(attempt_deadline,clock_timestamp()+($5::bigint*interval '1 millisecond')) \
                     WHERE id=$1 AND lease_epoch=$2 AND lease_owner=$3 AND owner=$4 AND state='RUNNING' AND lease_until>clock_timestamp() AND attempt_deadline>clock_timestamp() AND run_deadline>clock_timestamp()"
                ),
                &[&lease.run_id, &lease.epoch, &lease.worker, &lease.owner, &LEASE_MS],
            )
            .await?;
        Ok(changed == 1)
    }

    pub async fn finish(&self, client: &impl GenericClient, lease: &Lease, output: Value) -> Result<bool, Error> {
        if serde_json::to_vec(&output).map_err(|error| Error::Database(error.to_string()))?.len() > MAX_PAYLOAD_BYTES {
            return Err(Error::PayloadTooLarge);
        }
        let table = self.table();
        let changed = client
            .execute(
                &format!(
                    "UPDATE {table} SET state='SUCCEEDED',output=$5,error=NULL,lease_owner=NULL,lease_until=NULL, \
                     result_expires_at=clock_timestamp()+(result_ttl_ms::text||' milliseconds')::interval \
                     WHERE id=$1 AND lease_epoch=$2 AND lease_owner=$3 AND owner=$4 AND state='RUNNING' AND lease_until>clock_timestamp() AND attempt_deadline>clock_timestamp() AND run_deadline>clock_timestamp()"
                ),
                &[&lease.run_id, &lease.epoch, &lease.worker, &lease.owner, &output],
            )
            .await?;
        Ok(changed == 1)
    }

    pub async fn retry(&self, client: &impl GenericClient, lease: &Lease, error: &str) -> Result<bool, Error> {
        let table = self.table();
        let changed = client
            .execute(
                &format!(
                    "UPDATE {table} SET \
                       state=CASE WHEN attempt>=max_attempts OR run_deadline<=clock_timestamp() THEN 'FAILED' ELSE 'QUEUED' END, \
                       next_run_at=CASE WHEN attempt>=max_attempts OR run_deadline<=clock_timestamp() THEN next_run_at ELSE clock_timestamp()+(LEAST(30000,1000*power(2,LEAST(attempt-1,15)))::bigint*interval '1 millisecond') END, \
                       error=$5,lease_owner=NULL,lease_until=NULL, \
                       result_expires_at=CASE WHEN attempt>=max_attempts OR run_deadline<=clock_timestamp() THEN clock_timestamp()+(result_ttl_ms::text||' milliseconds')::interval ELSE NULL END \
                     WHERE id=$1 AND lease_epoch=$2 AND lease_owner=$3 AND owner=$4 AND state='RUNNING' AND lease_until>clock_timestamp() AND attempt_deadline>clock_timestamp() AND run_deadline>clock_timestamp()"
                ),
                &[&lease.run_id, &lease.epoch, &lease.worker, &lease.owner, &error],
            )
            .await?;
        Ok(changed == 1)
    }

    pub async fn fail(&self, client: &impl GenericClient, lease: &Lease, code: &str) -> Result<bool, Error> {
        let table = self.table();
        let changed = client
            .execute(
                &format!(
                    "UPDATE {table} SET state='FAILED',error=$5,lease_owner=NULL,lease_until=NULL, \
                     result_expires_at=clock_timestamp()+(result_ttl_ms::text||' milliseconds')::interval \
                     WHERE id=$1 AND lease_epoch=$2 AND lease_owner=$3 AND owner=$4 AND state='RUNNING' AND lease_until>clock_timestamp() AND attempt_deadline>clock_timestamp() AND run_deadline>clock_timestamp()"
                ),
                &[&lease.run_id, &lease.epoch, &lease.worker, &lease.owner, &code],
            )
            .await?;
        Ok(changed == 1)
    }

    pub async fn check_lease(&self, client: &impl GenericClient, lease: &Lease) -> Result<bool, Error> {
        let table = self.table();
        let row = client
            .query_one(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM {table} WHERE id=$1 AND lease_epoch=$2 AND lease_owner=$3 AND owner=$4 \
                     AND state='RUNNING' AND lease_until>clock_timestamp() AND run_deadline>clock_timestamp() \
                     AND attempt_deadline>clock_timestamp())"
                ),
                &[&lease.run_id, &lease.epoch, &lease.worker, &lease.owner],
            )
            .await?;
        Ok(row.get(0))
    }

    pub async fn acknowledge_cancel(&self, client: &impl GenericClient, lease: &Lease) -> Result<bool, Error> {
        let table = self.table();
        let changed = client
            .execute(
                &format!(
                    "UPDATE {table} SET state='CANCELLED',lease_owner=NULL,lease_until=NULL, \
                     result_expires_at=clock_timestamp()+(result_ttl_ms::text||' milliseconds')::interval \
                     WHERE id=$1 AND lease_epoch=$2 AND lease_owner=$3 AND owner=$4 AND state='CANCEL_REQUESTED' AND lease_until>clock_timestamp() AND attempt_deadline>clock_timestamp()"
                ),
                &[&lease.run_id, &lease.epoch, &lease.worker, &lease.owner],
            )
            .await?;
        Ok(changed == 1)
    }

    pub async fn reject_cancel(&self, client: &impl GenericClient, lease: &Lease, error: &str) -> Result<bool, Error> {
        let table = self.table();
        let changed = client
            .execute(
                &format!(
                    "UPDATE {table} SET state='FAILED',error=$5,lease_owner=NULL,lease_until=NULL, \
                     result_expires_at=clock_timestamp()+(result_ttl_ms::text||' milliseconds')::interval \
                     WHERE id=$1 AND lease_epoch=$2 AND lease_owner=$3 AND owner=$4 AND state='CANCEL_REQUESTED' \
                       AND lease_until>clock_timestamp() AND attempt_deadline>clock_timestamp()"
                ),
                &[&lease.run_id, &lease.epoch, &lease.worker, &lease.owner, &error],
            )
            .await?;
        Ok(changed == 1)
    }

    pub async fn cancel(&self, client: &impl GenericClient, id: &str, owner: &str) -> Result<Option<Run>, Error> {
        let id = parse_id(id)?;
        let table = self.table();
        let _row = client
            .query_opt(
                &format!(
                    "UPDATE {table} SET state=CASE WHEN state='QUEUED' THEN 'CANCELLED' WHEN state='RUNNING' THEN 'CANCEL_REQUESTED' ELSE state END, \
                       cancel_requested_at=CASE WHEN state IN ('QUEUED','RUNNING') THEN clock_timestamp() ELSE cancel_requested_at END, \
                       result_expires_at=CASE WHEN state='QUEUED' THEN clock_timestamp()+(result_ttl_ms::text||' milliseconds')::interval ELSE result_expires_at END \
                     WHERE id=$1 AND owner=$2"
                ),
                &[&id, &owner],
            )
            .await?;
        self.status(client, &id.to_string(), owner).await
    }

    pub async fn status(&self, client: &impl GenericClient, id: &str, owner: &str) -> Result<Option<Run>, Error> {
        let id = parse_id(id)?;
        let table = self.table();
        let row = client
            .query_opt(
                &format!(
                    "SELECT id,operation,state,attempt,CASE WHEN result_expires_at>clock_timestamp() THEN output ELSE NULL END,error,result_expires_at \
                     FROM {table} WHERE id=$1 AND owner=$2"
                ),
                &[&id, &owner],
            )
            .await?;
        row.map(Run::from_row).transpose()
    }

    pub async fn result(&self, client: &impl GenericClient, id: &str, owner: &str) -> Result<Option<JobResult>, Error> {
        let id = parse_id(id)?;
        let table = self.table();
        let row = client
            .query_opt(
                &format!(
                    "SELECT id,operation,output FROM {table} WHERE id=$1 AND owner=$2 AND state='SUCCEEDED' AND output IS NOT NULL AND result_expires_at>clock_timestamp()"
                ),
                &[&id, &owner],
            )
            .await?;
        row.map(JobResult::from_row).transpose()
    }

    pub async fn expire_payloads(&self, client: &impl GenericClient) -> Result<Vec<Uuid>, Error> {
        let table = self.table();
        let rows = client
            .query(
                &format!(
                    "WITH expired AS ( \
                       SELECT id FROM {table} \
                       WHERE state IN ('SUCCEEDED','FAILED','CANCELLED') AND result_expires_at<=clock_timestamp() \
                         AND (input<>'{{}}'::jsonb OR grant_data<>'{{}}'::jsonb OR output IS NOT NULL) \
                       ORDER BY result_expires_at,id LIMIT 1000 FOR UPDATE SKIP LOCKED \
                     ) \
                     UPDATE {table} j SET input='{{}}'::jsonb,grant_data='{{}}'::jsonb,output=NULL \
                     FROM expired e WHERE j.id=e.id RETURNING j.id"
                ),
                &[],
            )
            .await?;
        Ok(rows.into_iter().map(|row| row.get(0)).collect())
    }
}

#[derive(Clone, Debug)]
pub struct NewRun {
    pub owner: String,
    pub actor: i64,
    pub grant: Value,
    pub operation: String,
    pub contract: String,
    pub deployment: String,
    pub implementation: String,
    pub input: Value,
    pub key: String,
    pub timeout_ms: i64,
    pub max_attempts: i32,
    pub ttl_ms: i64,
    pub delay_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Run {
    pub id: String,
    pub operation: String,
    pub state: String,
    pub attempt: i32,
    pub output: Option<Value>,
    pub error: Option<String>,
}

impl Run {
    fn from_row(row: tokio_postgres::Row) -> Result<Self, Error> {
        let id: Uuid = row.get(0);
        let output: Option<Value> = row.get(4);
        let error: Option<String> = row.get(5);
        Ok(Self { id: id.to_string(), operation: row.get(1), state: row.get(2), attempt: row.get(3), output, error })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct JobResult {
    pub id: String,
    pub operation: String,
    pub output: Value,
}

impl JobResult {
    fn from_row(row: tokio_postgres::Row) -> Result<Self, Error> {
        let id: Uuid = row.get(0);
        Ok(Self { id: id.to_string(), operation: row.get(1), output: row.get(2) })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lease {
    pub run_id: Uuid,
    pub epoch: i64,
    pub worker: String,
    pub owner: String,
    pub actor: i64,
    pub grant: Value,
    pub operation: String,
    pub contract: String,
    pub deployment: String,
    pub implementation: String,
    pub input: Value,
    pub attempt: i32,
    pub run_deadline: String,
}

impl Lease {
    fn from_row(row: tokio_postgres::Row) -> Result<Self, Error> {
        Ok(Self {
            run_id: row.get(0),
            epoch: row.get(1),
            worker: row.get(2),
            owner: row.get(3),
            actor: row.get(4),
            grant: row.get(5),
            operation: row.get(6),
            contract: row.get(7),
            deployment: row.get(8),
            implementation: row.get(9),
            input: row.get(10),
            attempt: row.get(11),
            run_deadline: row.get(12),
        })
    }
}

pub fn ddl(schema: &str) -> Result<String, Error> {
    validate_schema(schema)?;
    Ok(format!(
        "CREATE TABLE {schema}.{TABLE} (\
           id uuid PRIMARY KEY, owner text NOT NULL, actor_id bigint NOT NULL CHECK(actor_id>0), grant_data jsonb NOT NULL, \
           operation text NOT NULL, contract_digest text NOT NULL, deployment_digest text NOT NULL, implementation_digest text NOT NULL, \
           input jsonb NOT NULL, input_digest text NOT NULL, idempotency_key text NOT NULL, \
           state text NOT NULL CHECK(state IN ('QUEUED','RUNNING','CANCEL_REQUESTED','SUCCEEDED','FAILED','CANCELLED')), \
           attempt integer NOT NULL DEFAULT 0 CHECK(attempt>=0), max_attempts integer NOT NULL CHECK(max_attempts>0), \
           lease_epoch bigint NOT NULL DEFAULT 0 CHECK(lease_epoch>=0), lease_owner text, lease_until timestamptz, attempt_deadline timestamptz, \
           timeout_ms bigint NOT NULL CHECK(timeout_ms>0), run_deadline timestamptz NOT NULL, next_run_at timestamptz NOT NULL DEFAULT clock_timestamp(), \
           cancel_requested_at timestamptz, output jsonb, error text, result_ttl_ms bigint NOT NULL CHECK(result_ttl_ms>0), \
           initial_delay_ms bigint NOT NULL CHECK(initial_delay_ms>=0), created_at timestamptz NOT NULL DEFAULT clock_timestamp(), result_expires_at timestamptz, \
           UNIQUE(owner,operation,contract_digest,deployment_digest,idempotency_key))"
    ))
}

fn validate_schema(schema: &str) -> Result<(), Error> {
    let bytes = schema.as_bytes();
    if bytes.len() > 63
        || !bytes.starts_with(b"aip_")
        || !bytes.get(4).is_some_and(u8::is_ascii_lowercase)
        || !bytes[5..].iter().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
    {
        return Err(Error::InvalidSchema);
    }
    Ok(())
}

fn parse_id(id: &str) -> Result<Uuid, Error> {
    Uuid::parse_str(id).map_err(|_| Error::InvalidRun)
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn run_budget_ms(timeout_ms: i64, max_attempts: i32, initial_delay_ms: i64) -> i64 {
    let retry_budget = (1..max_attempts).map(|attempt| (1000_i64.saturating_mul(1_i64 << (attempt - 1).min(15))).min(30_000)).sum::<i64>();
    initial_delay_ms.saturating_add(timeout_ms.saturating_mul(i64::from(max_attempts))).saturating_add(retry_budget)
}
