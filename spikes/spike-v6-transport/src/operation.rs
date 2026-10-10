use serde_json::{json, Value};
use spike_v2_read::{
    id_wire::IdWire,
    plan::{Caller, Reject},
};
use tokio_postgres::{types::ToSql, Client};

async fn authorize(db: &Client, facts: &Value, name: &str, input: &Value, caller: &Caller, wire: IdWire) -> Result<(), Reject> {
    let plan = spike_v2_read::plan::plan_operation_authorization(facts, name, input, caller, wire)?;
    let params: Vec<&(dyn ToSql + Sync)> = plan.params.iter().map(|value| value as _).collect();
    let result = tokio::time::timeout(super::server::DB_PREFLIGHT_TIMEOUT, db.query_one(&plan.sql, &params)).await;
    match result {
        Ok(Ok(row)) if row.get::<_, bool>(0) => Ok(()),
        Ok(Ok(_)) => Err(Reject { code: "ACCESS_DENIED", msg: "operation 실행 정책 거부".into() }),
        _ => Err(Reject { code: "DB_UNAVAILABLE", msg: "operation 정책 평가 실패".into() }),
    }
}

pub async fn invoke(db: &mut Client, facts: &Value, body: &Value, caller: &Caller, wire: IdWire, config: &super::server::ReadExtensions) -> Value {
    let deadline = facts["operations"][body["operation"].as_str().unwrap()]["deadlineMs"].as_u64().unwrap();
    match tokio::time::timeout(std::time::Duration::from_millis(deadline), invoke_inner(db, facts, body, caller, wire, config)).await {
        Ok(result) => result,
        Err(_) => json!({"ok":false,"code":"DEADLINE_EXCEEDED"}),
    }
}

async fn invoke_inner(db: &mut Client, facts: &Value, body: &Value, caller: &Caller, wire: IdWire, config: &super::server::ReadExtensions) -> Value {
    let name = body["operation"].as_str().unwrap();
    let input = &body["input"];
    if let Err(error) = authorize(db, facts, name, input, caller, wire).await {
        return json!({"ok":false,"code":error.code});
    }
    let mut worker = match spike_v4_worker::Worker::try_start_with(
        config.lang,
        config.dir.to_str().unwrap(),
        spike_v4_worker::Isolation::MacNetDeny,
        spike_v4_worker::WorkerLimits::default(),
    )
    .await
    {
        Ok(worker) => worker,
        Err(_) => return json!({"ok":false,"code":"WORKER_FAILED"}),
    };
    let deadline = facts["operations"][name]["deadlineMs"].as_u64().unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(deadline),
        spike_v4_worker::invoke_operation(&mut worker, facts, name, input, caller, wire),
    )
    .await;
    worker.stop().await;
    match result {
        Ok(Ok(output)) => {
            let now = match db.query_one("SELECT clock_timestamp()::text", &[]).await {
                Ok(row) => row.get::<_, String>(0),
                Err(_) => return json!({"ok":false,"code":"DB_UNAVAILABLE"}),
            };
            let current = Caller { actor_id: caller.actor_id, now };
            match authorize(db, facts, name, input, &current, wire).await {
                Ok(()) => json!({"ok":true,"output":output}),
                Err(error) => json!({"ok":false,"code":error.code}),
            }
        }
        Ok(Err(error)) => json!({"ok":false,"code":error.code}),
        Err(_) => json!({"ok":false,"code":"DEADLINE_EXCEEDED"}),
    }
}
