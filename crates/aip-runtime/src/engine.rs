//! Intent invocation: validation, idempotency, transactions, retries,
//! pagination and post-commit bookkeeping.

use crate::crypto;
use crate::error::AipError;
use crate::exec::{self, ExecCtx, env_from};
use crate::objects::{ObjectStore, Upload, check_upload};
use crate::validate;
use aip_ir::codes;
use aip_plan::*;
use base64::Engine as _;
use deadpool_postgres::Pool;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

pub struct Engine {
    pub program: Arc<Program>,
    pub pool: Pool,
    pub objects: ObjectStore,
    /// The server's secret (`AIP_SECRET`): signs tokens and derives the signing secret of every webhook endpoint.
    pub secret: Vec<u8>,
    pub outbound: crate::outbound::Settings,
    /// `AIP_ENCRYPTION_KEYS`, read at startup; `None` for a program without encrypted fields.
    pub keys: Option<Arc<crate::crypto::Keys>>,
}

#[derive(Debug, Default)]
pub struct Call {
    pub intent: String,
    pub input: Value,
    pub actor: Option<String>,
    pub client: Option<String>,
    pub idempotency_key: Option<String>,
    pub uploads: HashMap<String, Upload>,
    pub flags: HashMap<String, bool>,
    /// The impersonation session the caller's token names; `actor` is then the person acted as.
    pub impersonation: Option<Impersonation>,
}

#[derive(Debug, Clone, Default)]
pub struct Impersonation {
    pub session: String,
    /// Who is really calling. Whatever a caller puts here is replaced by the session row when the call starts.
    pub operator: Option<String>,
}

#[derive(Debug)]
pub struct Reply {
    pub data: Value,
    pub page: Option<Value>,
    pub partial: Option<Value>,
}

const MAX_RETRIES: usize = 3;

impl Engine {
    pub async fn call(&self, call: Call) -> Result<Reply, AipError> {
        self.dispatch(call, false).await
    }

    /// Trusted entry that may also run `internal` intents: durable timers (`at ... run`) and
    /// in-process callers. Nothing reachable from HTTP calls it.
    pub async fn call_internal(&self, call: Call) -> Result<Reply, AipError> {
        self.dispatch(call, true).await
    }

    /// A call that carries an impersonation session is only good while the session is open: not ended, not
    /// expired, and for this very actor. The operator comes from the session row, never from the caller.
    async fn open_session(&self, call: &mut Call) -> Result<(), AipError> {
        let Some(imp) = &mut call.impersonation else { return Ok(()) };
        let refuse =
            |why: &str| AipError::new(codes::AUTH_UNAUTHENTICATED, &call.intent, "the impersonation session is not open").reason(why.to_string());
        let (Some(actor), Some(spec)) = (call.actor.as_deref(), &self.program.impersonation) else { return Err(refuse("IMPERSONATION_ENDED")) };
        let (Ok(session), Ok(target)) = (uuid::Uuid::parse_str(&imp.session), uuid::Uuid::parse_str(actor)) else {
            return Err(refuse("IMPERSONATION_ENDED"));
        };
        let client = self.client(&call.intent).await?;
        // one round trip: is the session open, and does its operator still hold the power it was opened with
        let sql = format!(
            "WITH s AS (SELECT \"operator\" FROM \"_aip_impersonation\" WHERE \"id\" = $1 AND \"target\" = $2 AND \"ended_at\" IS NULL AND \"expires_at\" > now()) \
             SELECT \"operator\"::text, ({}) FROM s",
            spec.operator_check.text.trim_start_matches("SELECT ")
        );
        let row = client.query_opt(sql.as_str(), &[&session, &target]).await.map_err(|e| exec::db_error(&self.program, &call.intent, &e))?;
        match row {
            Some(r) if r.get::<_, bool>(1) => {
                imp.operator = Some(r.get(0));
                Ok(())
            }
            Some(_) => {
                // the operator lost the right to impersonate: the session ends here and now, for good
                client
                    .execute("UPDATE \"_aip_impersonation\" SET \"ended_at\" = now() WHERE \"id\" = $1 AND \"ended_at\" IS NULL", &[&session])
                    .await
                    .map_err(|e| exec::db_error(&self.program, &call.intent, &e))?;
                Err(refuse("IMPERSONATION_ENDED"))
            }
            None => Err(refuse("IMPERSONATION_ENDED")),
        }
    }

    /// Whether `actor` is still a row of the actor entity; a token outlives the person it was issued for.
    pub async fn actor_exists(&self, actor: &str, intent: &str) -> Result<(), AipError> {
        let Some(spec) = &self.program.actor else { return Ok(()) };
        let client = self.client(intent).await?;
        let id = uuid::Uuid::parse_str(actor).map_err(|_| AipError::new(codes::AUTH_UNAUTHENTICATED, intent, "bad actor"))?;
        let row = client
            .query_opt(format!("SELECT 1 FROM \"{}\" WHERE \"id\" = $1", spec.table).as_str(), &[&id])
            .await
            .map_err(|e| exec::db_error(&self.program, intent, &e))?;
        if row.is_none() {
            return Err(AipError::new(codes::AUTH_UNAUTHENTICATED, intent, "unknown actor"));
        }
        Ok(())
    }

    /// One evaluation of a subscription for `actor`: the rows as a list, as of one snapshot. It is everything a call is,
    /// checked from the start every time: the actor must still exist, an impersonation session must still be open, and the
    /// query's own allow, visibility, field visibility and tenant rules are applied again, so a subscriber who lost access
    /// finds out here and not from a stale grant.
    pub async fn run_subscription(
        &self,
        sub: &Subscription,
        input: Value,
        actor: Option<String>,
        impersonation: Option<Impersonation>,
        client: Option<String>,
    ) -> Result<Value, AipError> {
        let mut call = Call { intent: sub.name.clone(), input, actor, client, impersonation, ..Default::default() };
        if let Some(a) = &call.actor {
            self.actor_exists(a, &sub.name).await?;
        }
        self.open_session(&mut call).await?;
        let reply = self.query(&sub.query, call).await?;
        Ok(if sub.query.single { Value::Array(vec![reply.data]) } else { reply.data })
    }

    async fn dispatch(&self, mut call: Call, trusted: bool) -> Result<Reply, AipError> {
        let Some(intent) = self.program.intents.get(&call.intent).cloned() else {
            return Err(AipError::new(codes::REQUEST_UNKNOWN_INTENT, &call.intent, format!("no intent named '{}'", call.intent)));
        };
        self.open_session(&mut call).await?;
        match intent {
            Intent::Query(q) if trusted || !q.internal => self.query(&q, call).await,
            Intent::Command(c) if trusted || !c.internal => self.command(&c, call).await,
            _ => Err(AipError::new(codes::REQUEST_UNKNOWN_INTENT, &call.intent, "internal intents are not callable")),
        }
    }

    async fn client(&self, intent: &str) -> Result<deadpool_postgres::Object, AipError> {
        self.pool.get().await.map_err(|e| {
            tracing::error!(error = %e, "pool");
            AipError::new(codes::UNAVAILABLE, intent, "database unavailable").retryable()
        })
    }

    async fn rate_limit(&self, intent: &str, limits: &[RateLimit], call: &Call) -> Result<(), AipError> {
        if limits.is_empty() {
            return Ok(());
        }
        let client = self.client(intent).await?;
        for l in limits {
            let who = match l.key.as_str() {
                "actor" => call.actor.clone().unwrap_or_else(|| format!("anon:{}", call.client.clone().unwrap_or_default())),
                "client" => call.client.clone().unwrap_or_default(),
                other => other.to_string(),
            };
            let key = format!("{intent}:{}:{who}", l.key);
            let row = client
                .query_one(
                    "INSERT INTO \"_aip_rate\" (\"key\", \"window_start\", \"count\") VALUES ($1, to_timestamp(floor(extract(epoch from now()) / $2) * $2), 1)
                     ON CONFLICT (\"key\", \"window_start\") DO UPDATE SET \"count\" = \"_aip_rate\".\"count\" + 1 RETURNING \"count\"",
                    &[&key, &(l.per_seconds as f64)],
                )
                .await
                .map_err(|e| exec::db_error(&self.program, intent, &e))?;
            let n: i32 = row.get(0);
            if n as u64 > l.n {
                return Err(AipError::new(codes::RATE_LIMITED, intent, format!("at most {} calls per {}s", l.n, l.per_seconds)).retryable());
            }
        }
        Ok(())
    }

    fn validate(&self, name: &str, params: &[ParamSpec], call: &Call) -> Result<Map<String, Value>, AipError> {
        let ctx = validate::Ctx { program: &self.program, intent: name };
        let mut input = call.input.clone();
        // pagination inputs are transport-level, not declared parameters
        if let Value::Object(m) = &mut input {
            m.remove("cursor");
            m.remove("page");
        }
        let v = validate::params(&ctx, params, &input, "")?;
        for p in params {
            if let TypeSpec::Upload { max_bytes, types } = &p.ty
                && let Some(Value::String(h)) = v.get(&p.name)
            {
                let up = call
                    .uploads
                    .get(h)
                    .ok_or_else(|| AipError::new(codes::INPUT_INVALID, name, format!("file part '{h}' is missing")).path(p.name.clone()))?;
                check_upload(up, *max_bytes, types).map_err(|m| AipError::new(codes::INPUT_INVALID, name, m).path(p.name.clone()))?;
            }
            if let TypeSpec::List { of, .. } = &p.ty
                && let TypeSpec::Upload { max_bytes, types } = of.as_ref()
            {
                for (i, h) in v.get(&p.name).and_then(|x| x.as_array()).cloned().unwrap_or_default().iter().enumerate() {
                    let h = h.as_str().unwrap_or_default();
                    let up = call.uploads.get(h).ok_or_else(|| {
                        AipError::new(codes::INPUT_INVALID, name, format!("file part '{h}' is missing")).path(format!("{}[{i}]", p.name))
                    })?;
                    check_upload(up, *max_bytes, types).map_err(|m| AipError::new(codes::INPUT_INVALID, name, m).path(format!("{}[{i}]", p.name)))?;
                }
            }
        }
        Ok(v)
    }

    fn base_env(&self, input: &Map<String, Value>, call: &Call) -> exec::Env {
        let mut env = env_from(input, call.actor.as_deref());
        for (k, v) in &call.flags {
            env.set(&format!("__flag.{k}"), Some(v.to_string()));
        }
        env.set("__client", call.client.clone());
        env.set("__imp", call.impersonation.as_ref().map(|i| i.session.clone()));
        env.set("__secret", Some(String::from_utf8_lossy(&self.secret).into_owned()));
        env
    }

    // ---------- queries ----------

    async fn query(&self, q: &QueryPlan, call: Call) -> Result<Reply, AipError> {
        self.rate_limit(&q.name, &q.rate_limits, &call).await?;
        let input = self.validate(&q.name, &q.params, &call)?;
        let mut env = self.base_env(&input, &call);
        let size = q.page.as_ref().map(|p| p.size).unwrap_or(0);
        if let Some(p) = &q.page {
            if p.keyset {
                if let Some(c) = call.input.get("cursor").and_then(|c| c.as_str()) {
                    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .decode(c)
                        .map_err(|_| AipError::new(codes::INPUT_INVALID, &q.name, "bad cursor").path("cursor"))?;
                    let text = String::from_utf8(raw).map_err(|_| AipError::new(codes::INPUT_INVALID, &q.name, "bad cursor").path("cursor"))?;
                    env.set("__cursor", Some(text));
                }
            } else {
                let page = call.input.get("page").and_then(|v| v.as_u64()).unwrap_or(1).max(1);
                if p.max_page.is_some_and(|m| page > m) {
                    return Err(
                        AipError::new(codes::INPUT_INVALID, &q.name, format!("page must be at most {}", p.max_page.unwrap_or(0))).path("page")
                    );
                }
                env.set("__offset", Some(((page - 1) * p.size).to_string()));
            }
        }
        let variant = match &q.sort_param {
            Some(sp) => {
                let v = input.get(sp).and_then(|x| x.as_str()).unwrap_or_default().to_string();
                q.variants.iter().find(|x| x.when.as_deref() == Some(v.as_str())).or(q.variants.first())
            }
            None => q.variants.first(),
        }
        .cloned()
        .ok_or_else(|| AipError::new(codes::INTERNAL, &q.name, "query has no executable statement"))?;
        let mut client = self.client(&q.name).await?;
        let tx = client
            .build_transaction()
            .isolation_level(tokio_postgres::IsolationLevel::RepeatableRead)
            .read_only(true)
            .start()
            .await
            .map_err(|e| exec::db_error(&self.program, &q.name, &e))?;
        let uploads = HashMap::new();
        let mut ctx = ExecCtx {
            program: &self.program,
            intent: &q.name,
            uploads: &uploads,
            objects: &self.objects,
            staged: Vec::new(),
            partial: Vec::new(),
            has_actor: call.actor.is_some(),
            keys: self.keys.as_deref(),
        };
        exec::run_steps(&mut ctx, &*tx, &q.prelude, &mut env).await?;
        let rows = exec::query(&*tx, &variant.main, &env).await.map_err(|e| exec::db_error(&self.program, &q.name, &e))?;
        tx.commit().await.map_err(|e| exec::db_error(&self.program, &q.name, &e))?;
        let mut data: Value = rows.first().and_then(|r| r.get::<_, Option<Value>>(0)).unwrap_or(Value::Null);
        crypto::open_result(self.keys.as_deref(), &q.name, &q.decrypt, &mut data)?;
        let reply = if q.single {
            if data.is_null() {
                return Err(AipError::new(codes::NOT_FOUND, &q.name, "not found"));
            }
            Reply { data, page: None, partial: None }
        } else {
            let mut items = data.as_array().cloned().unwrap_or_default();
            let has_more = size > 0 && items.len() as u64 > size;
            if has_more {
                items.truncate(size as usize);
            }
            let next = if has_more {
                items.last().and_then(|l| l.get("__k")).map(|k| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(k.to_string()))
            } else {
                None
            };
            for it in &mut items {
                if let Value::Object(m) = it {
                    m.remove("__k");
                }
            }
            let page =
                q.page.as_ref().map(|p| if p.keyset { json!({"next_cursor": next, "has_more": has_more}) } else { json!({"has_more": has_more}) });
            Reply { data: Value::Array(items), page, partial: None }
        };
        if !q.touches.is_empty() {
            self.touch(q, &env, &call).await;
        }
        Ok(reply)
    }

    /// Observational counters: best effort, never fails the query.
    async fn touch(&self, q: &QueryPlan, env: &exec::Env, call: &Call) {
        let Ok(client) = self.pool.get().await else { return };
        for t in &q.touches {
            let Some(e) = self.program.entities.get(&t.entity) else { continue };
            let Some(col) = e.columns.iter().find(|c| c.field == t.field) else { continue };
            let Ok(rows) = exec::query(&**client, &t.id, env).await else { continue };
            let Some(id) = rows.first().and_then(|r| r.get::<_, Option<uuid::Uuid>>(0)) else { continue };
            if let Some(secs) = t.dedupe_seconds {
                let key = format!("{}:{}:{}:{}", t.entity, t.field, id, call.client.clone().unwrap_or_default());
                let fresh = client
                    .query_opt(
                        "INSERT INTO \"_aip_counter_seen\" (\"key\", \"expires_at\") VALUES ($1, now() + make_interval(secs => $2))
                         ON CONFLICT (\"key\") DO UPDATE SET \"expires_at\" = EXCLUDED.\"expires_at\" WHERE \"_aip_counter_seen\".\"expires_at\" < now() RETURNING 1",
                        &[&key, &(secs as f64)],
                    )
                    .await;
                if !matches!(fresh, Ok(Some(_))) {
                    continue;
                }
            }
            let sql = format!("UPDATE \"{}\" SET \"{}\" = \"{}\" + 1 WHERE \"id\" = $1", e.table, col.column, col.column);
            if let Err(err) = client.execute(sql.as_str(), &[&id]).await {
                tracing::warn!(error = %err, "counter touch failed");
            }
        }
    }

    // ---------- commands ----------

    async fn command(&self, c: &CommandPlan, call: Call) -> Result<Reply, AipError> {
        self.rate_limit(&c.name, &c.rate_limits, &call).await?;
        let input = self.validate(&c.name, &c.params, &call)?;
        if c.idempotent && c.idempotency_key.is_none() && call.idempotency_key.is_none() {
            return Err(AipError::new(codes::INPUT_IDEMPOTENCY_KEY_REQUIRED, &c.name, "send an Idempotency-Key header"));
        }
        if !c.idempotent && call.idempotency_key.is_some() {
            return Err(AipError::new(
                codes::INPUT_IDEMPOTENCY_KEY_UNSUPPORTED,
                &c.name,
                "this command is not idempotent; the key would give a false guarantee",
            ));
        }
        // what is bound for an encrypted field must not be kept readable next to it: the idempotency table keeps a keyed digest
        let hash = {
            let mut hashed = input.clone();
            for (param, member) in encrypted_inputs(c) {
                let Some(keys) = self.keys.as_deref() else { continue };
                if let Some(v) = hashed.get_mut(&param) {
                    mask_input(v, member.as_deref(), |plain| Value::String(keys.fingerprint(&exec::json_text(plain).unwrap_or_default())));
                }
            }
            format!("{:x}", Sha256::digest(Value::Object(hashed).to_string()))
        };
        let mut attempt = 0;
        loop {
            attempt += 1;
            let result = self.command_once(c, &call, &input, &hash).await;
            match result {
                Err(e) if e.retryable && e.code == codes::CONCURRENCY_CONFLICT && attempt < MAX_RETRIES && call.uploads.is_empty() => {
                    tracing::info!(intent = %c.name, attempt, "retrying after serialization conflict");
                    continue;
                }
                other => return other,
            }
        }
    }

    async fn command_once(&self, c: &CommandPlan, call: &Call, input: &Map<String, Value>, hash: &str) -> Result<Reply, AipError> {
        let mut env = self.base_env(input, call);
        let mut client = self.client(&c.name).await?;
        let tx = client.transaction().await.map_err(|e| exec::db_error(&self.program, &c.name, &e))?;
        let actor_key = call.actor.clone().unwrap_or_default();
        let mut idem_key: Option<String> = None;
        if c.idempotent {
            let key = match &c.idempotency_key {
                Some(k) => {
                    // `idempotent by <expr>`: the key comes from the input itself
                    let rows = exec::query(&*tx, k, &env).await.map_err(|e| exec::db_error(&self.program, &c.name, &e))?;
                    rows.first().and_then(|r| r.get::<_, Option<String>>(0))
                }
                None => call.idempotency_key.clone(),
            }
            .unwrap_or_default();
            let inserted = tx
                .query_opt(
                    "INSERT INTO \"_aip_idempotency\" (\"intent\", \"actor\", \"key\", \"request_hash\") VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING RETURNING 1",
                    &[&c.name, &actor_key, &key, &hash],
                )
                .await
                .map_err(|e| exec::db_error(&self.program, &c.name, &e))?;
            if inserted.is_none() {
                let prev = tx
                    .query_one(
                        "SELECT \"request_hash\", \"response\" FROM \"_aip_idempotency\" WHERE \"intent\" = $1 AND \"actor\" = $2 AND \"key\" = $3",
                        &[&c.name, &actor_key, &key],
                    )
                    .await
                    .map_err(|e| exec::db_error(&self.program, &c.name, &e))?;
                let prev_hash: String = prev.get(0);
                if prev_hash != hash {
                    return Err(AipError::new(codes::IDEMPOTENCY_KEY_REUSED, &c.name, "this key was already used with a different input"));
                }
                // the kept response is the encrypted form: what a replay answers is decrypted again, now
                let mut resp: Value = prev.get::<_, Option<Value>>(1).unwrap_or(Value::Null);
                crypto::open_result(self.keys.as_deref(), &c.name, &c.decrypt, &mut resp)?;
                return Ok(Reply { data: resp, page: None, partial: None });
            }
            idem_key = Some(key);
        }
        let mut ctx = ExecCtx {
            program: &self.program,
            intent: &c.name,
            uploads: &call.uploads,
            objects: &self.objects,
            staged: Vec::new(),
            partial: Vec::new(),
            has_actor: call.actor.is_some(),
            keys: self.keys.as_deref(),
        };
        let run = match exec::run_steps(&mut ctx, &*tx, &c.steps, &mut env).await {
            Ok(()) => exec::settle_rules(&mut ctx, &*tx, &env).await,
            err => err,
        };
        let staged = std::mem::take(&mut ctx.staged);
        if let Err(e) = run {
            if e.commit {
                tx.commit().await.map_err(|err| exec::db_error(&self.program, &c.name, &err))?;
            } else {
                drop(tx);
            }
            for k in &staged {
                self.objects.remove(k).await;
            }
            return Err(e);
        }
        let data = match &c.returns {
            Some(r) => exec::query(&*tx, r, &env)
                .await
                .map_err(|e| exec::db_error(&self.program, &c.name, &e))?
                .first()
                .and_then(|row| row.get::<_, Option<Value>>(0))
                .unwrap_or(Value::Null),
            None => Value::Null,
        };
        let partial = if ctx.partial.is_empty() { None } else { Some(Value::Array(ctx.partial.clone())) };
        let mut data = data;
        if let Some(k) = &idem_key {
            // kept while still encrypted: the table must not hold what the fields protect
            let stored = json!(data);
            tx.execute(
                "UPDATE \"_aip_idempotency\" SET \"response\" = $4 WHERE \"intent\" = $1 AND \"actor\" = $2 AND \"key\" = $3",
                &[&c.name, &actor_key, k, &stored],
            )
            .await
            .map_err(|e| exec::db_error(&self.program, &c.name, &e))?;
        }
        crypto::open_result(self.keys.as_deref(), &c.name, &c.decrypt, &mut data)?;
        let superuser = self.is_superuser(&*tx, &env).await;
        let actor_uuid = call.actor.as_deref().and_then(|a| uuid::Uuid::parse_str(a).ok());
        if let Some(imp) = &call.impersonation {
            // every command of a session is audited, with the person acted as and the person acting
            let operator = imp.operator.as_deref().and_then(|o| uuid::Uuid::parse_str(o).ok());
            let session = uuid::Uuid::parse_str(&imp.session).ok();
            tx.execute(
                "INSERT INTO \"_aip_audit\" (\"intent\", \"actor\", \"superuser\", \"input\", \"impersonated_by\", \"impersonation\") VALUES ($1, $2, false, $3, $4, $5)",
                &[&c.name, &actor_uuid, &audit_input(c, input), &operator, &session],
            )
            .await
            .map_err(|e| exec::db_error(&self.program, &c.name, &e))?;
        } else if c.audited || superuser {
            tx.execute(
                "INSERT INTO \"_aip_audit\" (\"intent\", \"actor\", \"superuser\", \"input\") VALUES ($1, $2, $3, $4)",
                &[&c.name, &actor_uuid, &superuser, &audit_input(c, input)],
            )
            .await
            .map_err(|e| exec::db_error(&self.program, &c.name, &e))?;
        }
        match tx.commit().await {
            Ok(()) => {
                if !staged.is_empty()
                    && let Ok(cl) = self.pool.get().await
                {
                    let _ = cl.execute("UPDATE \"_aip_object\" SET \"state\" = 'active' WHERE \"key\" = ANY($1)", &[&staged]).await;
                }
                Ok(Reply { data, page: None, partial })
            }
            Err(e) => {
                for k in &staged {
                    self.objects.remove(k).await;
                }
                Err(exec::db_error(&self.program, &c.name, &e))
            }
        }
    }

    async fn is_superuser<C: tokio_postgres::GenericClient>(&self, c: &C, env: &exec::Env) -> bool {
        let Some(su) = self.program.actor.as_ref().and_then(|a| a.superuser.as_ref()) else { return false };
        // someone acting as a superuser is still the operator: the bypass is not theirs to lend
        if env.get("__actor").is_none() || env.get("__imp").is_some() {
            return false;
        }
        exec::query(c, su, env).await.ok().and_then(|r| r.first().and_then(|x| x.get::<_, Option<bool>>(0))).unwrap_or(false)
    }
}

/// The parameters (and record members) the command binds to an encrypted field, as `(parameter, member)`.
fn encrypted_inputs(c: &CommandPlan) -> Vec<(String, Option<String>)> {
    fn walk(steps: &[Step], params: &[ParamSpec], out: &mut Vec<(String, Option<String>)>) {
        for s in steps {
            match s {
                Step::Encrypt { source, member, .. } if params.iter().any(|p| p.name == *source) => out.push((source.clone(), member.clone())),
                Step::When { steps, .. } | Step::EachPartial { steps, .. } | Step::ForEach { steps, .. } => walk(steps, params, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&c.steps, &c.params, &mut out);
    out
}

/// Replaces a value (or the member of a record value) with `f(value)`.
fn mask_input(v: &mut Value, member: Option<&str>, f: impl Fn(&Value) -> Value) {
    match (member, v) {
        (Some(m), Value::Object(o)) => {
            if let Some(x) = o.get_mut(m) {
                *x = f(x);
            }
        }
        (None, v) if !v.is_null() => *v = f(v),
        _ => {}
    }
}

/// What the audit log keeps of a call: its input, minus what is bound for an encrypted field.
fn audit_input(c: &CommandPlan, input: &Map<String, Value>) -> Value {
    let mut shown = input.clone();
    for (param, member) in encrypted_inputs(c) {
        if let Some(v) = shown.get_mut(&param) {
            mask_input(v, member.as_deref(), |_| Value::String("[encrypted]".into()));
        }
    }
    Value::Object(shown)
}
