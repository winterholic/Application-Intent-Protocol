use super::{invoke, invoke_operation, serve_call, Isolation, Lang, Worker, MAX_RETIRED_TOKENS};
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
    let mut db = connect_with_url(aip_test_support::test_database_url!()).await.expect("worker PostgreSQL connection");
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
            ("old evicted", first_token.as_deref().unwrap_or(""), "WIRE.TOKEN_INVALID"),
            ("latest retired", last_token.as_deref().unwrap_or(""), "WIRE.TOKEN_EXPIRED"),
            ("unknown", "made-up-token", "WIRE.TOKEN_INVALID"),
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
    let mut db = connect_with_url(aip_test_support::test_database_url!()).await.expect("worker PostgreSQL connection");
    let ext_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/extensions");
    let mut failures = Vec::new();

    for lang in [Lang::Node, Lang::Python] {
        let mut worker = Worker::start_with(lang, ext_dir, isolation()).await;
        if let Err(error) = worker.child.kill().await {
            failures.push(format!("{lang:?} child kill failed: {:?}", error.kind()));
        }

        let result = invoke(&mut db, &mut worker, &facts, "Recruitment.stats", &json!({ "clubId": "10", "value": "send-failure" }), &caller()).await;
        let actual = token_error(result);
        if actual != Some("WIRE.WORKER_FAILED") {
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
    let mut db = connect_with_url(aip_test_support::test_database_url!()).await.expect("worker PostgreSQL connection");
    let mut failures = Vec::new();
    for lang in [Lang::Node, Lang::Python] {
        let path = std::env::temp_dir().join(format!("aip-cancelled-grant-{}-{lang:?}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let dir = OwnedDir(path);
        let (name, source) = match lang {
            Lang::Node => (
                "values.mjs",
                "export async function echo(input) { if (input.value === 'cancel') await new Promise(() => {}); return {value: input.value}; }",
            ),
            Lang::Python => (
                "values.py",
                "import asyncio\nasync def echo(input, ctx):\n    if input['value'] == 'cancel':\n        await asyncio.Event().wait()\n    return {'value': input['value']}\n",
            ),
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
        if token_error(old) != Some("WIRE.TOKEN_EXPIRED") {
            failures.push(format!("{lang:?} cancelled grant was not retired"));
        }
        worker.stop().await;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicitly_unisolated_busy_workers_are_reaped_by_stop_and_drop() {
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    async fn wait_for(path: &Path) -> bool {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if path.exists() {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or(false)
    }

    async fn wait_dead(pid: u32) -> bool {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let alive = Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success());
                if !alive {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or(false)
    }

    let mut failures = Vec::new();
    for lang in [Lang::Node, Lang::Python] {
        for cleanup in ["stop", "drop"] {
            let path = std::env::temp_dir().join(format!("aip-unsafe-worker-{cleanup}-{}-{lang:?}", std::process::id()));
            std::fs::create_dir(&path).unwrap();
            let dir = OwnedDir(path.clone());
            let marker = path.join("started");
            let marker_literal = serde_json::to_string(&marker.to_string_lossy()).unwrap();
            let (filename, source) = match lang {
                Lang::Node => (
                    "spin.mjs",
                    format!(
                        "import {{ writeFileSync }} from 'node:fs';\nexport async function run() {{ writeFileSync({marker_literal}, 'started'); while (true) {{}} }}\n"
                    ),
                ),
                Lang::Python => (
                    "spin.py",
                    format!(
                        "from pathlib import Path\nasync def run(input, ctx):\n    Path({marker_literal}).write_text('started')\n    while True: pass\n"
                    ),
                ),
            };
            std::fs::write(path.join(filename), source).unwrap();
            let facts: Value = load_str(
                "actor principal\noperation read spin { input { value: Text } output { value: Text } allow actor != null effect none deadline 100ms implementation \"spin.run\" }",
                Form::A,
            )
            .unwrap_or_else(|error| panic!("unsafe worker operation fixture invalid: {error:?}"))
            .execution;
            let mut worker = Worker::start_with(lang, &path.to_string_lossy(), Isolation::None).await;
            let Some(pid) = worker.child.id() else {
                failures.push(format!("{lang:?} {cleanup}: worker PID was unavailable"));
                worker.stop().await;
                continue;
            };
            let result =
                invoke_operation(&mut worker, &facts, "spin", &json!({"value":"busy"}), &caller(), spike_v2_read::id_wire::IdWire::Legacy).await;
            if result.as_ref().err().map(|error| error.code) != Some("WIRE.DEADLINE_EXCEEDED") || !wait_for(&marker).await {
                failures.push(format!("{lang:?} {cleanup}: expected a started busy extension and deadline, got {result:?}"));
            }
            if worker.tokens.len() != 0 || worker.retired_tokens.len() != 1 {
                failures.push(format!("{lang:?} {cleanup}: timed-out grant was not invalidated"));
            }
            let alive_after_timeout = Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !alive_after_timeout {
                failures.push(format!("{lang:?} {cleanup}: expected the low-level worker to remain reusable until owner cleanup"));
            }
            if cleanup == "stop" {
                worker.stop().await;
            } else {
                drop(worker);
            }
            if !wait_dead(pid).await {
                failures.push(format!("{lang:?} {cleanup}: worker process {pid} remained alive"));
                let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
            }
            drop(dir);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn grant_tokens_carry_128_bits_of_os_randomness() {
    let (a, b) = (super::grant_token('g', 1), super::grant_token('g', 1));
    for token in [&a, &b] {
        let (prefix, random) = token.split_once('-').unwrap();
        assert_eq!(prefix, "g1");
        assert_eq!(random.len(), 32, "{token}");
        assert!(random.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
    assert_ne!(a, b, "같은 호출 번호의 토큰도 추측할 수 없어야 한다");
}
