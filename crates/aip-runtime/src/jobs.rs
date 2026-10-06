//! Job worker. A job is claimed with SKIP LOCKED and kept alive by a heartbeat;
//! a worker that dies leaves a stale heartbeat and another worker resumes the
//! job from its committed progress. Items run at least once; the export file is
//! rebuilt from the item snapshot, so it is complete even after a resume.

use crate::engine::Engine;
use crate::exec::{self, Env, ExecCtx};
use aip_plan::Job;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

const MAX_ATTEMPTS: i32 = 3;
const STALE_SECS: f64 = 300.0;

pub async fn run(engine: Arc<Engine>) {
    loop {
        match tick(&engine).await {
            Ok(true) => continue,
            Ok(false) => tokio::time::sleep(Duration::from_millis(500)).await,
            Err(e) => {
                tracing::warn!(error = %e, "job worker");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

struct Claimed {
    id: uuid::Uuid,
    name: String,
    params: Value,
    requested_by: uuid::Uuid,
    items: Option<Value>,
    done: i32,
}

/// Claims one due job and runs it to completion. Returns true if a job was claimed.
pub async fn tick(engine: &Engine) -> anyhow::Result<bool> {
    if engine.program.jobs.is_empty() {
        return Ok(false);
    }
    let client = engine.pool.get().await?;
    let row = client
        .query_opt(
            "UPDATE \"_aip_job\" SET \"status\" = 'RUNNING', \"attempts\" = \"attempts\" + 1, \"heartbeat\" = now(), \"started_at\" = coalesce(\"started_at\", now()) \
             WHERE \"id\" = (SELECT \"id\" FROM \"_aip_job\" WHERE (\"status\" = 'QUEUED' AND \"available_at\" <= now()) \
               OR (\"status\" = 'RUNNING' AND \"heartbeat\" < now() - make_interval(secs => $1)) ORDER BY \"available_at\" LIMIT 1 FOR UPDATE SKIP LOCKED) \
             RETURNING \"id\", \"name\", \"params\", \"requested_by\", \"items\", \"done\"",
            &[&STALE_SECS],
        )
        .await?;
    drop(client);
    let Some(row) = row else { return Ok(false) };
    let job = Claimed { id: row.get(0), name: row.get(1), params: row.get(2), requested_by: row.get(3), items: row.get(4), done: row.get(5) };
    let result = match engine.program.jobs.iter().find(|j| j.name == job.name) {
        Some(plan) => run_job(engine, plan, &job).await,
        None => Err(format!("job '{}' is no longer defined", job.name)),
    };
    if let Err(msg) = result {
        tracing::warn!(job = %job.name, id = %job.id, error = %msg, "job failed");
        let c = engine.pool.get().await?;
        c.execute(
            "UPDATE \"_aip_job\" SET \"status\" = CASE WHEN \"attempts\" >= $2 THEN 'FAILED' ELSE 'QUEUED' END, \"error\" = $3, \
             \"available_at\" = now() + make_interval(secs => 10 * power(2, \"attempts\")), \
             \"finished_at\" = CASE WHEN \"attempts\" >= $2 THEN now() END WHERE \"id\" = $1",
            &[&job.id, &MAX_ATTEMPTS, &msg],
        )
        .await?;
    }
    Ok(true)
}

fn env_for(plan: &Job, job: &Claimed) -> Env {
    let mut input = serde_json::Map::new();
    for p in &plan.params {
        input.insert(p.clone(), job.params.get(p).cloned().unwrap_or(Value::Null));
    }
    let mut env = exec::env_from(&input, Some(&job.requested_by.to_string()));
    env.set("__job", Some(job.id.to_string()));
    env
}

fn ctx<'a>(engine: &'a Engine, name: &'a str, uploads: &'a HashMap<String, crate::objects::Upload>) -> ExecCtx<'a> {
    ExecCtx { program: &engine.program, intent: name, uploads, objects: &engine.objects, staged: Vec::new(), partial: Vec::new(), has_actor: true, keys: engine.keys.as_deref() }
}

async fn run_job(engine: &Engine, plan: &Job, job: &Claimed) -> Result<(), String> {
    let e = |x: &dyn std::fmt::Display| x.to_string();
    let uploads = HashMap::new();
    let mut env = env_for(plan, job);
    let mut client = engine.pool.get().await.map_err(|x| e(&x))?;

    // snapshot the items once; a resumed job walks the same list
    let items: Vec<Value> = match (&plan.source, &job.items) {
        (_, Some(v)) => v.as_array().cloned().unwrap_or_default(),
        (Some(src), None) => {
            let tx = client.transaction().await.map_err(|x| e(&x))?;
            let rows = exec::query(&*tx, src, &env).await.map_err(|x| e(&x))?;
            let v: Value = rows.first().and_then(|r| r.get::<_, Option<Value>>(0)).unwrap_or(Value::Array(Vec::new()));
            let n = v.as_array().map(|a| a.len() as i32).unwrap_or(0);
            tx.execute("UPDATE \"_aip_job\" SET \"items\" = $2, \"total\" = $3 WHERE \"id\" = $1", &[&job.id, &v, &n]).await.map_err(|x| e(&x))?;
            tx.commit().await.map_err(|x| e(&x))?;
            v.as_array().cloned().unwrap_or_default()
        }
        (None, None) => Vec::new(),
    };

    let mut file = match &plan.export {
        Some(x) => {
            let (key, path) = engine.objects.create(&x.bucket, &x.format).await.map_err(|x| e(&x))?;
            let mut f = tokio::fs::File::create(&path).await.map_err(|x| e(&x))?;
            // BOM so spreadsheet apps read UTF-8 (Korean names) correctly
            f.write_all("\u{feff}".as_bytes()).await.map_err(|x| e(&x))?;
            f.write_all(csv_line(&x.header).as_bytes()).await.map_err(|x| e(&x))?;
            Some((key, path, f))
        }
        None => None,
    };

    if plan.source.is_some() {
        let batch = plan.batch.max(1) as usize;
        for (n, chunk) in items.chunks(batch).enumerate() {
            let start = n * batch;
            let tx = client.transaction().await.map_err(|x| e(&x))?;
            let mut cx = ctx(engine, &plan.name, &uploads);
            if !plan.steps.is_empty() {
                for (i, id) in chunk.iter().enumerate() {
                    if ((start + i) as i32) < job.done {
                        continue;
                    }
                    if let Some(item) = &plan.item {
                        env.set_json(item, id);
                    }
                    if let Some(g) = &plan.item_guard
                        && exec::query(&*tx, g, &env).await.map_err(|x| e(&x))?.is_empty()
                    {
                        continue;
                    }
                    exec::run_steps(&mut cx, &*tx, &plan.steps, &mut env).await.map_err(|x| format!("item {}: {x}", start + i))?;
                }
            }
            if let (Some(x), Some((_, _, f))) = (&plan.export, file.as_mut()) {
                env.set("__ids", Some(Value::Array(chunk.to_vec()).to_string()));
                let rows = exec::query(&*tx, &x.rows, &env).await.map_err(|x| e(&x))?;
                let v: Value = rows.first().and_then(|r| r.get::<_, Option<Value>>(0)).unwrap_or_default();
                for r in v.as_array().cloned().unwrap_or_default() {
                    let mut cells: Vec<String> =
                        r.as_array().cloned().unwrap_or_default().iter().map(|c| c.as_str().unwrap_or_default().to_string()).collect();
                    // encrypted cells are opened for the file, with the id in the row's first cell
                    for d in &x.decrypt {
                        let row = cells.first().cloned().unwrap_or_default();
                        if cells.get(d.column).is_some_and(|c| !c.is_empty()) {
                            let keys = engine.keys.as_deref().ok_or_else(|| format!("{} is encrypted and this process has no keys", d.field))?;
                            cells[d.column] = keys.decrypt(&d.field, &row, &cells[d.column]).map_err(|why| {
                                format!("{}: {} cannot be decrypted ({why:?})", aip_ir::codes::ENCRYPTION_DECRYPT_FAILED, d.field)
                            })?;
                        }
                    }
                    f.write_all(csv_line(&cells).as_bytes()).await.map_err(|x| e(&x))?;
                }
            }
            exec::settle_rules(&mut cx, &*tx, &env).await.map_err(|x| e(&x))?;
            let done = (start + chunk.len()) as i32;
            tx.execute("UPDATE \"_aip_job\" SET \"done\" = greatest(\"done\", $2), \"heartbeat\" = now() WHERE \"id\" = $1", &[&job.id, &done])
                .await
                .map_err(|x| e(&x))?;
            tx.commit().await.map_err(|x| e(&x))?;
        }
    } else if job.done == 0 {
        let tx = client.transaction().await.map_err(|x| e(&x))?;
        let mut cx = ctx(engine, &plan.name, &uploads);
        exec::run_steps(&mut cx, &*tx, &plan.steps, &mut env).await.map_err(|x| e(&x))?;
        exec::settle_rules(&mut cx, &*tx, &env).await.map_err(|x| e(&x))?;
        tx.execute("UPDATE \"_aip_job\" SET \"done\" = 1, \"total\" = 1, \"heartbeat\" = now() WHERE \"id\" = $1", &[&job.id])
            .await
            .map_err(|x| e(&x))?;
        tx.commit().await.map_err(|x| e(&x))?;
    }

    let tx = client.transaction().await.map_err(|x| e(&x))?;
    let mut key_out = None;
    if let (Some(x), Some((key, path, mut f))) = (&plan.export, file.take()) {
        f.flush().await.map_err(|x| e(&x))?;
        let size = tokio::fs::metadata(&path).await.map_err(|x| e(&x))?.len() as i64;
        tx.execute(
            "INSERT INTO \"_aip_object\" (\"key\", \"bucket\", \"content_type\", \"size\", \"state\") VALUES ($1, $2, 'text/csv; charset=utf-8', $3, 'active')",
            &[&key, &x.bucket, &size],
        )
        .await
        .map_err(|x| e(&x))?;
        if let Some(secs) = x.expires_seconds {
            tx.execute(
                "INSERT INTO \"_aip_outbox\" (\"kind\", \"name\", \"payload\", \"available_at\") VALUES ('effect', 's3.delete', jsonb_build_object('key', $1::text), now() + make_interval(secs => $2))",
                &[&key, &(secs as f64)],
            )
            .await
            .map_err(|x| e(&x))?;
        }
        key_out = Some(key);
    }
    env.set("__file", key_out.clone());
    let mut cx = ctx(engine, &plan.name, &uploads);
    exec::run_steps(&mut cx, &*tx, &plan.finish, &mut env).await.map_err(|x| e(&x))?;
    tx.execute(
        "UPDATE \"_aip_job\" SET \"status\" = 'DONE', \"file\" = $2, \"error\" = NULL, \"finished_at\" = now(), \"heartbeat\" = now() WHERE \"id\" = $1",
        &[&job.id, &key_out],
    )
    .await
    .map_err(|x| e(&x))?;
    tx.commit().await.map_err(|x| e(&x))?;
    tracing::info!(job = %plan.name, id = %job.id, items = items.len(), "job done");
    Ok(())
}

fn csv_line(cells: &[String]) -> String {
    let mut out = cells
        .iter()
        .map(|c| if c.contains([',', '"', '\n', '\r']) { format!("\"{}\"", c.replace('"', "\"\"")) } else { c.clone() })
        .collect::<Vec<_>>()
        .join(",");
    out.push_str("\r\n");
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn csv_quoting() {
        assert_eq!(super::csv_line(&["a".into(), "b,c".into(), "say \"hi\"".into()]), "a,\"b,c\",\"say \"\"hi\"\"\"\r\n");
    }
}
