use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::connect_with_url;
use spike_v2_read::plan::Caller;
use spike_v4_worker::{invoke, Isolation, Lang, Worker, WorkerLimits};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const NOW: &str = "2026-10-04T00:00:00Z";
const OLD_STATS: &str = "  extension read stats {\n    input { clubId: Club.Id }\n    output { approvedApplicants: Int }\n    access Apply.approvedCount\n    effect none\n    deadline 2s\n    implementation \"recruitment.stats\"\n  }";
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

fn facts() -> Value {
    let replacement = "  extension read stats {\n    input { clubId: Club.Id; value: Text }\n    output { value: Text }\n    access Apply.approvedCount\n    effect none\n    deadline 2s\n    implementation \"values.echo\"\n  }";
    assert_eq!(A.matches(OLD_STATS).count(), 1, "fixture stats extension changed");
    load_str(&A.replacen(OLD_STATS, replacement, 1), Form::A).unwrap_or_else(|e| panic!("invalid generated V1 fixture: {e:?}")).execution
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

fn limits(stdout_frame_bytes: usize, stderr_frame_bytes: usize) -> WorkerLimits {
    WorkerLimits { stdout_frame_bytes, stderr_frame_bytes }
}

fn owned_extension_dir(lang: Lang) -> io::Result<PathBuf> {
    let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("aip-frame-boundary-{}-{id}", std::process::id()));
    fs::create_dir(&path)?;
    let (name, source) = match lang {
        Lang::Node => ("values.mjs", "export async function echo(input) {\n  await new Promise((resolve, reject) => process.stderr.write('x'.repeat(4096) + '\\n', (error) => error ? reject(error) : resolve()));\n  await new Promise((resolve) => setTimeout(resolve, 20));\n  return { value: input.value };\n}\n"),
        Lang::Python => ("values.py", "import asyncio\nimport sys\n\nasync def echo(input, ctx):\n    sys.stderr.write('x' * 4096 + '\\n')\n    sys.stderr.flush()\n    await asyncio.sleep(0.02)\n    return {\"value\": input[\"value\"]}\n"),
    };
    if let Err(error) = fs::write(path.join(name), source) {
        let _ = fs::remove_dir_all(&path);
        return Err(error);
    }
    Ok(path)
}

fn result_label(result: Result<Value, spike_v2_read::plan::Reject>) -> (String, bool) {
    match result {
        Ok(value) => {
            let passed = value == json!({ "value": "small" });
            (format!("OK {value}"), passed)
        }
        Err(error) => (error.code.to_string(), false),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_bounds_stdout_frames_and_drains_large_stderr_lines() {
    let facts = facts();
    let mut db = connect_with_url("host=localhost dbname=postgres").await.expect("local PostgreSQL connection");
    let caller = Caller { actor_id: None, now: NOW.into() };
    let official_ext_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/extensions");
    let frame_limits = limits(256, 128);
    let mut failures = Vec::new();
    let mut table = Vec::new();

    for lang in [Lang::Node, Lang::Python] {
        match Worker::try_start_with(lang, official_ext_dir, isolation(), limits(0, 128)).await {
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
                table.push(format!("{lang:?} | zero limit | InvalidInput"));
            }
            Err(error) => failures.push(format!("{lang:?} zero limit: 기대 InvalidInput, 실제 {:?}", error.kind())),
            Ok(worker) => {
                failures.push(format!("{lang:?} zero limit: worker가 기동됨"));
                worker.stop().await;
            }
        }
        match Worker::try_start_with(lang, official_ext_dir, isolation(), limits(256, 0)).await {
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
                table.push(format!("{lang:?} | zero stderr limit | InvalidInput"));
            }
            Err(error) => failures.push(format!("{lang:?} zero stderr limit: 기대 InvalidInput, 실제 {:?}", error.kind())),
            Ok(worker) => {
                failures.push(format!("{lang:?} zero stderr limit: worker가 기동됨"));
                worker.stop().await;
            }
        }

        match Worker::try_start_with(lang, official_ext_dir, isolation(), frame_limits).await {
            Err(error) => failures.push(format!("{lang:?} stdout worker 기동 실패: {:?}", error.kind())),
            Ok(mut worker) => {
                let oversized = json!({ "clubId": "10", "value": "x".repeat(1024) });
                let first = invoke(&mut db, &mut worker, &facts, "Recruitment.stats", &oversized, &caller).await;
                let first_ok = first.as_ref().err().is_some_and(|error| error.code == "WORKER_FAILED");
                table.push(format!(
                    "{lang:?} | oversized stdout | {}",
                    first.as_ref().map(|v| format!("OK {v}")).unwrap_or_else(|e| e.code.to_string())
                ));
                if !first_ok {
                    failures.push(format!("{lang:?} oversized stdout: 기대 WORKER_FAILED"));
                }

                let small = json!({ "clubId": "10", "value": "small" });
                let second = invoke(&mut db, &mut worker, &facts, "Recruitment.stats", &small, &caller).await;
                let second_ok = second.as_ref().err().is_some_and(|error| error.code == "WORKER_FAILED");
                table.push(format!("{lang:?} | 다음 호출 | {}", second.as_ref().map(|v| format!("OK {v}")).unwrap_or_else(|e| e.code.to_string())));
                if !second_ok {
                    failures.push(format!("{lang:?} after oversized stdout: 기대 WORKER_FAILED"));
                }
                worker.stop().await;
            }
        }

        let temp_dir = match owned_extension_dir(lang) {
            Ok(path) => path,
            Err(error) => {
                failures.push(format!("{lang:?} temp extension 생성 실패: {:?}", error.kind()));
                continue;
            }
        };
        let temp_ext_dir = temp_dir.to_string_lossy().into_owned();
        match Worker::try_start_with(lang, &temp_ext_dir, isolation(), frame_limits).await {
            Err(error) => failures.push(format!("{lang:?} stderr worker 기동 실패: {:?}", error.kind())),
            Ok(mut worker) => {
                let small = json!({ "clubId": "10", "value": "small" });
                let result = invoke(&mut db, &mut worker, &facts, "Recruitment.stats", &small, &caller).await;
                let (actual, passed) = result_label(result);
                table.push(format!("{lang:?} | oversized stderr line | {actual}"));
                if !passed {
                    failures.push(format!("{lang:?} oversized stderr line: 기대 OK {{\"value\":\"small\"}}, 실제 {actual}"));
                }

                let oversized = json!({ "clubId": "10", "value": "x".repeat(1024) });
                let result = invoke(&mut db, &mut worker, &facts, "Recruitment.stats", &oversized, &caller).await;
                let (actual, passed) = match result {
                    Err(error) => (
                        format!("{} ({} bytes)", error.code, error.msg.len()),
                        error.code == "WORKER_FAILED" && error.msg.contains("stderr frame limit exceeded") && error.msg.len() <= 512,
                    ),
                    Ok(value) => (format!("OK {value}"), false),
                };
                table.push(format!("{lang:?} | stderr diagnostic bound | {actual}"));
                if !passed {
                    failures
                        .push(format!("{lang:?} stderr 진단: 기대 WORKER_FAILED, `stderr frame limit exceeded` 포함, 512바이트 이하; 실제 {actual}"));
                }
                worker.stop().await;
            }
        }
        if let Err(error) = fs::remove_dir_all(&temp_dir) {
            failures.push(format!("{lang:?} temp extension 정리 실패: {:?}", error.kind()));
        }
    }

    eprintln!("{}", table.join("\n"));
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
