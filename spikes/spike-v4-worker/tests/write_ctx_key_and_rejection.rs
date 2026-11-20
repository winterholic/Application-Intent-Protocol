//! PostgreSQL write extensions against a real PostgreSQL:
//! - C1: `ctx.data.apply(res, name, ids, key)` with the string key of the documented signature runs the transition.
//! - X1: a refused ctx call or a raising extension names its cause class on the `EXTENSION_ERROR` it ends with.
//! - X2: `ctx.reject(code, message)` ends the call with `WIRE.EXTENSION_REJECTED` and the business code, and nothing is committed.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{id_wire::IdWire, plan::Caller, sqlgen};
use spike_v4_worker::{write::prepare_write_in_with_wire, Isolation, Lang, Worker, WorkerLimits};
use std::{fs, path::PathBuf};

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Event {
  fields { id: Id; member: Member; checked: Bool }
  rows read when member = actor
  transition mark { allow member = actor; from checked = false; to checked = true }
  transition other { allow member = actor; from checked = true; to checked = false }
  expose apply mark { target id; bulk maxRows 2 }
  expose apply other { target id; bulk maxRows 2 }
  expose read {
    select id, member, checked
    filter id.eq
    sort id
    budget { rows 10; depth 1; cost 100; deadline 2s }
  }
  extension write keyed { input { id: Event.Id } output { ok: Bool } access Event.mark effect db deadline 5s implementation "w.keyed" }
  extension write badKey { input { id: Event.Id } output { ok: Bool } access Event.mark effect db deadline 5s implementation "w.bad_key" }
  extension write noAccess { input { id: Event.Id } output { ok: Bool } access Event.mark effect db deadline 5s implementation "w.no_access" }
  extension write badFilter { input { id: Event.Id } output { ok: Bool } access read.Event, Event.mark effect db deadline 5s implementation "w.bad_filter" }
  extension write rejects { input { id: Event.Id } output { ok: Bool } access Event.mark effect db deadline 5s implementation "w.rejects" }
  extension write rejectsBadCode { input { id: Event.Id } output { ok: Bool } access Event.mark effect db deadline 5s implementation "w.rejects_bad_code" }
  extension write rejectsLongMessage { input { id: Event.Id } output { ok: Bool } access Event.mark effect db deadline 5s implementation "w.rejects_long" }
  extension write crashes { input { id: Event.Id } output { ok: Bool } access Event.mark effect db deadline 5s implementation "w.crashes" }
  extension write rejectsAfterRefusal { input { id: Event.Id } output { ok: Bool } access Event.mark effect db deadline 5s implementation "w.rejects_after_refusal" }
}
"#;

const NODE: &str = r#"
export async function keyed(input, ctx) { await ctx.data.apply('Event', 'mark', [input.id], 'mark-1'); return { ok: true }; }
export async function bad_key(input, ctx) { await ctx.data.apply('Event', 'mark', [input.id], 'x'.repeat(101)); return { ok: true }; }
export async function no_access(input, ctx) { await ctx.data.apply('Event', 'mark', [input.id]); await ctx.data.apply('Event', 'other', [input.id]); return { ok: true }; }
export async function bad_filter(input, ctx) {
  await ctx.data.apply('Event', 'mark', [input.id]);
  await ctx.data.read({ read: 'Event', select: ['id'], filter: [{ field: 'member', op: 'eq', value: '1' }], limit: 1 });
  return { ok: true };
}
export async function rejects(input, ctx) { await ctx.data.apply('Event', 'mark', [input.id]); throw ctx.reject('INSUFFICIENT_STOCK', '재고가 부족합니다'); }
export async function rejects_bad_code(input, ctx) { throw ctx.reject('not a code!', 'x'); }
export async function rejects_long(input, ctx) { throw ctx.reject('TOO_LONG', 'x'.repeat(201)); }
export async function crashes(input, ctx) { await ctx.data.apply('Event', 'mark', [input.id]); throw new Error('boom'); }
export async function rejects_after_refusal(input, ctx) {
  try { await ctx.data.apply('Event', 'other', [input.id]); } catch (error) {}
  throw ctx.reject('NOT_ALLOWED', 'x');
}
"#;

const PYTHON: &str = r#"
async def keyed(input, ctx):
    await ctx.data.apply('Event', 'mark', [input['id']], 'mark-1')
    return {'ok': True}

async def bad_key(input, ctx):
    await ctx.data.apply('Event', 'mark', [input['id']], 'x' * 101)
    return {'ok': True}

async def no_access(input, ctx):
    await ctx.data.apply('Event', 'mark', [input['id']])
    await ctx.data.apply('Event', 'other', [input['id']])
    return {'ok': True}

async def bad_filter(input, ctx):
    await ctx.data.apply('Event', 'mark', [input['id']])
    await ctx.data.read({'read': 'Event', 'select': ['id'], 'filter': [{'field': 'member', 'op': 'eq', 'value': '1'}], 'limit': 1})
    return {'ok': True}

async def rejects(input, ctx):
    await ctx.data.apply('Event', 'mark', [input['id']])
    raise ctx.reject('INSUFFICIENT_STOCK', '재고가 부족합니다')

async def rejects_bad_code(input, ctx):
    raise ctx.reject('not a code!', 'x')

async def rejects_long(input, ctx):
    raise ctx.reject('TOO_LONG', 'x' * 201)

async def crashes(input, ctx):
    await ctx.data.apply('Event', 'mark', [input['id']])
    raise RuntimeError('boom')

async def rejects_after_refusal(input, ctx):
    try:
        await ctx.data.apply('Event', 'other', [input['id']])
    except Exception:
        pass
    raise ctx.reject('NOT_ALLOWED', 'x')
"#;

struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `[tag] text` of a `Reject.msg`, written out here so the test does not depend on the helper it checks.
fn tag_of(msg: &str) -> Option<&str> {
    msg.strip_prefix('[')?.split_once("] ").map(|(tag, _)| tag)
}

#[tokio::test]
async fn string_keys_causes_and_business_rejections_of_postgres_write_extensions() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let dir = std::env::temp_dir().join(format!("aip-write-dx-{unique}"));
    fs::create_dir(&dir).unwrap();
    let owned = OwnedDir(dir);
    fs::write(owned.0.join("w.mjs"), NODE).unwrap();
    fs::write(owned.0.join("w.py"), PYTHON).unwrap();
    let facts: Value = load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let schema = format!("aip_write_dx_{unique}");
    sqlgen::set_schema(&schema);
    let mut db = spike_v2_read::connect_with_url(aip_test_support::test_database_url!()).await.expect("local PostgreSQL");
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("fresh schema ownership");
    let mut failures: Vec<String> = vec![];
    let run: Result<(), tokio_postgres::Error> = async {
        for statement in ddl.iter().skip(1) {
            db.batch_execute(statement).await?;
        }
        db.batch_execute(&format!(
            "INSERT INTO {schema}.member(id) VALUES (1); INSERT INTO {schema}.event(id,member_id,checked) VALUES (11,1,false)"
        ))
        .await?;
        let caller = Caller { actor_id: Some(1), now: "2026-10-05T00:00:00Z".into() };
        let wire = IdWire::DecimalString;
        let isolation = if cfg!(target_os = "macos") { Isolation::MacNetDeny } else { Isolation::None };
        for lang in [Lang::Node, Lang::Python] {
            let mut worker =
                Worker::try_start_with(lang, owned.0.to_str().unwrap(), isolation, WorkerLimits::default()).await.expect("official worker");
            // (extension, committed row after the call, expected code, expected tag of the msg)
            let cases: [(&str, bool, Option<&str>, Option<&str>); 9] = [
                ("keyed", true, None, None),
                ("badKey", false, Some("WIRE.EXTENSION_ERROR"), Some("ctx.apply.key_format")),
                ("noAccess", false, Some("WIRE.EXTENSION_ERROR"), Some("ctx.apply.access_not_declared")),
                ("badFilter", false, Some("WIRE.EXTENSION_ERROR"), Some("ctx.read.filter_not_exposed")),
                ("rejects", false, Some("WIRE.EXTENSION_REJECTED"), Some("INSUFFICIENT_STOCK")),
                ("rejectsBadCode", false, Some("WIRE.EXTENSION_ERROR"), Some("extension.reject_invalid")),
                ("rejectsLongMessage", false, Some("WIRE.EXTENSION_ERROR"), Some("extension.reject_invalid")),
                ("crashes", false, Some("WIRE.EXTENSION_ERROR"), Some("extension.exception")),
                // A refused ctx call is the first cause: a later rejection does not turn it into a business refusal.
                ("rejectsAfterRefusal", false, Some("WIRE.EXTENSION_ERROR"), Some("ctx.apply.access_not_declared")),
            ];
            for (name, committed, code, tag) in cases {
                db.batch_execute(&format!("UPDATE {schema}.event SET checked=false")).await?;
                let tx = db.transaction().await?;
                let result =
                    prepare_write_in_with_wire(&tx, &mut worker, &facts, &format!("Event.{name}"), &json!({"id": "11"}), &caller, wire).await;
                let label = format!("{lang:?} {name}");
                match (&result, code) {
                    (Ok(prepared), None) if prepared.output == json!({"ok": true}) => tx.commit().await?,
                    (Err(error), Some(code)) if error.code == code && tag_of(&error.msg) == tag => {
                        if name == "rejects" && !error.msg.ends_with("재고가 부족합니다") {
                            failures.push(format!("{label}: the business message is not carried: {}", error.msg));
                        }
                        tx.rollback().await?
                    }
                    (Ok(prepared), _) => {
                        failures.push(format!("{label}: accepted ({})", prepared.output));
                        tx.rollback().await?
                    }
                    (Err(error), _) => {
                        failures.push(format!("{label}: expected {code:?} tag {tag:?}, got {} {}", error.code, error.msg));
                        tx.rollback().await?
                    }
                }
                let stored: bool = db.query_one(&format!("SELECT checked FROM {schema}.event WHERE id=11"), &[]).await?.get(0);
                if stored != committed {
                    failures.push(format!("{label}: stored checked={stored}, expected {committed}"));
                }
            }
            worker.stop().await;
        }
        Ok(())
    }
    .await;
    let cleanup = db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await;
    assert!(run.is_ok(), "database: {run:?}");
    assert!(cleanup.is_ok(), "owned schema cleanup: {cleanup:?}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
