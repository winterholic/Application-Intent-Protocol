use super::{invoke, serve_call, Isolation, Lang, Worker, MAX_RETIRED_TOKENS};
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::connect_with_url;
use spike_v2_read::plan::{Caller, Reject};
use std::collections::HashSet;

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const NOW: &str = "2026-10-04T00:00:00Z";
const OLD_STATS: &str = "  extension read stats {\n    input { clubId: Club.Id }\n    output { approvedApplicants: Int }\n    access Apply.approvedCount\n    effect none\n    deadline 2s\n    implementation \"recruitment.stats\"\n  }";

fn facts() -> Value {
    let replacement = "  extension read stats {\n    input { clubId: Club.Id; value: Text }\n    output { value: Text }\n    access Apply.approvedCount\n    effect none\n    deadline 2s\n    implementation \"values.echo\"\n  }";
    assert_eq!(A.matches(OLD_STATS).count(), 1, "fixture stats extension changed");
    load_str(&A.replacen(OLD_STATS, replacement, 1), Form::A).unwrap_or_else(|error| panic!("invalid generated V1 fixture: {error:?}")).execution
}

fn isolation() -> Isolation {
    #[cfg(target_os = "macos")]
    {
        Isolation::MacNetDeny
    }
    #[cfg(not(target_os = "macos"))]
    {
        Isolation::None
    }
}

fn caller() -> Caller {
    Caller { actor_id: None, now: NOW.into() }
}

fn token_error(result: Result<Value, Reject>) -> Option<&'static str> {
    result.err().map(|error| error.code)
}

fn invoke_token(worker: &Worker, known_before: &HashSet<String>) -> Option<String> {
    worker.retired_tokens.back().cloned().or_else(|| worker.tokens.keys().find(|token| !known_before.contains(*token)).cloned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completed_grants_are_released_and_retired_tokens_are_bounded() {
    let facts = facts();
    let mut db = connect_with_url("host=localhost dbname=postgres").await.expect("local PostgreSQL connection");
    let ext_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/extensions");
    let mut failures = Vec::new();

    for lang in [Lang::Node, Lang::Python] {
        let mut worker = Worker::start_with(lang, ext_dir, isolation()).await;
        let mut first_token = None;
        let mut last_token = None;

        for index in 0..(MAX_RETIRED_TOKENS + 8) {
            let known_before: HashSet<String> = worker.tokens.keys().chain(worker.retired_tokens.iter()).cloned().collect();
            let value = format!("echo-{index}");
            let result = invoke(&mut db, &mut worker, &facts, "Recruitment.stats", &json!({ "clubId": "10", "value": value }), &caller()).await;

            if !matches!(result, Ok(ref output) if output == &json!({ "value": value })) {
                failures.push(format!("{lang:?} invoke {index} failed: {result:?}"));
            }

            let issued = invoke_token(&worker, &known_before);
            if index == 0 {
                first_token = issued.clone();
            }
            last_token = issued.or(last_token);

            if !worker.tokens.is_empty() {
                failures.push(format!("{lang:?} invoke {index} retained {} active grant entries", worker.tokens.len()));
            }
            if worker.retired_tokens.len() > MAX_RETIRED_TOKENS {
                failures
                    .push(format!("{lang:?} invoke {index} retained {} retired tokens (limit {MAX_RETIRED_TOKENS})", worker.retired_tokens.len()));
            }
        }

        if worker.retired_tokens.len() != MAX_RETIRED_TOKENS {
            failures.push(format!("{lang:?} retired token count: expected {MAX_RETIRED_TOKENS}, got {}", worker.retired_tokens.len()));
        }

        for (label, token, expected) in [
            ("old evicted", first_token.as_deref().unwrap_or(""), "TOKEN_INVALID"),
            ("latest retired", last_token.as_deref().unwrap_or(""), "TOKEN_EXPIRED"),
            ("unknown", "made-up-token", "TOKEN_INVALID"),
        ] {
            let result = serve_call(&mut db, &mut worker, &facts, &json!({ "type": "call", "call": 1, "token": token })).await;
            let actual = token_error(result);
            if actual != Some(expected) {
                failures.push(format!("{lang:?} {label} token: expected {expected}, got {actual:?}"));
            }
        }

        worker.stop().await;
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broken_worker_stdin_does_not_retain_a_grant() {
    let facts = facts();
    let mut db = connect_with_url("host=localhost dbname=postgres").await.expect("local PostgreSQL connection");
    let ext_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/extensions");
    let mut failures = Vec::new();

    for lang in [Lang::Node, Lang::Python] {
        let mut worker = Worker::start_with(lang, ext_dir, isolation()).await;
        if let Err(error) = worker.child.kill().await {
            failures.push(format!("{lang:?} child kill failed: {:?}", error.kind()));
        }

        let result = invoke(&mut db, &mut worker, &facts, "Recruitment.stats", &json!({ "clubId": "10", "value": "send-failure" }), &caller()).await;
        let actual = token_error(result);
        if actual != Some("WORKER_FAILED") {
            failures.push(format!("{lang:?} broken stdin result: expected WORKER_FAILED, got {actual:?}"));
        }
        if !worker.tokens.is_empty() {
            failures.push(format!("{lang:?} broken stdin retained {} grant entries", worker.tokens.len()));
        }

        worker.stop().await;
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

struct OwnedDir(std::path::PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn externally_cancelled_grant_is_retired_before_the_next_valid_call() {
    let facts = facts();
    let mut db = connect_with_url("host=localhost dbname=postgres").await.expect("local PostgreSQL connection");
    let mut failures = Vec::new();
    for lang in [Lang::Node, Lang::Python] {
        let path = std::env::temp_dir().join(format!("aip-cancelled-grant-{}-{lang:?}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let dir = OwnedDir(path);
        let (name, source) = match lang {
            Lang::Node => ("values.mjs", "export async function echo(input) { if (input.value === 'cancel') await new Promise(() => {}); return {value: input.value}; }"),
            Lang::Python => ("values.py", "import asyncio\nasync def echo(input, ctx):\n    if input['value'] == 'cancel':\n        await asyncio.Event().wait()\n    return {'value': input['value']}\n"),
        };
        std::fs::write(dir.0.join(name), source).unwrap();
        let mut worker = Worker::start_with(lang, &dir.0.to_string_lossy(), isolation()).await;
        let pending_input = json!({"clubId": "10", "value": "cancel"});
        let who = caller();
        let cancelled = tokio::time::timeout(
            std::time::Duration::from_millis(25),
            invoke(&mut db, &mut worker, &facts, "Recruitment.stats", &pending_input, &who),
        )
        .await;
        if cancelled.is_ok() || worker.tokens.len() != 1 {
            failures.push(format!("{lang:?} cancellation did not leave exactly one in-flight grant"));
        }
        let old_token = worker.tokens.keys().next().cloned().unwrap_or_default();
        let resumed = invoke(&mut db, &mut worker, &facts, "Recruitment.stats", &json!({"clubId": "10", "value": "after"}), &who).await;
        if !matches!(resumed, Ok(ref output) if output == &json!({"value": "after"})) || !worker.tokens.is_empty() {
            failures.push(format!("{lang:?} follow-up did not release both grants: {resumed:?}"));
        }
        let old = serve_call(&mut db, &mut worker, &facts, &json!({"type":"call", "call":1,"token":old_token})).await;
        if token_error(old) != Some("TOKEN_EXPIRED") {
            failures.push(format!("{lang:?} cancelled grant was not retired"));
        }
        worker.stop().await;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
