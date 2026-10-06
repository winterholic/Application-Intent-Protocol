//! End-to-end for the SaaS example: tenant isolation as a security boundary.
//!
//! Four layers keep two workspaces apart: the application's own membership
//! checks (visibility, `allow`), the intent rules the compiler derives from
//! `tenant via` (one tenant per call, filtered reads, writes into the call's
//! tenant), the database triggers that refuse a reference across tenants, and the
//! trigger that pins every transaction's writes to one tenant, which is what binds
//! the contexts with no caller (webhooks, handlers, schedules, rules, job items).
//! The probes below are the attacks only the last three layers can stop, run by
//! someone who belongs to both workspaces, and the background work that has to stay
//! inside one tenant per item. `saas_negative_controls` runs the same probes with
//! each layer switched off and requires the attack to work, so a passing
//! end-to-end test is not a test that cannot fail.

mod common;

use aip_runtime::engine::{Call, Engine};
use aip_runtime::error::AipError;
use common::{call, drain, fails, ok, sql_one};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const HOOK_SECRET: &str = "tracker_test_only";

static KEY: AtomicUsize = AtomicUsize::new(0);

fn key() -> String {
    format!("k{}", KEY.fetch_add(1, Ordering::Relaxed))
}

fn id(v: &Value) -> String {
    v["id"].as_str().expect("id").to_string()
}

async fn count(e: &Engine, table: &str) -> i64 {
    sql_one(e, &format!("SELECT count(*) AS n FROM {table}")).await.as_i64().expect("count")
}

async fn scalar(e: &Engine, q: &str) -> Value {
    sql_one(e, q).await
}

async fn member(e: &Engine, name: &str) -> String {
    let v = scalar(e, &format!("INSERT INTO member (email) VALUES ('{name}@x.com') RETURNING id")).await;
    v.as_str().expect("id").to_string()
}

async fn workspace(e: &Engine, owner: &str, name: &str) -> String {
    id(&ok(e, "CreateWorkspace", Some(owner), json!({"name": name}), Some(&key())).await)
}

async fn project(e: &Engine, actor: &str, ws: &str, title: &str) -> String {
    id(&ok(e, "CreateProject", Some(actor), json!({"workspace": ws, "title": title}), Some(&key())).await)
}

async fn task(e: &Engine, actor: &str, project: &str, title: &str, assignee: Option<&str>) -> String {
    let mut input = json!({"project": project, "title": title});
    if let Some(a) = assignee {
        input["assignee"] = json!(a);
    }
    id(&ok(e, "CreateTask", Some(actor), input, Some(&key())).await)
}

/// What the SQL layer answers for a statement run behind the application's back, mapped like a runtime error.
async fn raw(e: &Engine, sql: &str) -> Result<u64, AipError> {
    let c = e.pool.get().await.expect("connection");
    c.execute(sql, &[]).await.map_err(|err| aip_runtime::exec::db_error(&e.program, "raw", &err))
}

/// Like [`raw`], inside a transaction that declares itself `cross tenant` the way a `cross tenant` plan does.
async fn raw_cross(e: &Engine, sql: &str) -> Result<u64, AipError> {
    let mut c = e.pool.get().await.expect("connection");
    let map = |err| aip_runtime::exec::db_error(&e.program, "raw", &err);
    let tx = c.transaction().await.map_err(map)?;
    tx.execute("SELECT set_config('aip.tenant_cross', 'on', true)", &[]).await.map_err(map)?;
    let n = tx.execute(sql, &[]).await.map_err(map)?;
    tx.commit().await.map_err(map)?;
    Ok(n)
}

async fn call_internal(e: &Engine, intent: &str) -> Result<(), AipError> {
    e.call_internal(Call { intent: intent.into(), input: json!({}), client: Some("test-client".into()), ..Default::default() }).await.map(|_| ())
}

/// Two workspaces and three people: alice is only in A, bob only in B, carol in both.
struct World {
    e: Arc<Engine>,
    alice: String,
    bob: String,
    carol: String,
    wa: String,
    wb: String,
    pa: String,
    pb: String,
}

async fn world(e: Arc<Engine>) -> World {
    let (alice, bob, carol) = (member(&e, "alice").await, member(&e, "bob").await, member(&e, "carol").await);
    let wa = workspace(&e, &alice, "A").await;
    let wb = workspace(&e, &bob, "B").await;
    ok(&e, "AddMember", Some(&alice), json!({"workspace": wa, "member": carol}), None).await;
    ok(&e, "AddMember", Some(&bob), json!({"workspace": wb, "member": carol}), None).await;
    let pa = project(&e, &alice, &wa, "PA").await;
    let pb = project(&e, &bob, &wb, "PB").await;
    World { e, alice, bob, carol, wa, wb, pa, pb }
}

/// Every attack carol (a member of both workspaces) can try, each on rows of its own, and what became of it.
struct Probes {
    /// `MoveTask` of a task of A into a project of B, and where the task is afterwards.
    move_task: Result<(), AipError>,
    moved: bool,
    /// `CreateTask` in A whose parent task is in B, and how many rows named `child` exist afterwards.
    foreign_parent: Result<(), AipError>,
    children: i64,
    /// `MoveToMyDefault` when the caller's default project is in B.
    default_project: Result<(), AipError>,
    default_moved: bool,
    /// `CloneToMyDefault`, an insert into that same foreign project, and how many rows named `clone-me` exist afterwards.
    clone_default: Result<(), AipError>,
    clones: i64,
    /// `PinParent` with a pinned task of B: no parameter names it, only the database can notice.
    pin_parent: Result<(), AipError>,
    pinned: bool,
    /// Titles `MyTasks(A)` returns, with carol's tasks in A and in B, and the count `OpenTaskCount(A)` answers against the truth.
    my_tasks: Vec<String>,
    open_count: i64,
    open_truth: i64,
    /// Raw SQL: a task of A with a parent of B, moving a project that has another project's task pointing into it, and a harmless move.
    raw_foreign_parent: Result<u64, AipError>,
    raw_unsafe_move: Result<u64, AipError>,
    raw_clean_move: Result<u64, AipError>,
    clean_moved: bool,
    /// Raw SQL in a `cross tenant` transaction: the same kind of move works, a reference across tenants still does not.
    raw_cross_move: Result<u64, AipError>,
    cross_moved: bool,
    raw_cross_foreign_parent: Result<u64, AipError>,
    /// The internal `cross tenant` command archived done tasks of both workspaces; the public call is refused.
    archived_both: bool,
    internal_is_hidden: bool,
}

async fn probes(w: &World) -> Probes {
    let e = &w.e;
    let c = Some(w.carol.as_str());
    let (wa, wb, pa, pb) = (&w.wa, &w.wb, &w.pa, &w.pb);
    let tb = task(e, &w.bob, pb, "B-parent", None).await;

    let t1 = task(e, &w.carol, pa, "move-me", None).await;
    let move_task = call(e, "MoveTask", c, json!({"task": t1, "project": pb}), None).await.map(|_| ());
    let moved = scalar(e, &format!("SELECT project_id FROM task WHERE id = '{t1}'")).await == json!(pb);

    let foreign_parent = call(e, "CreateTask", c, json!({"project": pa, "title": "child", "parent": tb}), Some(&key())).await.map(|_| ());
    let children = scalar(e, "SELECT count(*) AS n FROM task WHERE title = 'child'").await.as_i64().expect("count");

    let t2 = task(e, &w.carol, pa, "default-me", None).await;
    raw(e, &format!("UPDATE member SET default_project_id = '{pb}' WHERE id = '{}'", w.carol)).await.expect("default project");
    let default_project = call(e, "MoveToMyDefault", c, json!({"task": t2}), None).await.map(|_| ());
    let default_moved = scalar(e, &format!("SELECT project_id FROM task WHERE id = '{t2}'")).await == json!(pb);

    let t2b = task(e, &w.carol, pa, "clone-me", None).await;
    let clone_default = call(e, "CloneToMyDefault", c, json!({"task": t2b}), Some(&key())).await.map(|_| ());
    let clones = scalar(e, "SELECT count(*) AS n FROM task WHERE title = 'clone-me'").await.as_i64().expect("count");

    let t3 = task(e, &w.carol, pa, "pin-me", None).await;
    raw(e, &format!("UPDATE member SET pinned_id = '{tb}' WHERE id = '{}'", w.carol)).await.expect("pinned task");
    let pin_parent = call(e, "PinParent", c, json!({"task": t3}), None).await.map(|_| ());
    let pinned = scalar(e, &format!("SELECT parent_id FROM task WHERE id = '{t3}'")).await == json!(tb);

    task(e, &w.carol, pa, "carol-in-A", Some(&w.carol)).await;
    task(e, &w.carol, pb, "carol-in-B", Some(&w.carol)).await;
    let listed = ok(e, "MyTasks", c, json!({"workspace": wa}), None).await;
    let my_tasks = listed.as_array().expect("list").iter().map(|t| t["title"].as_str().expect("title").to_string()).collect();
    let open_count = ok(e, "OpenTaskCount", c, json!({"workspace": wa}), None).await["open"].as_i64().expect("count");
    let open_truth = scalar(
        e,
        &format!("SELECT count(*) AS n FROM task t JOIN project p ON p.id = t.project_id WHERE p.workspace_id = '{wa}' AND t.status = 'OPEN'"),
    )
    .await
    .as_i64()
    .expect("count");

    let ta = task(e, &w.alice, pa, "raw-child", None).await;
    let raw_foreign_parent = raw(e, &format!("UPDATE task SET parent_id = '{tb}' WHERE id = '{ta}'")).await;

    // y1 (in project PY) has parent x1 (in project PX); both are in A until PX moves to B
    let (px, py) = (project(e, &w.alice, wa, "PX").await, project(e, &w.alice, wa, "PY").await);
    let x1 = task(e, &w.alice, &px, "x1", None).await;
    let y1 = task(e, &w.alice, &py, "y1", None).await;
    raw(e, &format!("UPDATE task SET parent_id = '{x1}' WHERE id = '{y1}'")).await.expect("same-tenant parent");
    let raw_unsafe_move = raw(e, &format!("UPDATE project SET workspace_id = '{wb}' WHERE id = '{px}'")).await;

    // nothing points into this project from elsewhere, so it may change workspace, and its comment follows
    let pz = project(e, &w.alice, wa, "PZ").await;
    let z1 = task(e, &w.alice, &pz, "z1", None).await;
    ok(e, "AddComment", Some(&w.alice), json!({"task": z1, "body": "hi"}), Some(&key())).await;
    let raw_clean_move = raw(e, &format!("UPDATE project SET workspace_id = '{wb}' WHERE id = '{pz}'")).await;
    let clean_moved = scalar(e, &format!("SELECT workspace_id FROM project WHERE id = '{pz}'")).await == json!(wb);

    // the same move, but declared cross tenant; and a bare reference to another tenant in such a transaction
    let pw = project(e, &w.alice, wa, "PW").await;
    let raw_cross_move = raw_cross(e, &format!("UPDATE project SET workspace_id = '{wb}' WHERE id = '{pw}'")).await;
    let cross_moved = scalar(e, &format!("SELECT workspace_id FROM project WHERE id = '{pw}'")).await == json!(wb);
    let tc = task(e, &w.alice, pa, "cross-parent", None).await;
    let raw_cross_foreign_parent = raw_cross(e, &format!("UPDATE task SET parent_id = '{tb}' WHERE id = '{tc}'")).await;

    let done_a = task(e, &w.alice, pa, "done-a", None).await;
    let done_b = task(e, &w.bob, pb, "done-b", None).await;
    ok(e, "CompleteTask", Some(&w.alice), json!({"task": done_a}), None).await;
    ok(e, "CompleteTask", Some(&w.bob), json!({"task": done_b}), None).await;
    let public = call(e, "ArchiveDoneTasks", None, json!({}), None).await.err().map(|x| x.code);
    let internal_is_hidden = public.as_deref() == Some("AIP.REQUEST.UNKNOWN_INTENT");
    call_internal(e, "ArchiveDoneTasks").await.expect("internal cross tenant command");
    let archived = scalar(e, &format!("SELECT count(*) AS n FROM task WHERE id IN ('{done_a}', '{done_b}') AND status = 'ARCHIVED'")).await;
    let archived_both = archived == json!(2);

    Probes {
        move_task,
        moved,
        foreign_parent,
        children,
        default_project,
        default_moved,
        clone_default,
        clones,
        pin_parent,
        pinned,
        my_tasks,
        open_count,
        open_truth,
        raw_foreign_parent,
        raw_unsafe_move,
        raw_clean_move,
        clean_moved,
        raw_cross_move,
        cross_moved,
        raw_cross_foreign_parent,
        archived_both,
        internal_is_hidden,
    }
}

fn hook_secret() {
    static SET: std::sync::Once = std::sync::Once::new();
    // SAFETY: set once, before the first webhook is received, to a value nothing else writes
    SET.call_once(|| unsafe { std::env::set_var("TRACKER_SECRET", HOOK_SECRET) });
}

/// A genuine tracker delivery naming two tasks, applied by the dispatcher.
async fn deliver(e: &Engine, id: &str, event: &str, first: &str, second: &str) {
    hook_secret();
    let body = json!({"id": id, "type": event, "data": {"first": first, "second": second}}).to_string();
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(HOOK_SECRET.as_bytes()).expect("key");
    mac.update(body.as_bytes());
    let sig: String = mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let headers = HashMap::from([("x-signature".to_string(), sig)]);
    aip_runtime::webhook::receive(e, "Tracker", &headers, body.as_bytes()).await.expect("acknowledged");
    drain(e).await;
}

async fn status(e: &Engine, task: &str) -> String {
    scalar(e, &format!("SELECT status FROM task WHERE id = '{task}'")).await.as_str().expect("status").to_string()
}

async fn run_jobs(e: &Engine) {
    while aip_runtime::jobs::tick(e).await.expect("job worker") {}
}

/// What the work that has no caller did, in a world where carol belongs to both workspaces.
struct Background {
    /// A tracker payload that names a task of A and one of B: what the dispatcher recorded, and whether anything changed.
    mixed_error: Option<String>,
    mixed_applied: bool,
    /// One naming two tasks of A, and a `cross tenant` handler naming one of each.
    same_ok: bool,
    cross_hook_ok: bool,
    /// `TaskCompleted` handlers ran for a task of each workspace.
    events_ok: bool,
    /// The sweep that handles one task at a time in one transaction, then the `cross tenant` statement that undoes it.
    sweep: Result<(), String>,
    swept: bool,
    restore: Result<(), String>,
    restored: bool,
    /// The job of A: tasks of A it archived, rows it was told to walk, and whether B's done task was left alone.
    job_total: i64,
    job_total_truth: i64,
    job_left_b_alone: bool,
    job_archived_a: bool,
    /// One item per member in one batch: the job's status and error, and welcome tasks created in A and in B.
    welcome: (String, Option<String>),
    welcomed: (i64, i64),
    /// Sign-off of a task of A by a reviewer of B, what the status says to them, and by A's own reviewer.
    foreign_signoff: Result<(), AipError>,
    foreign_can_vote: bool,
    own_signoff: Result<(), AipError>,
    signed_off: bool,
    /// One transaction that closed tasks of both workspaces: did the rule fire in each.
    close_all: Result<(), AipError>,
    retro: (i64, i64),
}

async fn background(w: &World) -> Background {
    let e = &w.e;
    let (wa, wb, pa, pb) = (&w.wa, &w.wb, &w.pa, &w.pb);

    // a webhook handler writes one tenant, whatever its payload names
    let ta1 = task(e, &w.alice, pa, "h-a1", None).await;
    let ta2 = task(e, &w.alice, pa, "h-a2", None).await;
    let tb1 = task(e, &w.bob, pb, "h-b1", None).await;
    deliver(e, "evt-mixed", "tasks.closed", &ta1, &tb1).await;
    let mixed_applied = status(e, &ta1).await == "DONE" || status(e, &tb1).await == "DONE";
    let last = scalar(e, "SELECT last_error FROM _aip_outbox WHERE key = 'evt-mixed'").await;
    let mixed_error = last.as_str().map(String::from);
    deliver(e, "evt-same", "tasks.closed", &ta1, &ta2).await;
    let same_ok = status(e, &ta1).await == "DONE" && status(e, &ta2).await == "DONE";

    // events of both workspaces wait in the outbox together; each is handled in the tenant it names
    let ta3 = task(e, &w.alice, pa, "e-a3", None).await;
    let tb2 = task(e, &w.bob, pb, "e-b2", None).await;
    ok(e, "CompleteTask", Some(&w.alice), json!({"task": ta3}), None).await;
    ok(e, "CompleteTask", Some(&w.bob), json!({"task": tb2}), None).await;
    drain(e).await;
    let comments = scalar(e, &format!("SELECT count(*) AS n FROM comment WHERE body = 'completed' AND task_id IN ('{ta3}', '{tb2}')")).await;
    let events_ok = comments == json!(2);

    // `cross tenant on` may name one of each
    ok(e, "CompleteTask", Some(&w.bob), json!({"task": tb1}), None).await;
    deliver(e, "evt-cross", "tasks.reopened", &ta1, &tb1).await;
    let cross_hook_ok = status(e, &ta1).await == "OPEN" && status(e, &tb1).await == "OPEN";

    // a sweep over done tasks of both workspaces in one transaction, then the cross tenant statement that restores them
    let sweep = aip_runtime::schedule::run_now(e, "ArchiveSweep").await.map_err(|x| x.to_string());
    let swept = status(e, &ta2).await == "ARCHIVED" && status(e, &tb2).await == "ARCHIVED";
    let restore = aip_runtime::schedule::run_now(e, "RestoreArchived").await.map_err(|x| x.to_string());
    let restored = status(e, &ta2).await == "DONE" && status(e, &tb2).await == "DONE";

    // a job started for A walks A's done tasks only; carol, who may see B's too, starts it, so visibility is not what keeps B out
    raw(e, &format!("UPDATE membership SET role = 'ADMIN' WHERE member_id = '{}' AND workspace_id = '{wa}'", w.carol)).await.expect("admin");
    let a_done =
        format!("SELECT count(*) AS n FROM task t JOIN project p ON p.id = t.project_id WHERE p.workspace_id = '{wa}' AND t.status = 'DONE'");
    let job_total_truth = scalar(e, &a_done).await.as_i64().expect("count");
    ok(e, "ArchiveDone", Some(&w.carol), json!({"workspace": wa}), None).await;
    run_jobs(e).await;
    let job_total = scalar(e, "SELECT total FROM _aip_job WHERE name = 'ArchiveDone'").await.as_i64().unwrap_or(-1);
    let job_left_b_alone = status(e, &tb2).await == "DONE";
    let job_archived_a = status(e, &ta2).await == "ARCHIVED" && scalar(e, &a_done).await == json!(0);

    // a job whose items belong to different tenants, in one batch
    raw(e, &format!("UPDATE member SET default_project_id = '{pa}' WHERE id = '{}'", w.alice)).await.expect("default project");
    raw(e, &format!("UPDATE member SET default_project_id = '{pb}' WHERE id = '{}'", w.bob)).await.expect("default project");
    ok(e, "WelcomeDefaults", Some(&w.alice), json!({}), None).await;
    run_jobs(e).await;
    let job = scalar(e, "SELECT status || '|' || coalesce(error, '') FROM _aip_job WHERE name = 'WelcomeDefaults'").await;
    let job = job.as_str().unwrap_or_default().to_string();
    let (st, err) = job.split_once('|').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or_default();
    let welcome = (st, (!err.is_empty()).then_some(err));
    let in_project = |p: String| format!("SELECT count(*) AS n FROM task WHERE title = 'welcome' AND project_id = '{p}'");
    let welcomed =
        (scalar(e, &in_project(pa.clone())).await.as_i64().expect("count"), scalar(e, &in_project(pb.clone())).await.as_i64().expect("count"));

    // sign-off: the approvers named `Reviewer r` are those of the task's own workspace
    ok(e, "AddReviewer", Some(&w.alice), json!({"project": pa, "member": w.alice}), None).await;
    ok(e, "AddReviewer", Some(&w.bob), json!({"project": pb, "member": w.carol}), None).await;
    let ts = task(e, &w.alice, pa, "sign-me", None).await;
    ok(e, "RequestTaskSignoff", Some(&w.alice), json!({"task": ts}), None).await;
    let seen = ok(e, "TaskSignoffStatus", Some(&w.carol), json!({"task": ts}), None).await;
    let foreign_can_vote = seen["canVote"] == json!(true);
    let foreign_signoff = call(e, "ApproveTaskSignoff", Some(&w.carol), json!({"task": ts}), None).await.map(|_| ());
    let own_signoff =
        if foreign_signoff.is_ok() { Ok(()) } else { call(e, "ApproveTaskSignoff", Some(&w.alice), json!({"task": ts}), None).await.map(|_| ()) };
    let signed_off = status(e, &ts).await == "DONE";

    // one transaction closes the open tasks of every workspace; the rule settles each project in its own tenant
    let (ra, rb) = (project(e, &w.alice, wa, "R-A").await, project(e, &w.bob, wb, "R-B").await);
    for (who, p) in [(&w.alice, &ra), (&w.bob, &rb)] {
        task(e, who, p, "r1", None).await;
        task(e, who, p, "r2", None).await;
    }
    let close_all = call_internal(e, "CloseAllOpen").await;
    let retros = |p: &String| format!("SELECT count(*) AS n FROM task WHERE title = 'retro' AND project_id = '{p}'");
    let retro = (scalar(e, &retros(&ra)).await.as_i64().expect("count"), scalar(e, &retros(&rb)).await.as_i64().expect("count"));

    Background {
        mixed_error,
        mixed_applied,
        same_ok,
        cross_hook_ok,
        events_ok,
        sweep,
        swept,
        restore,
        restored,
        job_total,
        job_total_truth,
        job_left_b_alone,
        job_archived_a,
        welcome,
        welcomed,
        foreign_signoff,
        foreign_can_vote,
        own_signoff,
        signed_off,
        close_all,
        retro,
    }
}

fn mismatch(r: &Result<(), AipError>) -> bool {
    r.as_ref().err().is_some_and(|e| e.code == "AIP.TENANT.MISMATCH")
}

fn mismatch_forbidden(r: &Result<(), AipError>) -> bool {
    r.as_ref().err().is_some_and(|e| e.code == "AIP.AUTH.FORBIDDEN")
}

fn mismatch_u64(r: &Result<u64, AipError>) -> bool {
    r.as_ref().err().is_some_and(|e| e.code == "AIP.TENANT.MISMATCH")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saas_end_to_end() {
    let e = common::setup_app("saas", "aip_e2e_saas").await;
    let w = world(e.clone()).await;
    let e = &w.e;

    // --- one workspace behaves like an ordinary app
    let t1 = task(e, &w.alice, &w.pa, "write docs", Some(&w.alice)).await;
    let c1 = ok(e, "AddComment", Some(&w.alice), json!({"task": t1, "body": "first"}), Some(&key())).await;
    assert_eq!(c1["body"], json!("first"));
    let listed = ok(e, "ProjectTasks", Some(&w.alice), json!({"project": w.pa}), None).await;
    assert_eq!(listed.as_array().map(|a| a.len()), Some(1));
    assert_eq!(ok(e, "OpenTaskCount", Some(&w.alice), json!({"workspace": w.wa}), None).await["open"], json!(1));
    let comments = ok(e, "TaskComments", Some(&w.alice), json!({"task": t1}), None).await;
    assert_eq!(comments[0]["body"], json!("first"));
    // `MyWorkspaces` lists workspaces, which are not tenant-scoped, across tenants
    assert_eq!(ok(e, "MyWorkspaces", Some(&w.carol), json!({}), None).await.as_array().map(|a| a.len()), Some(2));

    // --- someone from A reaching for B's rows by id: the rows do not exist for them, and nothing changes
    let before = (count(e, "project").await, count(e, "task").await, count(e, "comment").await);
    let title_before = scalar(e, &format!("SELECT title FROM project WHERE id = '{}'", w.pb)).await;
    for (intent, input) in [
        ("GetProject", json!({"project": w.pb})),
        ("RenameProject", json!({"project": w.pb, "title": "pwned"})),
        ("CreateTask", json!({"project": w.pb, "title": "pwned"})),
        ("ProjectTasks", json!({"project": w.pb})),
        ("OpenTaskCount", json!({"workspace": w.wb})),
    ] {
        let k = (intent == "CreateTask").then(key);
        let err = fails(e, intent, Some(&w.alice), input, k.as_deref()).await;
        let wants_not_found = intent != "OpenTaskCount";
        if wants_not_found {
            assert_eq!(err.code, "AIP.NOT_FOUND", "{intent}: {err}");
            assert_eq!(err.status(), 404);
        } else {
            // the root itself is not tenant-scoped and has no visibility rule: the membership check answers
            assert_eq!(err.code, "AIP.AUTH.FORBIDDEN", "{intent}: {err}");
        }
    }
    assert_eq!((count(e, "project").await, count(e, "task").await, count(e, "comment").await), before);
    assert_eq!(scalar(e, &format!("SELECT title FROM project WHERE id = '{}'", w.pb)).await, title_before);

    // --- the attacks only the tenant rules stop, by someone who belongs to both
    let p = probes(&w).await;
    let task_rows = count(e, "task").await;
    assert!(mismatch(&p.move_task), "{:?}", p.move_task);
    assert_eq!(p.move_task.as_ref().err().map(|x| x.status()), Some(404), "answers like a missing row");
    assert!(!p.moved);
    assert!(mismatch(&p.foreign_parent), "{:?}", p.foreign_parent);
    assert_eq!(p.children, 0, "a refused call writes nothing");
    assert!(mismatch(&p.default_project), "{:?}", p.default_project);
    assert!(!p.default_moved);
    assert!(mismatch(&p.clone_default) && p.clones == 1, "an insert into another tenant is refused and writes nothing: {:?}", p.clone_default);
    assert!(mismatch(&p.pin_parent), "the database refuses the cross-tenant parent: {:?}", p.pin_parent);
    assert!(!p.pinned);
    assert!(!p.my_tasks.iter().any(|t| t == "carol-in-B"), "B's row leaked into A's list: {:?}", p.my_tasks);
    assert!(p.my_tasks.iter().any(|t| t == "carol-in-A"));
    assert_eq!(p.open_count, p.open_truth, "the count saw only A's tasks");
    assert!(mismatch_u64(&p.raw_foreign_parent), "{:?}", p.raw_foreign_parent);
    assert!(mismatch_u64(&p.raw_unsafe_move), "a move that strands a child's parent in another tenant is refused: {:?}", p.raw_unsafe_move);
    assert!(
        mismatch_u64(&p.raw_clean_move) && !p.clean_moved,
        "a write that moves a row between tenants is two tenants' write: {:?}",
        p.raw_clean_move
    );
    assert!(p.raw_cross_move.is_ok() && p.cross_moved, "a transaction that declares cross tenant may move it: {:?}", p.raw_cross_move);
    assert!(mismatch_u64(&p.raw_cross_foreign_parent), "a reference across tenants is refused even there: {:?}", p.raw_cross_foreign_parent);
    assert!(p.archived_both && p.internal_is_hidden, "cross tenant work runs only as an internal intent");
    assert!(task_rows > 0);

    // --- work with no caller: every context writes one tenant
    let b = background(&w).await;
    assert!(!b.mixed_applied, "a payload that names two workspaces changes nothing");
    assert!(b.mixed_error.as_deref().is_some_and(|m| m.contains("AIP.TENANT.MISMATCH")), "{:?}", b.mixed_error);
    assert!(b.same_ok, "the same payload naming one workspace is applied");
    assert!(b.cross_hook_ok, "a cross tenant handler may name both");
    assert!(b.events_ok, "events of two workspaces are each handled in their own");
    assert!(b.sweep.is_ok() && b.swept, "a sweep over both workspaces in one transaction: {:?}", b.sweep);
    assert!(b.restore.is_ok() && b.restored, "a cross tenant schedule updates both: {:?}", b.restore);
    assert_eq!(b.job_total, b.job_total_truth, "the job walked only the tasks of its own workspace");
    assert!(b.job_total > 0 && b.job_left_b_alone && b.job_archived_a, "the job left B's done task alone");
    assert_eq!(b.welcome, ("DONE".to_string(), None), "items of two tenants in one batch");
    assert_eq!(b.welcomed, (1, 2));
    assert!(mismatch_forbidden(&b.foreign_signoff), "a reviewer of another workspace cannot sign off: {:?}", b.foreign_signoff);
    assert!(!b.foreign_can_vote);
    assert!(b.own_signoff.is_ok() && b.signed_off, "{:?}", b.own_signoff);
    assert!(b.close_all.is_ok(), "{:?}", b.close_all);
    assert_eq!(b.retro, (1, 1), "the rule settled the projects of both workspaces");

    // --- two parameters of different tenants in one call, checked before anything is written
    let ta = task(e, &w.carol, &w.pa, "stay", None).await;
    let tb = task(e, &w.carol, &w.pb, "other", None).await;
    let rows = count(e, "task").await;
    let err = fails(e, "CreateTask", Some(&w.carol), json!({"project": w.pa, "title": "mix", "parent": tb}), Some(&key())).await;
    assert_eq!((err.code.as_str(), err.intent.as_str()), ("AIP.TENANT.MISMATCH", "CreateTask"));
    assert_eq!(count(e, "task").await, rows);
    let err = fails(e, "MoveTask", Some(&w.carol), json!({"task": ta, "project": w.pb}), None).await;
    assert_eq!(err.code, "AIP.TENANT.MISMATCH");
    // within one tenant the same calls work
    let pa2 = project(e, &w.carol, &w.wa, "PA2").await;
    ok(e, "MoveTask", Some(&w.carol), json!({"task": ta, "project": pa2}), None).await;
    assert_eq!(scalar(e, &format!("SELECT project_id FROM task WHERE id = '{ta}'")).await, json!(pa2));
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Layer {
    /// Intent rules off (every intent behaves as `cross tenant`); triggers stay.
    IntentRules,
    /// All tenant triggers dropped (references and the pin); intent rules stay.
    Triggers,
    /// Only the transaction pin dropped; intent rules and the reference triggers stay.
    Pin,
    /// The steps that start a context (handler, schedule item, rule row, job item) no longer reset the pin; the pin trigger stays.
    Reset,
    /// All off: the application as if `tenant via` had not been written.
    Both,
}

async fn engine_without(layer: Layer, db: &str) -> Arc<Engine> {
    let path = format!("{}/../../examples/saas/app.aip", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(path).expect("example source");
    let (mut core, map) = aip_sema::pipeline::check_source(&src).into_core().expect("saas checks clean");
    if matches!(layer, Layer::IntentRules | Layer::Both) {
        for i in core.intents.values_mut() {
            match i {
                aip_ir::Intent::Query(q) => q.cross_tenant = true,
                aip_ir::Intent::Command(c) => c.cross_tenant = true,
            }
        }
    }
    if layer == Layer::Both {
        for e in core.entities.values_mut() {
            e.traits.tenant = None;
        }
    }
    let mut program = aip_pg::compile(&core, &map).program;
    match layer {
        Layer::Triggers | Layer::Both => program.ddl.retain(|s| !s.contains("__tenant")),
        Layer::Pin => program.ddl.retain(|s| !s.contains("__tenant_pin")),
        Layer::Reset => drop_context_pins(&mut program),
        Layer::IntentRules => {}
    }
    common::setup_program(db, program).await
}

/// Removes the step that starts each execution context sharing a transaction. The `cross` switch stays: it is a declaration, not a reset.
fn drop_context_pins(program: &mut aip_plan::Program) {
    fn strip(steps: &mut Vec<aip_plan::Step>) {
        steps.retain(|s| !matches!(s, aip_plan::Step::Exec { label, .. } if label.starts_with("tenant: pin") || label == "tenant: reset"));
    }
    program.handlers.iter_mut().for_each(|h| strip(&mut h.steps));
    program.schedules.iter_mut().for_each(|h| strip(&mut h.steps));
    program.rules.iter_mut().for_each(|h| strip(&mut h.steps));
    program.jobs.iter_mut().for_each(|h| strip(&mut h.steps));
    program.webhooks.iter_mut().flat_map(|w| w.handlers.iter_mut()).for_each(|h| strip(&mut h.steps));
}

/// The same attacks, with a layer removed, must succeed: each one is stopped by exactly the layer named.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saas_negative_controls() {
    // intent rules off: nothing checks the call, filters its reads or pins its writes; the triggers still refuse a bad reference
    let w = world(engine_without(Layer::IntentRules, "aip_e2e_saas_no_intent_rules").await).await;
    let p = probes(&w).await;
    assert!(p.move_task.is_ok() && p.moved, "without the call check a task moves to another tenant: {:?}", p.move_task);
    assert!(p.default_project.is_ok() && p.default_moved, "without the write guard a task follows a foreign default project");
    assert!(p.clone_default.is_ok() && p.clones == 2, "without the write guard a copy lands in another tenant: {:?}", p.clone_default);
    assert!(p.my_tasks.iter().any(|t| t == "carol-in-B"), "without the read filter B's row shows in A's list: {:?}", p.my_tasks);
    assert_ne!(p.open_count, p.open_truth, "without the read filter the count spans both workspaces");
    assert!(mismatch(&p.foreign_parent) && p.children == 0, "the trigger still refuses a foreign parent: {:?}", p.foreign_parent);
    assert!(mismatch(&p.pin_parent) && !p.pinned);
    assert!(mismatch_u64(&p.raw_foreign_parent) && mismatch_u64(&p.raw_unsafe_move));
    // the pin is not an intent rule: a raw write still cannot span tenants
    assert!(mismatch_u64(&p.raw_clean_move) && !p.clean_moved);
    // intents declared cross tenant do not pin, so the same move through them is a plain cross tenant write
    let b = background(&w).await;
    assert!(!b.mixed_applied && b.job_total == b.job_total_truth, "contexts that are not intents keep their rules");

    // triggers dropped: the intent rules still stop everything a parameter or a write names, but not a bare reference
    let w = world(engine_without(Layer::Triggers, "aip_e2e_saas_no_triggers").await).await;
    let p = probes(&w).await;
    assert!(mismatch(&p.move_task) && !p.moved);
    assert!(mismatch(&p.foreign_parent) && p.children == 0);
    assert!(mismatch(&p.default_project) && !p.default_moved);
    assert!(mismatch(&p.clone_default) && p.clones == 1);
    assert!(!p.my_tasks.iter().any(|t| t == "carol-in-B") && p.open_count == p.open_truth);
    assert!(p.pin_parent.is_ok() && p.pinned, "without the trigger a cross-tenant parent is stored: {:?}", p.pin_parent);
    assert!(p.raw_foreign_parent.is_ok() && p.raw_unsafe_move.is_ok(), "without the trigger raw writes strand rows in another tenant");
    assert!(p.raw_clean_move.is_ok() && p.clean_moved && p.raw_cross_foreign_parent.is_ok());
    let b = background(&w).await;
    assert!(b.mixed_applied && b.mixed_error.is_none(), "without the pin a webhook payload changes tasks of two workspaces");

    // only the pin dropped: the webhook is the attack that only it stops; everything an intent rule or a reference trigger stops stays stopped
    let w = world(engine_without(Layer::Pin, "aip_e2e_saas_no_pin").await).await;
    let p = probes(&w).await;
    assert!(mismatch(&p.move_task) && !p.moved && mismatch(&p.foreign_parent) && mismatch(&p.pin_parent));
    assert!(p.raw_clean_move.is_ok() && p.clean_moved, "without the pin a raw write may move a row between tenants: {:?}", p.raw_clean_move);
    assert!(mismatch_u64(&p.raw_cross_foreign_parent), "the reference trigger does not depend on the pin");
    let b = background(&w).await;
    assert!(b.mixed_applied && b.mixed_error.is_none(), "without the pin the payload naming A's and B's tasks goes through");
    assert!(b.same_ok && b.cross_hook_ok && b.job_total == b.job_total_truth && b.job_left_b_alone, "the rest does not need the pin");

    // contexts no longer reset between items: a batch or sweep that holds two tenants fails on its second item
    let w = world(engine_without(Layer::Reset, "aip_e2e_saas_no_reset").await).await;
    let b = background(&w).await;
    assert!(!b.mixed_applied, "the pin still refuses the mixed payload");
    assert!(b.sweep.is_err() && !b.swept, "without the per-item reset the sweep stops at its second tenant: {:?}", b.sweep);
    assert_ne!(b.welcome.0, "DONE", "without the per-item reset a job batch of two tenants fails");
    assert!(b.welcome.1.as_deref().is_some_and(|m| m.contains("AIP.TENANT.MISMATCH")), "{:?}", b.welcome);
    assert_eq!(b.welcomed.0 + b.welcomed.1, 0, "the failed batch committed nothing");

    // both dropped: every probe is a working attack
    let w = world(engine_without(Layer::Both, "aip_e2e_saas_off").await).await;
    let p = probes(&w).await;
    assert!(p.move_task.is_ok() && p.moved);
    assert!(p.foreign_parent.is_ok() && p.children == 1);
    assert!(p.default_project.is_ok() && p.default_moved);
    assert!(p.clone_default.is_ok() && p.clones == 2);
    assert!(p.pin_parent.is_ok() && p.pinned);
    assert!(p.my_tasks.iter().any(|t| t == "carol-in-B") && p.open_count != p.open_truth);
    assert!(p.raw_foreign_parent.is_ok() && p.raw_unsafe_move.is_ok());
    // cross tenant work and the harmless move work in every configuration that has no pin
    assert!(p.archived_both && p.clean_moved);
    let b = background(&w).await;
    assert!(b.mixed_applied, "a payload naming two tenants goes through");
    assert!(!b.job_left_b_alone && b.job_total != b.job_total_truth, "the job walked B's tasks as well: {} of {}", b.job_total, b.job_total_truth);
    assert!(b.foreign_signoff.is_ok(), "a reviewer of another workspace signed off: {:?}", b.foreign_signoff);
}
