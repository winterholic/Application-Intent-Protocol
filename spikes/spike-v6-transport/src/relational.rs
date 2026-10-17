//! Request handling over a non-PostgreSQL [`RelationalBackend`].
//!
//! `/read` follows the PostgreSQL path step for step (database clock, policy planner, deadline, Id wire encoding,
//! cache hint) with the adapter's SQL dialect. Everything else the adapter has not implemented is refused by name rather
//! than answered with a PostgreSQL-shaped substitute.
use crate::{server, READ_CACHE_MS};
use serde_json::{json, Value};
use spike_v2_read::backend::RelationalBackend;
use spike_v2_read::id_wire::{encode_rows, IdWire};
use spike_v2_read::plan::{plan_read_in, Caller, Reject};

fn err(e: Reject) -> Value {
    json!({ "ok": false, "code": e.code, "msg": e.msg })
}

pub(crate) async fn handle(backend: &dyn RelationalBackend, facts: &Value, path: &str, body: &Value, actor_id: Option<i64>, wire: IdWire) -> Value {
    if path != "/read" {
        return json!({
            "ok": false,
            "code": "WIRE.ADAPTER_UNSUPPORTED",
            "msg": format!("{} adapter는 {path}를 아직 지원하지 않음", backend.adapter()),
        });
    }
    let now = match tokio::time::timeout(server::DB_PREFLIGHT_TIMEOUT, backend.now()).await {
        Ok(Ok(now)) => now,
        Ok(Err(code)) => return json!({ "ok": false, "code": code, "msg": "실행 전 DB 조회 실패" }),
        Err(_) => return json!({ "ok": false, "code": "WIRE.DB_UNAVAILABLE", "msg": "실행 전 DB 조회 기한 초과" }),
    };
    let caller = Caller { actor_id, now };
    let plan = match plan_read_in(backend.dialect(), facts, &body["query"], &caller, wire) {
        Ok(plan) => plan,
        Err(e) => return err(e),
    };
    match tokio::time::timeout(std::time::Duration::from_millis(plan.deadline_ms), backend.read(&plan, wire)).await {
        Ok(Ok(rows)) => {
            let rows = match encode_rows(rows, &plan.output_type, wire) {
                Ok(rows) => rows,
                Err(e) => return err(e),
            };
            // `now` is always bound by the planner; a filter that uses the same value is conservatively not cached.
            // The dialect binds it as `YYYY-MM-DD HH:MM:SS.ffffff`, so compare against that spelling.
            let bound_now = caller.now.trim_end_matches('Z').replacen('T', " ", 1);
            let time_dependent = plan.params.iter().any(|v| v.as_deref() == Some(bound_now.as_str()));
            json!({ "ok": true, "rows": rows, "deps": plan.deps, "maxAgeMs": if time_dependent { 0 } else { READ_CACHE_MS } })
        }
        Ok(Err(e)) => err(e),
        Err(_) => json!({ "ok": false, "code": "WIRE.DEADLINE_EXCEEDED" }),
    }
}
