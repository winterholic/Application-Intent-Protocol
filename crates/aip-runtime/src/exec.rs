//! Executes compiled plans. The runtime never builds SQL; it only binds named
//! parameters (as text) to statements fixed at compile time.

use crate::error::AipError;
use crate::objects::{ObjectStore, Upload};
use aip_ir::codes;
use aip_plan::*;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use tokio_postgres::types::ToSql;
use tokio_postgres::{GenericClient, Row};

#[derive(Debug, Clone, Default)]
pub struct Env {
    vals: HashMap<String, Option<String>>,
}

pub fn json_text(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        other => Some(other.to_string()),
    }
}

impl Env {
    pub fn set(&mut self, k: &str, v: Option<String>) {
        self.vals.insert(k.to_string(), v);
    }

    pub fn set_json(&mut self, k: &str, v: &Value) {
        self.set(k, json_text(v));
    }

    pub fn get(&self, k: &str) -> Option<String> {
        self.vals.get(k).cloned().flatten()
    }

    fn params(&self, sql: &Sql) -> Vec<Option<String>> {
        sql.params.iter().map(|p| self.get(p)).collect()
    }
}

pub async fn query<C: GenericClient>(c: &C, sql: &Sql, env: &Env) -> Result<Vec<Row>, tokio_postgres::Error> {
    let vals = env.params(sql);
    let refs: Vec<&(dyn ToSql + Sync)> = vals.iter().map(|v| v as &(dyn ToSql + Sync)).collect();
    c.query(sql.text.as_str(), &refs).await
}

pub async fn execute<C: GenericClient>(c: &C, sql: &Sql, env: &Env) -> Result<u64, tokio_postgres::Error> {
    let vals = env.params(sql);
    let refs: Vec<&(dyn ToSql + Sync)> = vals.iter().map(|v| v as &(dyn ToSql + Sync)).collect();
    c.execute(sql.text.as_str(), &refs).await
}

async fn one_text<C: GenericClient>(c: &C, sql: &Sql, env: &Env) -> Result<Option<String>, tokio_postgres::Error> {
    let rows = query(c, sql, env).await?;
    Ok(rows.first().and_then(|r| r.get::<_, Option<String>>(0)))
}

async fn one_bool<C: GenericClient>(c: &C, sql: &Sql, env: &Env) -> Result<bool, tokio_postgres::Error> {
    let rows = query(c, sql, env).await?;
    Ok(rows.first().and_then(|r| r.get::<_, Option<bool>>(0)).unwrap_or(false))
}

async fn one_json<C: GenericClient>(c: &C, sql: &Sql, env: &Env) -> Result<Value, tokio_postgres::Error> {
    let rows = query(c, sql, env).await?;
    Ok(rows.first().and_then(|r| r.get::<_, Option<Value>>(0)).unwrap_or(Value::Null))
}

/// Maps a database error to the contract error taxonomy.
pub fn db_error(program: &Program, intent: &str, e: &tokio_postgres::Error) -> AipError {
    let Some(db) = e.as_db_error() else {
        tracing::error!(intent, error = %e, "database connection error");
        return AipError::new(codes::UNAVAILABLE, intent, "database unavailable").retryable();
    };
    let code = db.code().code();
    let lookup = |name: &str| program.constraint_errors.get(name).cloned();
    let from_spec = |spec: ErrorSpec, msg: &str| {
        let mut err = AipError::new(&spec.code, intent, msg.to_string());
        if let Some(r) = spec.reason {
            err = err.reason(r);
        }
        err
    };
    match code {
        "23505" | "23P01" | "23514" => {
            let name = db.constraint().unwrap_or_default();
            match lookup(name) {
                Some(spec) => from_spec(spec, db.message()),
                None if code == "23505" => AipError::new(codes::CONFLICT_UNIQUE, intent, db.message()),
                None => AipError::new(codes::INVARIANT_VIOLATED, intent, db.message()),
            }
        }
        "P0001" => {
            let name = db.message().strip_prefix("aip:").unwrap_or(db.message());
            match lookup(name) {
                Some(spec) => from_spec(spec, name),
                None => AipError::new(codes::INVARIANT_VIOLATED, intent, db.message()),
            }
        }
        "23503" => {
            if db.message().contains("still referenced") || db.detail().is_some_and(|d| d.contains("still referenced")) {
                AipError::new(codes::CONFLICT_IN_USE, intent, "the row is still referenced")
            } else {
                AipError::new(codes::INPUT_REFERENCE_NOT_FOUND, intent, "a referenced row does not exist")
            }
        }
        "23502" => AipError::new(codes::INPUT_INVALID, intent, format!("missing value for {}", db.column().unwrap_or("a required field"))),
        "22P02" | "22007" | "22008" | "22003" => AipError::new(codes::INPUT_INVALID, intent, db.message()),
        "40001" | "40P01" => AipError::new(codes::CONCURRENCY_CONFLICT, intent, "concurrent update; retry").retryable(),
        _ => {
            tracing::error!(intent, sqlstate = code, error = %db.message(), "unexpected database error");
            AipError::new(codes::INTERNAL, intent, "internal error")
        }
    }
}

pub struct ExecCtx<'a> {
    pub program: &'a Program,
    pub intent: &'a str,
    pub uploads: &'a HashMap<String, Upload>,
    pub objects: &'a ObjectStore,
    pub staged: Vec<String>,
    pub partial: Vec<Value>,
    pub has_actor: bool,
    /// The keys `encrypted` fields are written with; `None` when the server has none (the program has no such field).
    pub keys: Option<&'a crate::crypto::Keys>,
}

impl ExecCtx<'_> {
    fn err(&self, e: &tokio_postgres::Error) -> AipError {
        db_error(self.program, self.intent, e)
    }
}

pub async fn run_steps<C: GenericClient + Sync>(ctx: &mut ExecCtx<'_>, c: &C, steps: &[Step], env: &mut Env) -> Result<(), AipError> {
    for s in steps {
        Box::pin(run_step(ctx, c, s, env)).await?;
    }
    Ok(())
}

async fn run_step<C: GenericClient + Sync>(ctx: &mut ExecCtx<'_>, c: &C, s: &Step, env: &mut Env) -> Result<(), AipError> {
    match s {
        Step::Load { param, entity, many, sql, .. } => {
            let Some(raw) = env.get(param) else { return Ok(()) };
            let expected = if *many { serde_json::from_str::<Vec<Value>>(&raw).map(|v| v.len()).unwrap_or(0) } else { 1 };
            let rows = query(c, sql, env).await.map_err(|e| ctx.err(&e))?;
            if rows.len() < expected {
                let reason = format!("{}_NOT_FOUND", snake_upper(entity));
                let mut err = AipError::new(codes::NOT_FOUND, ctx.intent, format!("{entity} not found")).reason(reason).path(param.clone());
                if *many {
                    let found: Vec<String> = rows.iter().map(|r| r.get::<_, uuid::Uuid>(0).to_string()).collect();
                    let missing: Vec<String> =
                        serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default().into_iter().filter(|x| !found.contains(x)).collect();
                    err.message = format!("{entity} not found: {}", missing.join(", "));
                }
                return Err(err);
            }
        }
        Step::Lock { sql } => {
            query(c, sql, env).await.map_err(|e| ctx.err(&e))?;
        }
        Step::Let { name, sql, code } => {
            let v = one_text(c, sql, env).await.map_err(|e| ctx.err(&e))?;
            if v.is_none()
                && let Some(code) = code
            {
                return Err(AipError::new(codes::PRECONDITION_FAILED, ctx.intent, format!("precondition failed: {code}")).reason(code.clone()));
            }
            env.set(name, v);
        }
        Step::Check { kind, sql, code } => {
            let ok = one_bool(c, sql, env).await.map_err(|e| ctx.err(&e))?;
            if !ok {
                return Err(match kind {
                    CheckKind::Allow if !ctx.has_actor => AipError::new(codes::AUTH_UNAUTHENTICATED, ctx.intent, "authentication required"),
                    CheckKind::Allow => {
                        let mut e = AipError::new(codes::AUTH_FORBIDDEN, ctx.intent, "not allowed");
                        if let Some(c) = code {
                            e = e.reason(c.clone());
                        }
                        e
                    }
                    CheckKind::Version => {
                        let mut e = AipError::new(codes::CONFLICT_STALE_VERSION, ctx.intent, "the row changed since it was read; reload and retry");
                        if let Some(c) = code {
                            e = e.reason(c.clone());
                        }
                        e
                    }
                    CheckKind::Tenant => AipError::new(codes::TENANT_MISMATCH, ctx.intent, "the rows belong to different tenants"),
                    // consent belongs to a person: without one there is nobody to have consented
                    CheckKind::Consent if !ctx.has_actor => AipError::new(codes::AUTH_UNAUTHENTICATED, ctx.intent, "authentication required"),
                    CheckKind::Consent => {
                        let name = code.clone().unwrap_or_default();
                        AipError::new(codes::CONSENT_REQUIRED, ctx.intent, format!("consent '{name}' (current version) is required")).reason(name)
                    }
                    _ => {
                        let code = code.clone().unwrap_or_else(|| "PRECONDITION".into());
                        AipError::new(codes::PRECONDITION_FAILED, ctx.intent, format!("precondition failed: {code}")).reason(code)
                    }
                });
            }
        }
        Step::NewId { name } => env.set(name, Some(uuid::Uuid::new_v4().to_string())),
        Step::Encrypt { field, source, member, bind, row } => {
            let plain = match member {
                Some(m) => env.get(source).and_then(|s| serde_json::from_str::<Value>(&s).ok()).and_then(|v| v.get(m).and_then(json_text)),
                None => env.get(source),
            };
            // an absent value stays absent: NULL is not encrypted
            let Some(plain) = plain else {
                env.set(bind, None);
                return Ok(());
            };
            let keys = ctx.keys.ok_or_else(|| {
                AipError::new(codes::ENCRYPTION_KEYS_MISSING, ctx.intent, format!("{field} is encrypted and this process has no keys ({})", crate::crypto::ENV_KEYS))
            })?;
            let id = match row {
                EncryptRow::New { id } => env.get(id),
                EncryptRow::Existing { id } => one_text(c, id, env).await.map_err(|e| ctx.err(&e))?,
            };
            // no such row: the statement that follows changes nothing, so there is nothing to protect
            env.set(bind, id.map(|id| keys.encrypt(field, &id, &plain)));
        }
        Step::Exec { sql, bind, .. } => match bind {
            Some(b) => {
                let rows = query(c, sql, env).await.map_err(|e| ctx.err(&e))?;
                let id = rows.first().map(|r| r.get::<_, uuid::Uuid>(0).to_string());
                env.set(b, id);
            }
            None => {
                execute(c, sql, env).await.map_err(|e| ctx.err(&e))?;
            }
        },
        Step::When { cond, steps } => {
            if one_bool(c, cond, env).await.map_err(|e| ctx.err(&e))? {
                run_steps(ctx, c, steps, env).await?;
            }
        }
        Step::EachPartial { source, item, steps } => {
            let items: Vec<Value> = env.get(source).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
            for (i, it) in items.iter().enumerate() {
                env.set_json(item, it);
                c.execute("SAVEPOINT aip_item", &[]).await.map_err(|e| ctx.err(&e))?;
                match Box::pin(run_steps(ctx, c, steps, env)).await {
                    Ok(()) => {
                        c.execute("RELEASE SAVEPOINT aip_item", &[]).await.map_err(|e| ctx.err(&e))?;
                        ctx.partial.push(json!({"index": i, "ok": true}));
                    }
                    Err(err) => {
                        c.execute("ROLLBACK TO SAVEPOINT aip_item", &[]).await.map_err(|e| ctx.err(&e))?;
                        ctx.partial.push(json!({"index": i, "ok": false, "error": err}));
                    }
                }
            }
        }
        Step::ForEach { source, item, steps } => {
            let arr = one_json(c, source, env).await.map_err(|e| ctx.err(&e))?;
            for it in arr.as_array().cloned().unwrap_or_default() {
                env.set_json(item, &it);
                Box::pin(run_steps(ctx, c, steps, env)).await?;
            }
        }
        Step::Upload { param, bucket, bind, .. } => {
            let Some(handle) = env.get(param) else { return Ok(()) };
            let Some(up) = ctx.uploads.get(&handle) else {
                return Err(AipError::new(codes::INPUT_INVALID, ctx.intent, format!("upload '{handle}' was not sent")).path(param.clone()));
            };
            let key = ctx.objects.stage(bucket, up).await.map_err(|e| AipError::new(codes::INTERNAL, ctx.intent, format!("object store: {e}")))?;
            c.execute(
                "INSERT INTO \"_aip_object\" (\"key\", \"bucket\", \"content_type\", \"size\") VALUES ($1, $2, $3, $4)",
                &[&key, bucket, &up.content_type, &(up.bytes.len() as i64)],
            )
            .await
            .map_err(|e| ctx.err(&e))?;
            ctx.staged.push(key.clone());
            if let Some(b) = bind {
                env.set(b, Some(key));
            }
        }
        Step::Deferred { effect, args, key, .. } => {
            let payload = one_json(c, args, env).await.map_err(|e| ctx.err(&e))?;
            let k = match key {
                Some(k) => one_text(c, k, env).await.map_err(|e| ctx.err(&e))?,
                None => None,
            };
            c.execute(
                "INSERT INTO \"_aip_outbox\" (\"kind\", \"name\", \"intent\", \"key\", \"payload\") VALUES ('effect', $1, $2, $3, $4)",
                &[effect, &ctx.intent.to_string(), &k, &payload],
            )
            .await
            .map_err(|e| ctx.err(&e))?;
        }
        Step::Emit { event, payload, broker } => {
            let p = one_json(c, payload, env).await.map_err(|e| ctx.err(&e))?;
            let body = json!({"event": event, "data": p, "broker": broker});
            c.execute(
                "INSERT INTO \"_aip_outbox\" (\"kind\", \"name\", \"intent\", \"payload\") VALUES ('event', $1, $2, $3)",
                &[event, &ctx.intent.to_string(), &body],
            )
            .await
            .map_err(|e| ctx.err(&e))?;
        }
        Step::Notify { recipients, template, payload, category, digest_seconds } => {
            let to = one_json(c, recipients, env).await.map_err(|e| ctx.err(&e))?;
            if to.as_array().is_some_and(|a| a.is_empty()) {
                return Ok(());
            }
            let p = one_json(c, payload, env).await.map_err(|e| ctx.err(&e))?;
            let body = json!({"template": template, "recipients": to, "data": p, "category": category, "digest_seconds": digest_seconds});
            c.execute(
                "INSERT INTO \"_aip_outbox\" (\"kind\", \"name\", \"intent\", \"payload\") VALUES ('notify', $1, $2, $3)",
                &[template, &ctx.intent.to_string(), &body],
            )
            .await
            .map_err(|e| ctx.err(&e))?;
        }
        Step::ValidateDynamic { value, schema, path } => {
            let v = one_json(c, value, env).await.map_err(|e| ctx.err(&e))?;
            let s = one_json(c, schema, env).await.map_err(|e| ctx.err(&e))?;
            crate::dynamic::validate(&v, &s).map_err(|(k, m)| AipError::new(codes::INPUT_INVALID, ctx.intent, m).path(format!("{path}.{k}")))?;
        }
        Step::Fail { code, commit } => {
            let mut e = AipError::new(codes::PRECONDITION_FAILED, ctx.intent, format!("precondition failed: {code}")).reason(code.clone());
            e.commit = *commit;
            return Err(e);
        }
    }
    Ok(())
}

pub fn snake_upper(s: &str) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_uppercase());
    }
    out
}

/// Env for an intent call: validated input plus runtime values.
pub fn env_from(input: &Map<String, Value>, actor: Option<&str>) -> Env {
    let mut env = Env::default();
    for (k, v) in input {
        env.set_json(k, v);
    }
    env.set("__actor", actor.map(String::from));
    env
}

/// Upper bound on rule rounds in one transaction: rules that keep re-arming
/// each other are a definition error, not something to loop on forever.
const RULE_ROUNDS: usize = 16;

/// Runs `rule` bodies for rows marked pending in this transaction whose
/// condition has just become true. Call before every commit that wrote data.
pub async fn settle_rules<C: GenericClient + Sync>(ctx: &mut ExecCtx<'_>, c: &C, env: &Env) -> Result<(), AipError> {
    if ctx.program.rules.is_empty() {
        return Ok(());
    }
    for _ in 0..RULE_ROUNDS {
        // SKIP LOCKED: rows another transaction is settling right now are theirs
        let pending = c
            .query(
                "DELETE FROM \"_aip_rule_pending\" WHERE (\"rule\", \"id\") IN (SELECT \"rule\", \"id\" FROM \"_aip_rule_pending\" FOR UPDATE SKIP LOCKED) RETURNING \"rule\", \"id\"",
                &[],
            )
            .await
            .map_err(|e| ctx.err(&e))?;
        if pending.is_empty() {
            return Ok(());
        }
        for row in pending {
            let name: String = row.get(0);
            let id: uuid::Uuid = row.get(1);
            let Some(rule) = ctx.program.rules.iter().find(|r| r.name == name) else { continue };
            let mut renv = Env::default();
            renv.set("__actor", env.get("__actor"));
            renv.set("__imp", env.get("__imp"));
            renv.set(&rule.binding, Some(id.to_string()));
            let holds = one_bool(c, &rule.when, &renv).await.map_err(|e| ctx.err(&e))?;
            if holds {
                let armed = c
                    .query_opt("INSERT INTO \"_aip_rule_state\" VALUES ($1, $2) ON CONFLICT DO NOTHING RETURNING 1", &[&name, &id])
                    .await
                    .map_err(|e| ctx.err(&e))?
                    .is_some();
                if armed {
                    Box::pin(run_steps(ctx, c, &rule.steps, &mut renv)).await?;
                }
            } else {
                c.execute("DELETE FROM \"_aip_rule_state\" WHERE \"rule\" = $1 AND \"id\" = $2", &[&name, &id]).await.map_err(|e| ctx.err(&e))?;
            }
        }
    }
    Err(AipError::new(codes::INTERNAL, ctx.intent, "rules did not settle; they keep triggering each other"))
}
