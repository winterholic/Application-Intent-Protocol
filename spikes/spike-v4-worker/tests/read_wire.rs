use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v2_read::plan::Caller;
use spike_v2_read::{connect_with_url, sqlgen};
use spike_v4_worker::{invoke, invoke_with_wire, validate_read, Isolation, Lang, Worker};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const OLD_STATS: &str = "  extension read stats {\n    input { clubId: Club.Id }\n    output { approvedApplicants: Int }\n    access Apply.approvedCount\n    effect none\n    deadline 2s\n    implementation \"recruitment.stats\"\n  }";
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn facts() -> Value {
    let replacement = r#"  extension read stats {
    input { clubId: Club.Id }
    output { approvedApplicants: Int }
    access Apply.approvedCount
    effect none
    deadline 2s
    implementation "values.count"
  }
  extension read echo {
    input { clubId: Club.Id; optionalId: Club.Id?; member: Member?; value: Text }
    output { id: Club.Id; optionalId: Club.Id?; member: Member?; value: Text }
    access Apply.approvedCount
    effect none
    deadline 2s
    implementation "values.echo"
  }
  extension read wrong {
    input { clubId: Club.Id; optionalId: Club.Id?; member: Member?; value: Text }
    output { id: Club.Id; optionalId: Club.Id?; member: Member?; value: Text }
    access Apply.approvedCount
    effect none
    deadline 2s
    implementation "values.wrong"
  }"#;
    assert_eq!(A.matches(OLD_STATS).count(), 1, "fixture stats extension changed");
    load_str(&A.replacen(OLD_STATS, replacement, 1), Form::A).unwrap_or_else(|diags| panic!("invalid wire fixture: {diags:?}")).execution
}

fn echo_input(club_id: Value, optional_id: Value, member: Value, value: &str) -> Value {
    json!({ "clubId": club_id, "optionalId": optional_id, "member": member, "value": value })
}

fn code<T>(result: Result<T, spike_v2_read::plan::Reject>) -> Option<&'static str> {
    result.err().map(|error| error.code)
}

#[test]
fn preflight_validates_wire_ids_without_changing_legacy_contract() {
    let facts = facts();
    let safe = IdWire::SafeNumber;
    let decimal = IdWire::DecimalString;

    assert!(validate_read(&facts, "Recruitment.echo", &echo_input(json!(10), json!(9_007_199_254_740_991i64), json!(1), "safe"), safe).is_ok());
    assert_eq!(
        code(validate_read(&facts, "Recruitment.echo", &echo_input(json!("10"), Value::Null, Value::Null, "wrong type"), safe)),
        Some("BAD_VALUE")
    );
    assert_eq!(
        code(validate_read(&facts, "Recruitment.echo", &echo_input(json!(-1), Value::Null, Value::Null, "negative"), safe)),
        Some("BAD_VALUE")
    );
    assert_eq!(
        code(validate_read(&facts, "Recruitment.echo", &echo_input(json!(9_007_199_254_740_992u64), Value::Null, Value::Null, "unsafe"), safe)),
        Some("ID_OUT_OF_RANGE")
    );

    assert!(validate_read(&facts, "Recruitment.echo", &echo_input(json!("10"), json!("9223372036854775807"), json!("1"), "decimal"), decimal).is_ok());
    assert_eq!(
        code(validate_read(&facts, "Recruitment.echo", &echo_input(json!(10), Value::Null, Value::Null, "wrong type"), decimal)),
        Some("BAD_VALUE")
    );
    assert_eq!(
        code(validate_read(&facts, "Recruitment.echo", &echo_input(json!("-1"), Value::Null, Value::Null, "negative"), decimal)),
        Some("BAD_VALUE")
    );
    assert_eq!(
        code(validate_read(&facts, "Recruitment.echo", &echo_input(json!("010"), Value::Null, Value::Null, "noncanonical"), decimal)),
        Some("BAD_VALUE")
    );
    assert_eq!(
        code(validate_read(&facts, "Recruitment.echo", &echo_input(json!("10"), json!("9223372036854775808"), Value::Null, "overflow"), decimal)),
        Some("BAD_VALUE")
    );
    assert_eq!(
        code(validate_read(&facts, "Recruitment.echo", &json!({ "clubId": "10", "optionalId": null, "value": "missing ref" }), decimal)),
        Some("BAD_VALUE")
    );

    // V4's old Legacy extension boundary accepts digit strings beyond the V2 ID-wire limits.
    assert!(validate_read(
        &facts,
        "Recruitment.echo",
        &echo_input(json!("999999999999999999999"), Value::Null, Value::Null, "legacy"),
        IdWire::Legacy
    )
    .is_ok());
}

struct OwnedDir(PathBuf);

impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn extension_dir(lang: Lang) -> std::io::Result<OwnedDir> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("aip-read-wire-{}-{id}", std::process::id()));
    fs::create_dir(&path)?;
    let (name, source) = match lang {
        Lang::Node => (
            "values.mjs",
            "export async function echo(input) { return { id: input.clubId, optionalId: input.optionalId, member: input.member, value: input.value }; }\n\
             export async function wrong(input) { return { id: typeof input.clubId === 'number' ? String(input.clubId) : Number(input.clubId), optionalId: input.optionalId, member: input.member, value: input.value }; }\n\
             export async function count(input, ctx) { const r = await ctx.data.aggregate('Apply', 'approvedCount', { clubId: input.clubId }); return { approvedApplicants: r.value }; }\n",
        ),
        Lang::Python => (
            "values.py",
            "async def echo(input, ctx):\n    return {'id': input['clubId'], 'optionalId': input['optionalId'], 'member': input['member'], 'value': input['value']}\n\
             async def wrong(input, ctx):\n    club_id = input['clubId']\n    out_id = str(club_id) if isinstance(club_id, int) else int(club_id)\n    return {'id': out_id, 'optionalId': input['optionalId'], 'member': input['member'], 'value': input['value']}\n\
             async def count(input, ctx):\n    result = await ctx.data.aggregate('Apply', 'approvedCount', {'clubId': input['clubId']})\n    return {'approvedApplicants': result['value']}\n",
        ),
    };
    if let Err(error) = fs::write(path.join(name), source) {
        let _ = fs::remove_dir_all(&path);
        return Err(error);
    }
    Ok(OwnedDir(path))
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
    Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() }
}

const SEED: &str = "
INSERT INTO S.school (id, name) VALUES (1, 'A대');
INSERT INTO S.member (id, school_id) VALUES (1, 1), (2, 1), (3, 1);
INSERT INTO S.club (id, name, logo, school_id) VALUES (10, 'A동아리', NULL, 1), (11, 'B동아리', NULL, 1);
INSERT INTO S.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER'), (10, 2, 'MEMBER'), (11, 3, 'ADMIN');
INSERT INTO S.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
 (100, 'A 모집', '2026-10-10T00:00:00Z', 'PUBLISHED', 0, 10, NULL), (101, 'B 모집', '2026-10-12T00:00:00Z', 'PUBLISHED', 0, 11, NULL);
INSERT INTO S.apply (id, recruitment_id, status) VALUES (200, 100, 'APPROVE'), (201, 100, 'PENDING'), (202, 101, 'APPROVE'), (203, 101, 'APPROVE');
";

fn worker_limits() -> spike_v4_worker::WorkerLimits {
    spike_v4_worker::WorkerLimits::default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_wire_round_trips_and_ctx_aggregate_uses_the_same_wire() {
    let facts = facts();
    let mut db = connect_with_url("host=localhost dbname=postgres").await.expect("local PostgreSQL connection");
    let schema = format!("aip_read_wire_{}_{}", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed));
    sqlgen::set_schema(&schema);
    let ddl = sqlgen::create_ddl(&facts).expect("DDL generation");
    let mut owns_schema = false;
    let mut failures = Vec::new();

    if let Some(create_schema) = ddl.first() {
        match db.batch_execute(create_schema).await {
            Ok(()) => owns_schema = true,
            Err(error) => failures.push(format!("create test schema failed: {error}")),
        }
    } else {
        failures.push("create_ddl returned no schema statement".into());
    }

    if owns_schema {
        let mut ddl_ok = true;
        for statement in ddl.iter().skip(1) {
            if let Err(error) = db.batch_execute(statement).await {
                failures.push(format!("test DDL failed: {error}"));
                ddl_ok = false;
                break;
            }
        }
        if ddl_ok {
            if let Err(error) = db.batch_execute(&SEED.replace("S.", &format!("{schema}."))).await {
                failures.push(format!("seed failed: {error}"));
            } else {
                for lang in [Lang::Node, Lang::Python] {
                    let dir = match extension_dir(lang) {
                        Ok(dir) => dir,
                        Err(error) => {
                            failures.push(format!("{lang:?} worker source setup failed: {:?}", error.kind()));
                            continue;
                        }
                    };
                    let mut worker = match Worker::try_start_with(lang, &dir.0.to_string_lossy(), isolation(), worker_limits()).await {
                        Ok(worker) => worker,
                        Err(error) => {
                            failures.push(format!("{lang:?} worker startup failed: {:?}", error.kind()));
                            continue;
                        }
                    };

                    let safe_input = echo_input(json!(10), json!(9_007_199_254_740_991i64), json!(1), "safe");
                    let safe_result =
                        invoke_with_wire(&mut db, &mut worker, &facts, "Recruitment.echo", &safe_input, &caller(), IdWire::SafeNumber).await;
                    if !matches!(&safe_result, Ok(value) if value == &json!({ "id": 10, "optionalId": 9_007_199_254_740_991i64, "member": 1, "value": "safe" }))
                    {
                        failures.push(format!("{lang:?} safe-number echo mismatch: {safe_result:?}"));
                    }

                    let decimal_input = echo_input(json!("10"), json!("9223372036854775807"), json!("1"), "decimal");
                    let decimal_result =
                        invoke_with_wire(&mut db, &mut worker, &facts, "Recruitment.echo", &decimal_input, &caller(), IdWire::DecimalString).await;
                    if !matches!(&decimal_result, Ok(value) if value == &json!({ "id": "10", "optionalId": "9223372036854775807", "member": "1", "value": "decimal" }))
                    {
                        failures.push(format!("{lang:?} decimal-string echo mismatch: {decimal_result:?}"));
                    }

                    let wrong_safe = invoke_with_wire(
                        &mut db,
                        &mut worker,
                        &facts,
                        "Recruitment.wrong",
                        &echo_input(json!(10), Value::Null, Value::Null, "wrong-safe"),
                        &caller(),
                        IdWire::SafeNumber,
                    )
                    .await;
                    if code(wrong_safe) != Some("OUTPUT_INVALID") {
                        failures.push(format!("{lang:?} wrong safe-number output was not rejected"));
                    }
                    let wrong_decimal = invoke_with_wire(
                        &mut db,
                        &mut worker,
                        &facts,
                        "Recruitment.wrong",
                        &echo_input(json!("10"), Value::Null, Value::Null, "wrong-decimal"),
                        &caller(),
                        IdWire::DecimalString,
                    )
                    .await;
                    if code(wrong_decimal) != Some("OUTPUT_INVALID") {
                        failures.push(format!("{lang:?} wrong decimal-string output was not rejected"));
                    }

                    let legacy_input = echo_input(json!("999999999999999999999"), Value::Null, Value::Null, "legacy");
                    let legacy_result = invoke(&mut db, &mut worker, &facts, "Recruitment.echo", &legacy_input, &caller()).await;
                    if !matches!(&legacy_result, Ok(value) if value == &json!({ "id": "999999999999999999999", "optionalId": null, "member": null, "value": "legacy" }))
                    {
                        failures.push(format!("{lang:?} legacy echo changed: {legacy_result:?}"));
                    }

                    for (wire, id, expected) in [(IdWire::SafeNumber, json!(10), 1), (IdWire::DecimalString, json!("10"), 1)] {
                        let result =
                            invoke_with_wire(&mut db, &mut worker, &facts, "Recruitment.stats", &json!({ "clubId": id }), &caller(), wire).await;
                        if !matches!(&result, Ok(value) if value == &json!({ "approvedApplicants": expected })) {
                            failures.push(format!("{lang:?} {wire:?} ctx aggregate mismatch: {result:?}"));
                        }
                    }
                    worker.stop().await;
                }
            }
        }
        if let Err(error) = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await {
            failures.push(format!("owned test schema cleanup failed: {error}"));
        }
    }

    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
