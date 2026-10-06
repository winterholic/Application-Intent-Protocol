use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::Caller;
use spike_v2_read::{connect_with_url, sqlgen};
use spike_v4_worker::write::invoke_write;
use spike_v4_worker::{Isolation, Lang, Worker, WorkerLimits};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

const SPEC: &str = r#"
resource Member {
  fields { id: Id }
}
actor Member

resource Event {
  fields { id: Id; member: Member; checked: Bool }
  rows read when member = actor
  transition mark {
    allow member = actor
    from checked = false
    to checked = true
  }
  expose apply mark { target id; bulk maxRows 2 }
  extension write echo {
    input { value: Text }
    output { value: Text }
    access Event.mark
    effect db
    deadline 2s
    implementation "values.echo"
  }
}
"#;

struct OwnedTempDir(PathBuf);

impl Drop for OwnedTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn make_extension_dir(lang: Lang) -> std::io::Result<OwnedTempDir> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("aip-frame-write-{}-{id}", std::process::id()));
    fs::create_dir(&path)?;
    let (name, source) = match lang {
        Lang::Node => (
            "values.mjs",
            "export async function echo(input, ctx) {\n  await ctx.data.apply('Event', 'mark', ['11']);\n  return { value: input.value };\n}\n",
        ),
        Lang::Python => (
            "values.py",
            "async def echo(input, ctx):\n    await ctx.data.apply('Event', 'mark', ['11'])\n    return {\"value\": input[\"value\"]}\n",
        ),
    };
    if let Err(error) = fs::write(path.join(name), source) {
        let _ = fs::remove_dir_all(&path);
        return Err(error);
    }
    Ok(OwnedTempDir(path))
}

fn caller() -> Caller {
    Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() }
}

fn limits() -> WorkerLimits {
    WorkerLimits { stdout_frame_bytes: 256, stderr_frame_bytes: 128 }
}

fn pg_error_code(error: &tokio_postgres::Error) -> String {
    error.code().map(|state| state.code().to_owned()).unwrap_or_else(|| "unknown".into())
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

async fn checked_rows(db: &tokio_postgres::Client, schema: &str) -> Result<Vec<(i64, bool)>, tokio_postgres::Error> {
    db.query(&format!("SELECT id, checked FROM {schema}.event ORDER BY id"), &[])
        .await
        .map(|rows| rows.into_iter().map(|row| (row.get(0), row.get(1))).collect())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_write_worker_frame_rolls_back_ctx_changes_and_next_call_commits() {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let schema = format!("aip_frame_write_{}_{}", std::process::id(), id);
    sqlgen::set_schema(&schema);

    let parsed = load_str(SPEC, Form::A);
    let facts: Value = match parsed {
        Ok(output) => output.execution,
        Err(diags) => panic!("invalid test AIP: {diags:?}"),
    };
    let mut failures = Vec::new();
    let mut db = match connect_with_url("host=localhost dbname=postgres").await {
        Ok(client) => client,
        Err(spike_v2_read::ConnectError::Postgres(error)) => panic!("local PostgreSQL connection failed: {}", pg_error_code(&error)),
        Err(_) => panic!("local PostgreSQL connection policy or TLS configuration failed"),
    };

    let ddl = match sqlgen::create_ddl(&facts) {
        Ok(statements) => statements,
        Err(error) => {
            panic!("test schema DDL generation failed: {error}");
        }
    };
    let mut owns_schema = false;
    match ddl.first() {
        Some(create_schema) => match db.batch_execute(create_schema).await {
            Ok(()) => owns_schema = true,
            Err(error) => failures.push(format!("create owned schema failed: {}", pg_error_code(&error))),
        },
        None => failures.push("create_ddl returned no schema statement".into()),
    }

    if owns_schema {
        let mut ddl_ok = true;
        for statement in ddl.iter().skip(1) {
            if let Err(error) = db.batch_execute(statement).await {
                failures.push(format!("DDL statement failed: {}", pg_error_code(&error)));
                ddl_ok = false;
                break;
            }
        }

        if ddl_ok {
            let seed = format!(
                "INSERT INTO {schema}.member (id) VALUES (1), (2);\
                 INSERT INTO {schema}.event (id, member_id, checked) VALUES (11, 1, false), (99, 2, false);"
            );
            if let Err(error) = db.batch_execute(&seed).await {
                failures.push(format!("seed failed: {}", pg_error_code(&error)));
            } else {
                for lang in [Lang::Node, Lang::Python] {
                    let temp_dir = match make_extension_dir(lang) {
                        Ok(dir) => dir,
                        Err(error) => {
                            failures.push(format!("{lang:?} extension fixture creation failed: {:?}", error.kind()));
                            continue;
                        }
                    };
                    let ext_dir = temp_dir.0.to_string_lossy().into_owned();
                    let mut oversized_worker = match Worker::try_start_with(lang, &ext_dir, isolation(), limits()).await {
                        Ok(worker) => worker,
                        Err(error) => {
                            failures.push(format!("{lang:?} worker startup failed: {:?}", error.kind()));
                            continue;
                        }
                    };
                    let oversized_input = json!({ "value": "x".repeat(1024) });
                    let oversized_result = invoke_write(&mut db, &mut oversized_worker, &facts, "Event.echo", &oversized_input, &caller()).await;
                    if !oversized_result.as_ref().is_err_and(|error| error.code == "WORKER_FAILED") {
                        failures.push(format!("{lang:?} oversized frame: expected WORKER_FAILED, got {oversized_result:?}"));
                    }
                    oversized_worker.stop().await;

                    match checked_rows(&db, &schema).await {
                        Ok(rows) if rows == [(11, false), (99, false)] => {}
                        Ok(rows) => failures.push(format!("{lang:?} oversized frame did not rollback: {rows:?}")),
                        Err(error) => failures.push(format!("{lang:?} rollback state query failed: {}", pg_error_code(&error))),
                    }

                    if let Err(error) = db.batch_execute(&format!("UPDATE {schema}.event SET checked = false")).await {
                        failures.push(format!("{lang:?} reseed failed: {}", pg_error_code(&error)));
                        continue;
                    }
                    let mut normal_worker = match Worker::try_start_with(lang, &ext_dir, isolation(), limits()).await {
                        Ok(worker) => worker,
                        Err(error) => {
                            failures.push(format!("{lang:?} normal worker startup failed: {:?}", error.kind()));
                            continue;
                        }
                    };
                    let normal_result =
                        invoke_write(&mut db, &mut normal_worker, &facts, "Event.echo", &json!({ "value": "small" }), &caller()).await;
                    if !matches!(&normal_result, Ok(value) if value == &json!({ "value": "small" })) {
                        failures.push(format!("{lang:?} normal follow-up call failed: {normal_result:?}"));
                    }
                    normal_worker.stop().await;

                    match checked_rows(&db, &schema).await {
                        Ok(rows) if rows == [(11, true), (99, false)] => {}
                        Ok(rows) => failures.push(format!("{lang:?} normal follow-up state mismatch: {rows:?}")),
                        Err(error) => failures.push(format!("{lang:?} committed state query failed: {}", pg_error_code(&error))),
                    }

                    if let Err(error) = db.batch_execute(&format!("UPDATE {schema}.event SET checked = false WHERE id = 11")).await {
                        failures.push(format!("{lang:?} row reset failed: {}", pg_error_code(&error)));
                    }
                }
            }
        }

        if let Err(error) = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await {
            failures.push(format!("owned schema cleanup failed: {}", pg_error_code(&error)));
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
