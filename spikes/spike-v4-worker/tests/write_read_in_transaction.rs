//! PostgreSQL write extension `ctx.data.read`: the public read of the caller runs inside the extension's own transaction.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{
    connect_with_url,
    id_wire::{encode_rows, IdWire},
    plan::{plan_read_with_wire, Caller},
    sqlgen,
};
use spike_v4_worker::{write::prepare_write_in_with_wire, Isolation, Lang, Worker, WorkerLaunchConfig};
use std::{fs, path::PathBuf};

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Event {
  fields { id: Id; member: Member; label: Text(1..50); checked: Bool; note: Text? }
  rows read when member = actor
  transition mark { allow member = actor; from checked = false; to checked = true }
  expose apply mark { target id; bulk maxRows 2 }
  expose read {
    select id, member, label, checked
    filter id.eq, member.eq
    sort id
    budget { rows 10; depth 1; cost 100; deadline 2s }
  }
  extension write peek {
    input { id: Event.Id }
    output { before: Bool; after: Bool; seen: Int(0..100) }
    access read.Event, Event.mark
    effect db
    deadline 5s
    implementation "rd.peek"
  }
  extension write listing {
    input { id: Event.Id }
    output { json: Text(0..2000) }
    access read.Event
    effect db
    deadline 5s
    implementation "rd.listing"
  }
  extension write undeclared {
    input { id: Event.Id }
    output { ok: Bool }
    access Event.mark
    effect db
    deadline 5s
    implementation "rd.undeclared"
  }
  extension write hiddenField {
    input { id: Event.Id }
    output { ok: Bool }
    access read.Event, Event.mark
    effect db
    deadline 5s
    implementation "rd.hidden_field"
  }
  extension write mismatch {
    input { id: Event.Id }
    output { ok: Bool }
    access read.Event, Event.mark
    effect db
    deadline 5s
    implementation "rd.mismatch"
  }
  extension write remember {
    input { id: Event.Id }
    output { ok: Bool }
    access read.Event, Event.mark
    effect db
    deadline 5s
    implementation "rd.remember"
  }
  extension write stale {
    input { id: Event.Id }
    output { ok: Bool }
    access read.Event, Event.mark
    effect db
    deadline 5s
    implementation "rd.stale"
  }
}
"#;

const WORKER: &str = r#"
saved = {}

def query(id, select=("id", "checked")):
    return {"read": "Event", "select": list(select), "filter": [{"field": "id", "op": "eq", "value": id}], "limit": 1}

async def peek(input, ctx):
    before = await ctx.data.read(query(input["id"]))
    await ctx.data.apply("Event", "mark", [input["id"]])
    after = await ctx.data.read(query(input["id"]))
    return {"before": before["rows"][0]["checked"], "after": after["rows"][0]["checked"], "seen": len(after["rows"])}

async def listing(input, ctx):
    import json
    rows = await ctx.data.read({"read": "Event", "select": ["id", "member", "label", "checked"], "sort": [{"field": "id"}], "limit": 10})
    return {"json": json.dumps(rows["rows"], sort_keys=True)}

async def swallowed(ctx, id, attempt):
    await ctx.data.apply("Event", "mark", [id])
    try:
        await attempt()
    except Exception:
        pass
    return {"ok": True}

async def undeclared(input, ctx):
    return await swallowed(ctx, input["id"], lambda: ctx.data.read(query(input["id"])))

async def hidden_field(input, ctx):
    return await swallowed(ctx, input["id"], lambda: ctx.data.read(query(input["id"], ("note",))))

async def mismatch(input, ctx):
    return await swallowed(ctx, input["id"], lambda: ctx.call("read", "Event", {**query(input["id"]), "read": "Member"}))

async def remember(input, ctx):
    saved["ctx"] = ctx
    return {"ok": True}

async def stale(input, ctx):
    try:
        await saved["ctx"].data.read(query(input["id"]))
    except Exception as error:
        return {"ok": str(error) == "WIRE.TOKEN_EXPIRED"}
    return {"ok": False}
"#;

struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn public_query(id: &str, select: &[&str]) -> Value {
    json!({"read": "Event", "select": select, "filter": [{"field": "id", "op": "eq", "value": id}], "limit": 1})
}

#[test]
fn the_fixture_declares_read_grants_the_postgres_host_must_honour() {
    let facts: Value = load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let access = |name: &str| facts["resources"]["Event"]["extensions"][name]["access"].clone();
    assert_eq!(access("peek"), json!({"read.Event": "read", "Event.mark": "transition"}));
    assert_eq!(access("undeclared"), json!({"Event.mark": "transition"}));
}

#[tokio::test]
async fn ctx_data_read_runs_in_the_write_transaction_under_the_callers_read_policy() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let dir = std::env::temp_dir().join(format!("aip-write-read-{unique}"));
    fs::create_dir(&dir).unwrap();
    let owned = OwnedDir(dir);
    fs::write(owned.0.join("rd.py"), WORKER).unwrap();
    let schema = format!("aip_write_read_{unique}");
    sqlgen::set_schema(&schema);
    let facts: Value = load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let wire = IdWire::DecimalString;
    let mut db = connect_with_url(aip_test_support::test_database_url!()).await.expect("local PostgreSQL");
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("fresh schema ownership");
    let mut failures = vec![];
    let run: Result<(), tokio_postgres::Error> = async {
        for statement in ddl.iter().skip(1) {
            db.batch_execute(statement).await?;
        }
        db.batch_execute(&format!(
            "INSERT INTO {schema}.member(id) VALUES (1),(2); INSERT INTO {schema}.event(id,member_id,label,checked,note) VALUES \
             (11,1,'mine',false,'n1'),(12,1,'second',false,NULL),(99,2,'theirs',false,'secret')"
        ))
        .await?;
        let caller = Caller { actor_id: Some(1), now: "2026-10-05T00:00:00Z".into() };
        let isolation = if cfg!(target_os = "macos") { Isolation::MacNetDeny } else { Isolation::None };
        let launch = WorkerLaunchConfig { isolation, runtime_executable: Some("/usr/bin/python3".into()), ..WorkerLaunchConfig::default() };
        let mut worker = Worker::try_start_configured(Lang::Python, owned.0.to_str().unwrap(), launch).await.expect("official Python worker");
        let checked = |id: i64| format!("SELECT checked FROM {schema}.event WHERE id={id}");

        // The extension reads, writes through a declared transition, then reads again: the second read sees the uncommitted write.
        for commit in [false, true] {
            db.batch_execute(&format!("UPDATE {schema}.event SET checked=false")).await?;
            let tx = db.transaction().await?;
            match prepare_write_in_with_wire(&tx, &mut worker, &facts, "Event.peek", &json!({"id": "11"}), &caller, wire).await {
                Ok(prepared) => {
                    if prepared.output != json!({"before": false, "after": true, "seen": 1}) {
                        failures.push(format!("commit={commit}: read did not see the same transaction's write: {}", prepared.output));
                    }
                    if commit {
                        tx.commit().await?
                    } else {
                        tx.rollback().await?
                    }
                }
                Err(error) => {
                    failures.push(format!("commit={commit}: peek refused: {} {}", error.code, error.msg));
                    tx.rollback().await?;
                }
            }
            let stored: bool = db.query_one(&checked(11), &[]).await?.get(0);
            if stored != commit {
                failures.push(format!("commit={commit}: stored checked={stored}"));
            }
        }

        // Rows and fields come from the public read path of the same actor: another member's row stays hidden.
        let plan = plan_read_with_wire(
            &facts,
            &json!({"read": "Event", "select": ["id", "member", "label", "checked"], "sort": [{"field": "id"}], "limit": 10}),
            &caller,
            wire,
        )
        .unwrap();
        let public = encode_rows(spike_v2_read::execute(&mut db, &plan).await.unwrap(), &plan.output_type, wire).unwrap();
        let tx = db.transaction().await?;
        match prepare_write_in_with_wire(&tx, &mut worker, &facts, "Event.listing", &json!({"id": "11"}), &caller, wire).await {
            Ok(prepared) => {
                let inside: Value = serde_json::from_str(prepared.output["json"].as_str().unwrap()).unwrap();
                if inside != Value::Array(public.clone()) || public.iter().any(|row| row["id"] == "99") {
                    failures.push(format!("inside rows differ from the public read: {inside} vs {public:?}"));
                }
            }
            Err(error) => failures.push(format!("list refused: {} {}", error.code, error.msg)),
        }
        tx.rollback().await?;

        // The extension swallows a refused ctx call after a write: nothing commits, and the poison names the planner's own refusal.
        let planner_code = |query: &Value| plan_read_with_wire(&facts, query, &caller, wire).err().map(|error| error.code);
        let cases: Vec<(&str, &str, String)> = vec![
            ("Event.undeclared", "11", "WIRE.ACCESS_NOT_DECLARED".into()),
            ("Event.hiddenField", "11", planner_code(&public_query("11", &["note"])).expect("note is not a public field").to_string()),
            ("Event.mismatch", "11", "WIRE.BAD_VALUE".into()),
        ];
        for (name, id, code) in cases {
            db.batch_execute(&format!("UPDATE {schema}.event SET checked=false")).await?;
            let tx = db.transaction().await?;
            match prepare_write_in_with_wire(&tx, &mut worker, &facts, name, &json!({"id": id}), &caller, wire).await {
                Ok(_) => failures.push(format!("{name}: a swallowed refusal was committed")),
                Err(error) if error.code == "WIRE.EXTENSION_ERROR" && error.msg.ends_with(&code) => {}
                Err(error) => failures.push(format!("{name}: expected EXTENSION_ERROR after {code}, got {} {}", error.code, error.msg)),
            }
            tx.rollback().await?;
            let stored: bool = db.query_one(&checked(11), &[]).await?.get(0);
            if stored {
                failures.push(format!("{name}: database changed after the refusal"));
            }
        }

        // A ctx kept from an earlier invocation is expired: its read is refused with TOKEN_EXPIRED and returns no rows.
        let tx = db.transaction().await?;
        let first = prepare_write_in_with_wire(&tx, &mut worker, &facts, "Event.remember", &json!({"id": "11"}), &caller, wire).await;
        if let Err(error) = first {
            failures.push(format!("remember refused: {} {}", error.code, error.msg));
        }
        tx.rollback().await?;
        let tx = db.transaction().await?;
        match prepare_write_in_with_wire(&tx, &mut worker, &facts, "Event.stale", &json!({"id": "11"}), &caller, wire).await {
            Ok(prepared) if prepared.output == json!({"ok": true}) => {}
            Ok(prepared) => failures.push(format!("stale ctx read was not refused with TOKEN_EXPIRED: {}", prepared.output)),
            Err(error) => failures.push(format!("stale refused the whole extension: {} {}", error.code, error.msg)),
        }
        tx.rollback().await?;
        worker.stop().await;
        Ok(())
    }
    .await;
    let cleanup = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await;
    assert!(run.is_ok(), "database: {run:?}");
    assert!(cleanup.is_ok(), "owned schema cleanup: {cleanup:?}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
