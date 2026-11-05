//! Private transport 실험. Caller 인증·계약 지문·동시 호출 상한은 호출자가 책임진다.
use crate::server::ReadExtensions;
use serde_json::{json, Value};
use spike_v2_read::{id_wire::IdWire, plan::Caller};
use spike_v3_write::{before_commit_rejection, classify_write_error, connection_failed, send_commit, CommitFailure};
use spike_v4_worker::{
    write::{prepare_write_in_with_wire_configured_isolation, validate_write, write_needs_serializable},
    Worker,
};
use std::time::{Duration, Instant};
use tokio_postgres::Client;

fn failure(code: &str) -> Value {
    json!({"ok":false,"code":code})
}

/// COMMIT 전 단계의 DB 오류. 연결을 잃었다면 커밋된 것이 없어 같은 키로 다시 보내도 안전하다(DB_UNAVAILABLE).
/// 그 밖의 오류는 기존대로 내부 오류다.
fn before_commit_failure(error: &tokio_postgres::Error) -> Value {
    if connection_failed(error) {
        failure(spike_v3_write::lost_before_commit().code)
    } else {
        failure("WIRE.INTERNAL")
    }
}

fn transaction_setup(facts: &Value, name: &str, deadline: Duration) -> String {
    let isolation = spike_v3_write::isolation_sql(write_needs_serializable(facts, name));
    format!("{isolation}; SET LOCAL statement_timeout='{}ms'; SET LOCAL TimeZone='UTC'", deadline.as_millis())
}

pub async fn apply(db: &mut Client, facts: &Value, body: &Value, caller: &Caller, wire: IdWire, config: &ReadExtensions) -> Value {
    apply_with_launch(db, facts, body, caller, wire, config, &spike_v4_worker::WorkerLaunchConfig::default()).await
}

#[allow(clippy::too_many_arguments)]
pub async fn apply_with_launch(
    db: &mut Client,
    facts: &Value,
    body: &Value,
    caller: &Caller,
    wire: IdWire,
    config: &ReadExtensions,
    launch: &spike_v4_worker::WorkerLaunchConfig,
) -> Value {
    let exact_body = body.as_object().is_some_and(|object| object.len() == 2 && object.contains_key("key") && object.contains_key("request"));
    let request = &body["request"];
    let exact_request =
        request.as_object().is_some_and(|object| object.len() == 2 && object.contains_key("extension") && object.contains_key("input"));
    let Some(key) = body["key"].as_str().filter(|key| exact_body && !key.is_empty() && key.len() <= 100) else {
        return failure("WIRE.BAD_REQUEST");
    };
    let Some(name) = request["extension"].as_str().filter(|_| exact_request) else {
        return failure("WIRE.BAD_REQUEST");
    };
    let deadline = match validate_write(facts, name, &request["input"], wire) {
        Ok(deadline) => deadline,
        Err(error) => return super::err(error),
    };
    let setup = transaction_setup(facts, name, deadline);
    let expires = Instant::now() + deadline;
    let principal = super::principal_key(caller.actor_id, wire);
    let schema = spike_v2_read::sqlgen::schema();
    let req_to_store = match crate::encryption::journal_request(facts, request, schema, &principal, key) {
        Ok(request) => request,
        Err(e) => return failure(e.code),
    };
    let mut commit_sent = false;
    let operation = async {
        let mut claim_retries = 0;
        let tx = loop {
            let tx = match db.transaction().await {
                Ok(tx) => tx,
                Err(error) => return before_commit_failure(&error),
            };
            if let Err(error) = tx.batch_execute(&setup).await {
                return before_commit_failure(&error);
            }
            // The unique index arbitrates same-key ownership before the worker runs. Unlike a
            // transaction advisory-lock SELECT, a conflicting SERIALIZABLE insert can be
            // retried with a fresh snapshot without ever replaying external worker effects.
            let claim = tx
                .query_opt(
                    &format!("INSERT INTO {schema}.aip_idem(principal,key,request,result) VALUES($1,$2,$3,'{{}}'::jsonb) ON CONFLICT (principal,key) DO NOTHING RETURNING 1"),
                    &[&principal, &key, &req_to_store],
                )
                .await;
            match claim {
                Ok(Some(_)) => break tx,
                Ok(None) => {
                    match tx
                        .query_opt(&format!("SELECT request,result::text FROM {schema}.aip_idem WHERE principal=$1 AND key=$2"), &[&principal, &key])
                        .await
                    {
                        Ok(Some(row)) => {
                            let stored: String = row.get(0);
                            let matches = match crate::encryption::stored_request_matches(facts, &stored, request, schema, &principal, key) {
                                Ok(matches) => matches,
                                Err(e) => return failure(e.code),
                            };
                            if !matches {
                                return failure("WIRE.IDEMPOTENCY_MISMATCH");
                            }
                            let mut result: Value = match serde_json::from_str(&row.get::<_, String>(1)) {
                                Ok(result) => result,
                                Err(_) => return failure("WIRE.INTERNAL"),
                            };
                            result["replayed"] = json!(true);
                            return result;
                        }
                        Ok(None) if Instant::now() < expires && claim_retries < spike_v3_write::SERIALIZATION_RETRIES => {
                            drop(tx);
                            claim_retries += 1;
                            spike_v3_write::retry_backoff(claim_retries).await;
                            continue;
                        }
                        Ok(None) => return failure("WIRE.CONFLICT"),
                        Err(error) if error.code().is_some_and(|code| code.code() == "40001") => {
                            drop(tx);
                            if Instant::now() >= expires || claim_retries >= spike_v3_write::SERIALIZATION_RETRIES {
                                return failure("WIRE.CONFLICT");
                            }
                            claim_retries += 1;
                            spike_v3_write::retry_backoff(claim_retries).await;
                            continue;
                        }
                        Err(error) => return before_commit_failure(&error),
                    }
                }
                Err(error) if error.code().is_some_and(|code| code.code() == "40001") => {
                    drop(tx);
                    if Instant::now() >= expires || claim_retries >= spike_v3_write::SERIALIZATION_RETRIES {
                        return failure("WIRE.CONFLICT");
                    }
                    claim_retries += 1;
                    spike_v3_write::retry_backoff(claim_retries).await;
                }
                Err(error) => return before_commit_failure(&error),
            }
        };
        if Instant::now() >= expires {
            return failure("WIRE.DEADLINE_EXCEEDED");
        }
        let config = match config.validate_with_writes(facts, wire) {
            Ok(config) => config,
            Err(_) => return failure("WIRE.WORKER_CONFIG"),
        };
        let mut worker = match Worker::try_start_configured(config.lang, config.dir.to_str().unwrap(), launch.clone()).await {
            Ok(worker) => worker,
            Err(_) => return failure("WIRE.WORKER_FAILED"),
        };
        // lock 대기·spawn 시간을 남은 실행 budget에서 빼고 commit을 보내기 전에만 취소한다.
        let prepared = tokio::time::timeout(
            expires.saturating_duration_since(Instant::now()),
            prepare_write_in_with_wire_configured_isolation(&tx, &mut worker, facts, name, &request["input"], caller, wire),
        )
        .await;
        worker.stop().await;
        let prepared = match prepared {
            Ok(Ok(prepared)) => prepared,
            Ok(Err(error)) => return super::err(before_commit_rejection(&tx, error)),
            Err(_) => return failure("WIRE.DEADLINE_EXCEEDED"),
        };
        let expires = expires.min(prepared.expires);
        let (resource, extension) = name.split_once('.').unwrap();
        let mut tags: Vec<String> = facts["resources"][resource]["extensions"][extension]["access"]
            .as_object()
            .into_iter()
            .flatten()
            .flat_map(|(name, _)| super::grant_write_tags(facts, name))
            .collect();
        tags.sort();
        tags.dedup();
        let result = json!({"ok":true,"output":prepared.output,"tags":tags});
        let left = expires.saturating_duration_since(Instant::now());
        if left < Duration::from_millis(300) {
            return failure("WIRE.DEADLINE_EXCEEDED");
        }
        let insert = tokio::time::timeout(
            left,
            tx.execute(
                &format!("UPDATE {schema}.aip_idem SET result=$3::text::jsonb WHERE principal=$1 AND key=$2"),
                &[&principal, &key, &result.to_string()],
            ),
        )
        .await;
        match insert {
            Ok(Ok(1)) => {}
            Ok(Ok(_)) => return failure("WIRE.INTERNAL"),
            Ok(Err(error)) => return before_commit_failure(&error),
            Err(_) => return failure("WIRE.DEADLINE_EXCEEDED"),
        }
        let left = expires.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return failure("WIRE.DEADLINE_EXCEEDED");
        }
        match tokio::time::timeout(left, send_commit(tx, || commit_sent = true)).await {
            Ok(Ok(())) => result,
            Ok(Err(CommitFailure::Rejected(error))) => super::err(classify_write_error("커밋", error)),
            Ok(Err(CommitFailure::NotSent)) => failure(spike_v3_write::lost_before_commit().code),
            Ok(Err(CommitFailure::Unknown)) | Err(_) => failure("WIRE.COMMIT_UNKNOWN"),
        }
    };
    match tokio::time::timeout_at(tokio::time::Instant::from_std(expires), operation).await {
        Ok(result) => result,
        Err(_) if commit_sent => failure("WIRE.COMMIT_UNKNOWN"),
        Err(_) => failure("WIRE.DEADLINE_EXCEEDED"),
    }
}

#[cfg(test)]
mod isolation_selection_tests {
    use super::transaction_setup;
    use serde_json::json;
    use std::time::Duration;

    #[test]
    fn write_extension_owner_selects_strictest_allowed_transition_before_queries() {
        let facts = json!({"resources":{"Event":{
            "transitions":{
                "plain":{"isolation":null},
                "guarded":{"isolation":"serializable"}
            },
            "extensions":{
                "plain":{"access":{"Event.plain":{}}},
                "mixed":{"access":{"Event.plain":{},"Event.guarded":{}}}
            }
        }}});
        let plain = transaction_setup(&facts, "Event.plain", Duration::from_millis(1000));
        let mixed = transaction_setup(&facts, "Event.mixed", Duration::from_millis(1000));
        assert!(plain.starts_with("SET TRANSACTION ISOLATION LEVEL READ COMMITTED;"));
        assert!(mixed.starts_with("SET TRANSACTION ISOLATION LEVEL SERIALIZABLE;"));
    }
}
