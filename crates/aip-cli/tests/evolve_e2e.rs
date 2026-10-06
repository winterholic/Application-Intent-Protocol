//! End-to-end for schema evolution against PostgreSQL: a database deployed with one program is brought to the next by a
//! plan made from the recorded program. Safe additions keep the data and leave the same schema a fresh deployment would
//! make; removals and renames need a declaration; rules the rows already break refuse the change; `--plan` changes nothing.

use aip_runtime::{Deployment, MigrateReport, PlanKind, Pool};
use std::sync::Arc;

struct Built {
    core: aip_ir::Program,
    plan: aip_plan::Program,
}

fn build(src: &str) -> Built {
    let checked = aip_sema::pipeline::check_source(src);
    assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
    let (core, map) = checked.into_core().expect("no errors");
    let compiled = aip_pg::compile(&core, &map);
    assert!(!compiled.diagnostics.iter().any(|d| d.is_error()), "{:?}", compiled.diagnostics);
    Built { core, plan: compiled.program }
}

async fn fresh(db: &str) -> Pool {
    let admin_url = std::env::var("AIP_TEST_ADMIN_URL").unwrap_or_else(|_| "postgres://localhost/postgres".into());
    let admin = aip_runtime::pool(&admin_url, 1).expect("admin pool");
    let c = admin.get().await.expect("admin connection");
    c.execute(format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)").as_str(), &[]).await.expect("drop");
    c.execute(format!("CREATE DATABASE {db}").as_str(), &[]).await.expect("create");
    let url = admin_url.rsplit_once('/').map(|(base, _)| format!("{base}/{db}")).expect("url");
    aip_runtime::pool(&url, 16).expect("pool")
}

async fn deploy(pool: &Pool, b: &Built, reset: bool) -> anyhow::Result<MigrateReport> {
    let evolve = |old: &aip_ir::Program| aip_pg::evolve::plan(old, &b.core, &b.plan.ddl);
    let d = Deployment { core: &b.core, ddl_version: aip_pg::evolve::DDL_VERSION, evolve: &evolve };
    aip_runtime::migrate_deployed(pool, &b.plan, &d, reset).await
}

async fn plan_of(pool: &Pool, b: &Built) -> aip_runtime::PlanReport {
    let evolve = |old: &aip_ir::Program| aip_pg::evolve::plan(old, &b.core, &b.plan.ddl);
    let d = Deployment { core: &b.core, ddl_version: aip_pg::evolve::DDL_VERSION, evolve: &evolve };
    aip_runtime::plan(pool, &b.plan, &d).await.expect("plan")
}

/// Fails with the lines the two schemas do not share, not with both dumps.
fn assert_same_schema(a: &str, b: &str, what: &str) {
    let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let only_a: Vec<&&str> = la.iter().filter(|l| !lb.contains(l)).collect();
    let only_b: Vec<&&str> = lb.iter().filter(|l| !la.contains(l)).collect();
    assert!(only_a.is_empty() && only_b.is_empty(), "{what}\n  only in the evolved database: {only_a:?}\n  only in the fresh one: {only_b:?}");
}

async fn sql(pool: &Pool, q: &str) {
    pool.get().await.expect("conn").batch_execute(q).await.expect("sql");
}

async fn num(pool: &Pool, q: &str) -> i64 {
    pool.get().await.expect("conn").query_one(q, &[]).await.expect("query").get(0)
}

async fn text(pool: &Pool, q: &str) -> String {
    pool.get().await.expect("conn").query_one(q, &[]).await.expect("query").get(0)
}

/// Columns, indexes, constraints, triggers and functions of the application, one per line, sorted: two databases with the
/// same dump have the same schema (the order of columns inside a table is not part of it, `ADD COLUMN` appends).
async fn dump(pool: &Pool) -> String {
    let rows = pool
        .get()
        .await
        .expect("conn")
        .query(
            "SELECT l FROM (
               SELECT 'col '||table_name||'.'||column_name||' '||data_type||' '||is_nullable||' '||coalesce(column_default,'') AS l FROM information_schema.columns WHERE table_schema='public' AND table_name NOT LIKE '\\_aip\\_%'
               UNION ALL SELECT 'idx '||indexdef FROM pg_indexes WHERE schemaname='public' AND tablename NOT LIKE '\\_aip\\_%'
               UNION ALL SELECT 'con '||conrelid::regclass||' '||conname||' '||pg_get_constraintdef(oid) FROM pg_constraint WHERE connamespace='public'::regnamespace AND conrelid::regclass::text NOT LIKE '%\\_aip\\_%'
               UNION ALL SELECT 'trg '||pg_get_triggerdef(t.oid) FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid WHERE NOT tgisinternal AND c.relnamespace='public'::regnamespace
               UNION ALL SELECT 'fn '||proname||' '||md5(prosrc) FROM pg_proc WHERE pronamespace='public'::regnamespace AND (proname LIKE '%\\_\\_%' OR proname LIKE '\\_aip\\_%')
             ) x ORDER BY l",
            &[],
        )
        .await
        .expect("dump");
    rows.iter().map(|r| r.get::<_, String>(0)).collect::<Vec<_>>().join("\n")
}

const V1: &str = r#"
use postgres

enum Status { OPEN DONE }

entity Task {
  title: Text(1..30)
  status: Status = OPEN
  note: Text?
}
"#;

const V2: &str = r#"
use postgres

enum Status { OPEN REVIEW DONE }

entity Owner {
  name: Text(1..20)
}

entity Task {
  title: Text(1..100)
  status: Status = OPEN
  note: Text?
  priority: Int = 3
  tag: Text?
  owner: Owner?
}

entity Label {
  name: Text(1..20)
}
"#;

#[tokio::test]
async fn additions_keep_the_data_and_make_the_schema_a_fresh_deployment_would() {
    let pool = fresh("aip_e2e_evolve_add").await;
    let (v1, v2) = (build(V1), build(V2));
    assert_eq!(deploy(&pool, &v1, false).await.expect("first deployment").kind, PlanKind::Fresh);
    sql(&pool, "INSERT INTO task (title, note) VALUES ('first', 'n1'), ('second', NULL)").await;

    let r = deploy(&pool, &v2, false).await.expect("v2 is additions only");
    assert_eq!(r.kind, PlanKind::Evolve);
    assert_eq!(num(&pool, "SELECT count(*) FROM task").await, 2, "no row was lost");
    assert_eq!(text(&pool, "SELECT note FROM task WHERE title = 'first'").await, "n1");
    assert_eq!(num(&pool, "SELECT min(priority) FROM task").await, 3, "existing rows get the default of the new field");
    assert_eq!(num(&pool, "SELECT count(*) FROM task WHERE tag IS NULL AND owner_id IS NULL").await, 2, "optional fields start empty");
    sql(&pool, "INSERT INTO task (title, status) VALUES ('third', 'REVIEW')").await;
    assert_eq!(num(&pool, "SELECT count(*) FROM label").await, 0, "the new entity exists");

    let other = fresh("aip_e2e_evolve_add_fresh").await;
    deploy(&other, &v2, false).await.expect("fresh v2");
    assert_same_schema(&dump(&pool).await, &dump(&other).await, "evolving gives the schema a fresh deployment of v2 gives");
    assert_eq!(num(&pool, "SELECT count(*) FROM _aip_deployment").await, 2);

    // the same program again: nothing to do
    let again = deploy(&pool, &v2, false).await.expect("again");
    assert_eq!((again.kind, again.applied), (PlanKind::Unchanged, 0));
}

#[tokio::test]
async fn removing_a_field_needs_a_declaration_and_a_refused_change_changes_nothing() {
    let pool = fresh("aip_e2e_evolve_remove").await;
    deploy(&pool, &build(V1), false).await.expect("v1");
    sql(&pool, "INSERT INTO task (title, note) VALUES ('first', 'keep me')").await;
    let before = dump(&pool).await;

    let v3 = build(&V1.replace("  note: Text?\n", ""));
    let err = deploy(&pool, &v3, false).await.err().expect("a field disappeared without a word").to_string();
    assert!(err.contains("AIP.SCHEMA.UNDECLARED") && err.contains("Task.note"), "{err}");
    assert!(err.contains("removed field note") && err.contains("was note"), "the fix names both ways to say it: {err}");
    assert_eq!(dump(&pool).await, before, "refused: the schema is untouched");
    assert_eq!(text(&pool, "SELECT note FROM task").await, "keep me", "refused: the data is untouched");
    assert_eq!(num(&pool, "SELECT count(*) FROM _aip_deployment").await, 1, "refused: nothing recorded");

    let declared = build(&V1.replace("  note: Text?\n", "  removed field note\n"));
    let r = deploy(&pool, &declared, false).await.expect("declared removal");
    assert!(r.steps.iter().any(|s| s.class == aip_plan::StepClass::Declared), "the destructive step is labelled");
    assert_eq!(num(&pool, "SELECT count(*) FROM information_schema.columns WHERE table_name = 'task' AND column_name = 'note'").await, 0);
    assert_eq!(num(&pool, "SELECT count(*) FROM task").await, 1, "the row stays, only the column went");

    // the declaration may stay in the source: with the field gone from the database it does nothing
    let again = deploy(&pool, &declared, false).await.expect("same program");
    assert_eq!(again.kind, PlanKind::Unchanged);
}

#[tokio::test]
async fn a_rename_declared_with_was_moves_the_values() {
    let pool = fresh("aip_e2e_evolve_rename").await;
    deploy(&pool, &build(V1), false).await.expect("v1");
    sql(&pool, "INSERT INTO task (title, note) VALUES ('first', 'moved')").await;

    // without `was` a rename is a removal plus an addition, which is refused
    let renamed = V1.replace("note: Text?", "remark: Text?");
    let err = deploy(&pool, &build(&renamed), false).await.err().expect("undeclared").to_string();
    assert!(err.contains("AIP.SCHEMA.UNDECLARED") && err.contains("new in Task: remark"), "{err}");

    deploy(&pool, &build(&V1.replace("note: Text?", "remark: Text? was note")), false).await.expect("declared rename");
    assert_eq!(text(&pool, "SELECT remark FROM task").await, "moved", "the value is under the new name");
    assert_eq!(num(&pool, "SELECT count(*) FROM information_schema.columns WHERE table_name = 'task' AND column_name = 'note'").await, 0);

    // an entity too: the table, its rows and the objects named after it
    let items = V1.replace("entity Task", "entity Job was Task").replace("remark: Text?", "note: Text?");
    let src = items.replace("note: Text?", "remark: Text? was note");
    deploy(&pool, &build(&src), false).await.expect("entity rename");
    assert_eq!(text(&pool, "SELECT remark FROM job").await, "moved");
    let other = fresh("aip_e2e_evolve_rename_fresh").await;
    deploy(&other, &build(&src), false).await.expect("fresh");
    assert_same_schema(&dump(&pool).await, &dump(&other).await, "renamed objects match a fresh deployment");
}

#[tokio::test]
async fn a_unique_rule_over_duplicate_rows_is_refused_with_the_count() {
    let pool = fresh("aip_e2e_evolve_unique").await;
    deploy(&pool, &build(V1), false).await.expect("v1");
    sql(&pool, "INSERT INTO task (title) VALUES ('same'), ('same'), ('same'), ('other')").await;
    let before = dump(&pool).await;

    let v2 = build(&V1.replace("  note: Text?\n", "  note: Text?\n  unique (title) else TITLE_TAKEN\n"));
    let err = deploy(&pool, &v2, false).await.err().expect("duplicates").to_string();
    assert!(err.contains("AIP.SCHEMA.DATA_CONFLICT") && err.contains("3 row(s)"), "{err}");
    assert_eq!(dump(&pool).await, before);

    let report = plan_of(&pool, &v2).await;
    assert!(report.blocked());
    assert_eq!(report.checks.iter().filter_map(|c| c.violations).collect::<Vec<_>>(), [3]);
    assert_eq!(report.checks[0].sample.len(), 3, "ids of the offending rows");

    sql(&pool, "DELETE FROM task WHERE title = 'same'").await;
    deploy(&pool, &v2, false).await.expect("after the rows are fixed");
    assert_eq!(num(&pool, "SELECT count(*) FROM pg_indexes WHERE indexdef LIKE '%UNIQUE%(title)%' AND tablename = 'task'").await, 1);
}

#[tokio::test]
async fn rules_the_rows_already_break_are_refused() {
    let pool = fresh("aip_e2e_evolve_tighten").await;
    deploy(&pool, &build(V1), false).await.expect("v1");
    sql(&pool, "INSERT INTO task (title, note) VALUES ('a long title here', NULL)").await;

    // optional -> required while a row is empty
    let err = deploy(&pool, &build(&V1.replace("note: Text?", "note: Text")), false).await.err().expect("nulls").to_string();
    assert!(err.contains("AIP.SCHEMA.DATA_CONFLICT") && err.contains("1 row(s)"), "{err}");
    // narrower limit than a stored value
    let err = deploy(&pool, &build(&V1.replace("Text(1..30)", "Text(1..5)")), false).await.err().expect("too long").to_string();
    assert!(err.contains("AIP.SCHEMA.DATA_CONFLICT"), "{err}");
    // an enum value taken away while a row has it
    sql(&pool, "UPDATE task SET status = 'DONE'").await;
    let err = deploy(&pool, &build(&V1.replace("OPEN DONE", "OPEN")), false).await.err().expect("value in use").to_string();
    assert!(err.contains("AIP.SCHEMA.DATA_CONFLICT"), "{err}");
    // a new required field without a default cannot fill existing rows
    let err = deploy(&pool, &build(&V1.replace("note: Text?", "note: Text?\n  code: Text")), false).await.err().expect("no default").to_string();
    assert!(err.contains("AIP.SCHEMA.DATA_CONFLICT") && err.contains("no default"), "{err}");
    // another base type
    let err = deploy(&pool, &build(&V1.replace("title: Text(1..30)", "title: Int")), false).await.err().expect("type").to_string();
    assert!(err.contains("AIP.SCHEMA.TYPE_CHANGE"), "{err}");
    assert_eq!(num(&pool, "SELECT count(*) FROM _aip_deployment").await, 1, "none of them was recorded");

    // widening is applied by itself
    deploy(&pool, &build(&V1.replace("Text(1..30)", "Text(1..300)").replace("OPEN DONE", "OPEN DONE ARCHIVED")), false).await.expect("widening");
    sql(&pool, "UPDATE task SET status = 'ARCHIVED'").await;
    // with no row using it, a value can go
    sql(&pool, "UPDATE task SET status = 'OPEN'").await;
    deploy(&pool, &build(&V1.replace("Text(1..30)", "Text(1..300)")), false).await.expect("ARCHIVED goes, no row has it");
}

#[tokio::test]
async fn generated_triggers_are_replaced_without_a_reset() {
    let pool = fresh("aip_e2e_evolve_trigger").await;
    let base = format!("{V1}\ncommand AddTask(title: Text(1..30)) {{\n  allow public\n  do {{ insert Task {{ title }} }}\n}}\n");
    deploy(&pool, &build(&base), false).await.expect("a database from before the program had subscriptions");
    assert_eq!(num(&pool, "SELECT count(*) FROM pg_trigger WHERE tgname = 'task__notify'").await, 0);

    let live = format!("{base}\nsubscribe LiveTasks() {{\n  allow public\n  from Task t\n  select {{ id title }}\n}}\n");
    deploy(&pool, &build(&live), false).await.expect("adds a subscription");
    assert_eq!(
        num(&pool, "SELECT count(*) FROM pg_trigger WHERE tgname = 'task__notify'").await,
        1,
        "the notification trigger appeared without --reset"
    );

    // a trigger the generator owns that went missing or stale comes back with the next deployment
    sql(&pool, "DROP TRIGGER task__notify ON task").await;
    sql(&pool, "CREATE TRIGGER task__stale AFTER INSERT ON task FOR EACH STATEMENT EXECUTE FUNCTION \"_aip_notify_changed\"()").await;
    deploy(&pool, &build(&format!("{live}\nconfig unused: Int = 1\n")), false).await.expect("another change");
    assert_eq!(num(&pool, "SELECT count(*) FROM pg_trigger WHERE tgname = 'task__notify'").await, 1);
    assert_eq!(
        num(&pool, "SELECT count(*) FROM pg_trigger WHERE tgname = 'task__stale'").await,
        0,
        "a generated trigger the program no longer has is dropped"
    );
}

#[tokio::test]
async fn plan_prints_the_steps_and_changes_nothing() {
    let pool = fresh("aip_e2e_evolve_plan").await;
    deploy(&pool, &build(V1), false).await.expect("v1");
    sql(&pool, "INSERT INTO task (title) VALUES ('x')").await;
    let (schema, deployments, rows) =
        (dump(&pool).await, num(&pool, "SELECT count(*) FROM _aip_deployment").await, num(&pool, "SELECT count(*) FROM task").await);

    let report = plan_of(&pool, &build(V2)).await;
    assert_eq!(report.kind, PlanKind::Evolve);
    assert!(!report.blocked());
    assert!(report.plan.steps.iter().any(|s| s.sql.contains("ADD COLUMN \"priority\"")), "the plan lists the column it would add");
    let removed = plan_of(&pool, &build(&V1.replace("  note: Text?\n", ""))).await;
    assert!(removed.blocked() && removed.plan.rejections[0].code == "AIP.SCHEMA.UNDECLARED");

    assert_eq!(dump(&pool).await, schema, "the schema is as it was");
    assert_eq!((num(&pool, "SELECT count(*) FROM _aip_deployment").await, num(&pool, "SELECT count(*) FROM task").await), (deployments, rows));

    // a database that was never deployed: everything would be created, and still nothing is
    let empty = fresh("aip_e2e_evolve_plan_empty").await;
    assert_eq!(plan_of(&empty, &build(V1)).await.kind, PlanKind::Fresh);
    assert_eq!(num(&empty, "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public'").await, 0);
}

#[tokio::test]
async fn tables_without_a_record_are_adopted_when_they_match() {
    let pool = fresh("aip_e2e_evolve_adopt").await;
    deploy(&pool, &build(V1), false).await.expect("v1");
    sql(&pool, "DROP TABLE _aip_deployment").await;
    let r = deploy(&pool, &build(V1), false).await.expect("a matching schema without a record");
    assert_eq!(r.kind, PlanKind::Adopt);
    assert_eq!(num(&pool, "SELECT count(*) FROM _aip_deployment").await, 1);
    // a schema that does not match has nothing to be compared with
    sql(&pool, "DROP TABLE _aip_deployment").await;
    let err = deploy(&pool, &build(V2), false).await.err().expect("no record, no match").to_string();
    assert!(err.contains("AIP.SCHEMA.UNRECORDED"), "{err}");
}

#[tokio::test]
async fn processes_starting_together_apply_a_change_once() {
    let pool = Arc::new(fresh("aip_e2e_evolve_race").await);
    deploy(&pool, &build(V1), false).await.expect("v1");
    sql(&pool, "INSERT INTO task (title) VALUES ('x')").await;
    let v2 = Arc::new(build(V2));
    let start = |pool: Arc<Pool>, b: Arc<Built>| tokio::spawn(async move { deploy(&pool, &b, false).await.map(|r| r.kind) });
    let (a, b) = (start(pool.clone(), v2.clone()), start(pool.clone(), v2.clone()));
    let mut kinds = vec![a.await.expect("join").expect("first"), b.await.expect("join").expect("second")];
    kinds.sort_by_key(|k| format!("{k:?}"));
    assert_eq!(kinds, [PlanKind::Evolve, PlanKind::Unchanged], "one applies it, the other finds it done");
    assert_eq!(num(&pool, "SELECT count(*) FROM _aip_deployment").await, 2);
}

#[tokio::test]
async fn an_entity_goes_only_when_the_program_says_so() {
    let pool = fresh("aip_e2e_evolve_drop_entity").await;
    deploy(&pool, &build(V2), false).await.expect("v2 has Label and Owner");
    sql(&pool, "INSERT INTO label (name) VALUES ('x')").await;
    let without = V2.replace("entity Label {\n  name: Text(1..20)\n}\n", "");
    let err = deploy(&pool, &build(&without), false).await.err().expect("undeclared").to_string();
    assert!(err.contains("AIP.SCHEMA.UNDECLARED") && err.contains("entity Label"), "{err}");
    assert_eq!(num(&pool, "SELECT count(*) FROM label").await, 1);
    deploy(&pool, &build(&format!("{without}\nremoved entity Label\n")), false).await.expect("declared");
    assert_eq!(num(&pool, "SELECT count(*) FROM information_schema.tables WHERE table_name = 'label'").await, 0);
}

#[tokio::test]
async fn changes_the_planner_does_not_know_how_to_carry_are_refused() {
    let pool = fresh("aip_e2e_evolve_unsupported").await;
    deploy(&pool, &build(V1), false).await.expect("v1");
    let err = deploy(&pool, &build(&V1.replace("  note: Text?\n", "  note: Text?\n  history\n")), false).await.err().expect("trait").to_string();
    assert!(err.contains("AIP.SCHEMA.UNSUPPORTED") && err.contains("history"), "{err}");
    // an ordered enum whose existing values trade places changes what `>=` means for stored rows
    let ordered = "enum Level ordered { LOW < HIGH }\nentity Ticket { level: Level = LOW }\n";
    let reordered = "enum Level ordered { HIGH < LOW }\nentity Ticket { level: Level = LOW }\n";
    let p2 = fresh("aip_e2e_evolve_unsupported_enum").await;
    deploy(&p2, &build(&format!("use postgres\n{ordered}")), false).await.expect("ordered");
    let err = deploy(&p2, &build(&format!("use postgres\n{reordered}")), false).await.err().expect("reorder").to_string();
    assert!(err.contains("AIP.SCHEMA.UNSUPPORTED") && err.contains("Level"), "{err}");
    // a value in the middle is a change of meaning only for the new value: applied, with a note
    let middle = "enum Level ordered { LOW < MID < HIGH }\nentity Ticket { level: Level = LOW }\n";
    let r = deploy(&p2, &build(&format!("use postgres\n{middle}")), false).await.expect("middle value");
    assert!(r.notes.iter().any(|n| n.contains("Level") && n.contains("MID")), "{:?}", r.notes);
}

#[tokio::test]
async fn the_commands_plan_apply_and_compare_against_the_deployed_program() {
    let pool = fresh("aip_e2e_evolve_cli").await;
    let admin_url = std::env::var("AIP_TEST_ADMIN_URL").unwrap_or_else(|_| "postgres://localhost/postgres".into());
    let url = admin_url.rsplit_once('/').map(|(base, _)| format!("{base}/aip_e2e_evolve_cli")).expect("url");
    let dir = std::env::temp_dir().join("aip-evolve-cli");
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("app.aip");
    let aip = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_aip")).args(args).env("DATABASE_URL", &url).output().expect("run aip");
        (out.status.code(), String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr))
    };
    let path = file.to_str().expect("path");

    std::fs::write(&file, V1).expect("write");
    let (code, out) = aip(&["migrate", path]);
    assert_eq!(code, Some(0), "{out}");
    sql(&pool, "INSERT INTO task (title) VALUES ('x')").await;

    std::fs::write(&file, V2).expect("write");
    let before = dump(&pool).await;
    let (code, out) = aip(&["migrate", "--plan", path]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("[safe]") && out.contains("ADD COLUMN \"priority\"") && out.contains("ready:"), "{out}");
    assert_eq!(dump(&pool).await, before, "--plan changed nothing");
    let (code, out) = aip(&["diff", path, "--against", "deployed"]);
    assert_eq!(code, Some(0), "{out}");

    // the removal without a declaration blocks the plan (exit 1) and the apply
    std::fs::write(&file, V1.replace("  note: Text?\n", "")).expect("write");
    let (code, out) = aip(&["migrate", "--plan", path]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("refused AIP.SCHEMA.UNDECLARED") && out.contains("blocked"), "{out}");
    let (code, out) = aip(&["migrate", path]);
    assert_ne!(code, Some(0), "{out}");
    assert!(out.contains("AIP.SCHEMA.UNDECLARED"), "{out}");
    assert_eq!(dump(&pool).await, before);

    std::fs::write(&file, V2).expect("write");
    let (code, out) = aip(&["migrate", path]);
    assert_eq!(code, Some(0), "{out}");
    // after the change the deployed program is V2: comparing V2 with it finds nothing, and V1 against it breaks nothing it did not add
    let (code, out) = aip(&["diff", path, "--against", "deployed", "--json"]);
    assert_eq!((code, out.trim()), (Some(0), "[]"));
}

#[tokio::test]
async fn rules_the_rows_already_meet_are_applied() {
    let pool = fresh("aip_e2e_evolve_fits").await;
    deploy(&pool, &build(V1), false).await.expect("v1");
    // no rows yet: a required field without a default fits an empty table
    deploy(&pool, &build(&V1.replace("note: Text?", "note: Text?\n  code: Text")), false).await.expect("empty table");
    sql(&pool, "INSERT INTO task (title, code) VALUES ('short', 'c')").await;
    // narrower, and every row fits
    deploy(&pool, &build(&V1.replace("note: Text?", "note: Text?\n  code: Text").replace("Text(1..30)", "Text(1..10)")), false)
        .await
        .expect("rows fit");
    // optional -> required with no empty row, and a reference to another entity, with its foreign key and index
    sql(&pool, "UPDATE task SET note = 'n'").await;
    let v = "enum Status { OPEN DONE }\nentity Owner { name: Text }\nentity Task {\n  title: Text(1..10)\n  status: Status = OPEN\n  note: Text\n  code: Text\n  owner: Owner?\n}\n";
    let next = build(&format!("use postgres\n{v}"));
    deploy(&pool, &next, false).await.expect("required and a new reference");
    let other = fresh("aip_e2e_evolve_fits_fresh").await;
    deploy(&other, &next, false).await.expect("fresh");
    assert_same_schema(&dump(&pool).await, &dump(&other).await, "the same schema as a fresh deployment");
}

#[tokio::test]
async fn renamed_fields_carry_their_constraints_indexes_and_references() {
    let pool = fresh("aip_e2e_evolve_rename_objects").await;
    let v1 = "use postgres\nenum Status { OPEN DONE }\nentity Owner { name: Text }\nentity Task {\n  title: Text(1..30)\n  status: Status = OPEN\n  owner: Owner?\n  unique (title) else TITLE_TAKEN\n}\n";
    let v2 = "use postgres\nenum Status { OPEN DONE }\nentity Owner { name: Text }\nentity Task {\n  headline: Text(1..30) was title\n  state: Status = OPEN was status\n  assignee: Owner? was owner\n  unique (headline) else TITLE_TAKEN\n}\n";
    deploy(&pool, &build(v1), false).await.expect("v1");
    sql(&pool, "INSERT INTO owner (name) VALUES ('o'); INSERT INTO task (title, status, owner_id) SELECT 't', 'DONE', id FROM owner").await;
    deploy(&pool, &build(v2), false).await.expect("renames");
    assert_eq!(text(&pool, "SELECT headline || state FROM task").await, "tDONE");
    assert_eq!(num(&pool, "SELECT count(*) FROM task t JOIN owner o ON o.id = t.assignee_id").await, 1, "the reference still points at its row");
    let other = fresh("aip_e2e_evolve_rename_objects_fresh").await;
    deploy(&other, &build(v2), false).await.expect("fresh");
    assert_same_schema(&dump(&pool).await, &dump(&other).await, "renamed columns carry their objects");
    // the rules still hold under the new names
    assert!(pool.get().await.expect("conn").batch_execute("INSERT INTO task (headline) VALUES ('t')").await.is_err(), "unique rule");
    assert!(pool.get().await.expect("conn").batch_execute("INSERT INTO task (headline, state) VALUES ('u', 'NOPE')").await.is_err(), "enum rule");
}
