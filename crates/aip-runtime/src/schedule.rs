//! Durable schedules. One instance runs each schedule (advisory lock); the
//! last run time is stored so a restart neither skips nor repeats a run.

use crate::engine::Engine;
use crate::exec::{self, Env, ExecCtx};
use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

pub async fn run(engine: Arc<Engine>) {
    loop {
        for s in &engine.program.schedules {
            if let Err(e) = maybe_run(&engine, s).await {
                tracing::warn!(schedule = %s.name, error = %e, "schedule failed");
            }
        }
        tokio::time::sleep(Duration::from_secs(20)).await;
    }
}

/// The latest scheduled instant at or before `now` for the supported cron forms.
pub fn last_due(cron: &str, tz: Option<&str>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if let Some(secs) = cron.strip_prefix("@every ").and_then(|s| s.trim_end_matches('s').parse::<i64>().ok()) {
        let t = now.timestamp() - now.timestamp().rem_euclid(secs.max(1));
        return Utc.timestamp_opt(t, 0).single();
    }
    let parts: Vec<&str> = cron.split_whitespace().collect();
    let [m, h, dom, _, dow] = parts.as_slice() else { return None };
    let (m, h): (u32, u32) = (m.parse().ok()?, h.parse().ok()?);
    let zone: chrono_tz::Tz = tz.and_then(|t| t.parse().ok()).unwrap_or(chrono_tz::UTC);
    let local_now = now.with_timezone(&zone);
    for back in 0..40 {
        let day = local_now.date_naive() - chrono::Duration::days(back);
        let ok_dom = *dom == "*" || dom.parse::<u32>().ok() == Some(day.day());
        let ok_dow = *dow == "*" || weekday(dow) == Some(day.weekday().num_days_from_monday());
        if !(ok_dom && ok_dow) {
            continue;
        }
        let cand = zone.from_local_datetime(&day.and_hms_opt(h, m, 0)?).single()?;
        if cand <= local_now {
            return Some(cand.with_timezone(&Utc));
        }
    }
    None
}

fn weekday(s: &str) -> Option<u32> {
    ["mon", "tue", "wed", "thu", "fri", "sat", "sun"].iter().position(|d| *d == s).map(|i| i as u32)
}

async fn maybe_run(engine: &Engine, s: &aip_plan::Schedule) -> anyhow::Result<()> {
    let now = Utc::now();
    let Some(due) = last_due(&s.cron, s.tz.as_deref(), now) else { return Ok(()) };
    let mut client = engine.pool.get().await?;
    let tx = client.transaction().await?;
    let got: bool = tx.query_one("SELECT pg_try_advisory_xact_lock(hashtext($1))", &[&s.name]).await?.get(0);
    if !got {
        return Ok(());
    }
    let last: Option<DateTime<Utc>> =
        tx.query_opt("SELECT \"last_run\" FROM \"_aip_schedule_run\" WHERE \"name\" = $1", &[&s.name]).await?.map(|r| r.get(0));
    let Some(last) = last else {
        // first deployment: start counting from now instead of firing immediately
        tx.execute("INSERT INTO \"_aip_schedule_run\" (\"name\", \"last_run\") VALUES ($1, $2)", &[&s.name, &now]).await?;
        tx.commit().await?;
        return Ok(());
    };
    if last >= due {
        return Ok(());
    }
    run_body(engine, &tx, s).await?;
    tx.execute("UPDATE \"_aip_schedule_run\" SET \"last_run\" = $2 WHERE \"name\" = $1", &[&s.name, &now]).await?;
    tx.commit().await?;
    tracing::info!(schedule = %s.name, "ran");
    let _ = now.hour();
    Ok(())
}

/// Runs the named schedule now, in one transaction, without the cron bookkeeping.
pub async fn run_now(engine: &Engine, name: &str) -> anyhow::Result<()> {
    let s = engine.program.schedules.iter().find(|s| s.name == name).ok_or_else(|| anyhow::anyhow!("no schedule named {name}"))?;
    let mut client = engine.pool.get().await?;
    let tx = client.transaction().await?;
    run_body(engine, &tx, s).await?;
    tx.commit().await?;
    Ok(())
}

/// Runs one schedule's steps in `tx`, once per item of its `for` sweep. All items share the
/// transaction, so each one's steps begin by resetting what the previous item fixed (the tenant).
pub async fn run_body(engine: &Engine, tx: &deadpool_postgres::Transaction<'_>, s: &aip_plan::Schedule) -> anyhow::Result<()> {
    let uploads = HashMap::new();
    let mut ctx = ExecCtx {
        program: &engine.program,
        intent: &s.name,
        uploads: &uploads,
        objects: &engine.objects,
        staged: Vec::new(),
        partial: Vec::new(),
        has_actor: false,
        keys: engine.keys.as_deref(),
    };
    let mut env = Env::default();
    match (&s.source, &s.item) {
        (Some(src), Some(item)) => {
            let rows = exec::query(&**tx, src, &env).await?;
            let ids: serde_json::Value = rows.first().and_then(|r| r.get::<_, Option<serde_json::Value>>(0)).unwrap_or_default();
            for id in ids.as_array().cloned().unwrap_or_default() {
                env.set_json(item, &id);
                exec::run_steps(&mut ctx, &**tx, &s.steps, &mut env).await.map_err(|e| anyhow::anyhow!(e.to_string()))?;
            }
        }
        _ => exec::run_steps(&mut ctx, &**tx, &s.steps, &mut env).await.map_err(|e| anyhow::anyhow!(e.to_string()))?,
    }
    exec::settle_rules(&mut ctx, &**tx, &env).await.map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_in_seoul() {
        let now = Utc.with_ymd_and_hms(2026, 10, 1, 16, 0, 0).single().expect("time"); // 01:00 KST Oct 2
        let due = last_due("0 0 * * *", Some("Asia/Seoul"), now).expect("due");
        assert_eq!(due, Utc.with_ymd_and_hms(2026, 10, 1, 15, 0, 0).single().expect("time")); // 00:00 KST Oct 2
    }

    #[test]
    fn weekly_monday() {
        let now = Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).single().expect("time"); // Thursday
        let due = last_due("0 0 * * mon", None, now).expect("due");
        assert_eq!(due.weekday(), chrono::Weekday::Mon);
    }
}
