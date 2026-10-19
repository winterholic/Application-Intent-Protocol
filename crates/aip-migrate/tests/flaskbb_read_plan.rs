use futures_util::FutureExt;
use serde_json::{Value, json};
use spike_v1_fixture::{Form, load_str};
use spike_v2_read::plan::{Caller, Plan, plan_read};
use std::{panic::AssertUnwindSafe, path::PathBuf, process::Command};
use tokio_postgres::types::{ToSql, Type};

const DB: &str = aip_test_support::test_database_url!();

#[tokio::test]
async fn explain_seeded_flaskbb_count_with_and_without_jit() {
    let schema = format!("aip_flaskbb_plan_{}", std::process::id());
    let facts = load_str(include_str!("../../../porting/flaskbb/app.aip"), Form::A).unwrap().execution;
    let mut db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    let result = AssertUnwindSafe(async {
        aip_migrate::init(DB, &schema, &facts).await.unwrap();
        let seed = Command::new("python3")
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../porting/flaskbb/seed/install.py"))
            .args(["--schema", &schema, "--admin-email", "plan@example.test", "--welcome"])
            .output()
            .unwrap();
        assert!(seed.status.success(), "{}", String::from_utf8_lossy(&seed.stderr));
        db.batch_execute(std::str::from_utf8(&seed.stdout).unwrap()).await.unwrap();
        spike_v2_read::sqlgen::set_schema(&schema);
        let plan = plan_read(&facts, &json!({"read":"Topic","select":["id","title","post_count"]}), &Caller { actor_id:Some(1), now:"2026-10-19T00:00:00Z".into() }).unwrap();
        let params:Vec<&(dyn ToSql+Sync)> = plan.params.iter().map(|p|p as &(dyn ToSql+Sync)).collect();
        let types=vec![Type::TEXT;params.len()];
        // Same freshly seeded data and SQL as the CLI regression, with a separate
        // bounded diagnostic timeout. The product's declared 2s deadline is unchanged.
        let available:bool=db.query_one("SELECT pg_jit_available()",&[]).await.unwrap().get(0);
        for jit in ["on","off"] {
            db.batch_execute(&format!("SET jit={jit}; SET statement_timeout='30s'")).await.unwrap();
            let statement=db.prepare_typed(&format!("EXPLAIN (ANALYZE, FORMAT JSON) {}",plan.sql),&types).await.unwrap();
            let report:Value=db.query_one(&statement,&params).await.unwrap().get(0);
            println!("{}",json!({"fixture":"flaskbb-seeded-count","jit":jit,"jitAvailable":available,"estimatedCost":report[0]["Plan"]["Total Cost"],"jitReport":report[0]["JIT"],"executionMs":report[0]["Execution Time"],"planningMs":report[0]["Planning Time"]}));
            if jit=="on" && available {assert!(report[0]["JIT"].is_object(),"the baseline must actually invoke JIT");}
            if jit=="off" {assert!(report[0].get("JIT").is_none(),"negative control must omit JIT");}
        }
        db.batch_execute("SET jit=on").await.unwrap();
        let rows=spike_v2_read::execute(&mut db,&plan).await.unwrap();
        assert_eq!(rows.len(),1);assert_eq!(rows[0]["title"],"Welcome!");assert_eq!(rows[0]["post_count"],1);
    }).catch_unwind().await;
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await.unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn short_read_jit_setting_is_local_and_keeps_deadlines_and_long_reads() {
    let mut db = spike_v2_read::connect_owned_with_url(DB).await.unwrap();
    db.batch_execute("SET jit=on").await.unwrap();
    let mut plan = Plan {
        sql: "SELECT json_build_object('jit',current_setting('jit'))::text".into(),
        params: vec![],
        deadline_ms: 2000,
        output_type: json!({}),
        cost: 1,
        null_is_denied: false,
        deps: vec![],
    };
    assert_eq!(spike_v2_read::execute(&mut db, &plan).await.unwrap(), vec![json!({"jit":"off"})]);
    assert_eq!(db.query_one("SHOW jit", &[]).await.unwrap().get::<_, String>(0), "on", "a committed read must restore the connection setting");
    plan.deadline_ms = 2001;
    assert_eq!(spike_v2_read::execute(&mut db, &plan).await.unwrap(), vec![json!({"jit":"on"})], "longer reads keep the operator preference");
    plan.deadline_ms = 50;
    plan.sql = "SELECT json_build_object('delay',pg_sleep(0.15))::text".into();
    let rejected = spike_v2_read::execute(&mut db, &plan).await.unwrap_err();
    assert_eq!(rejected.code, "WIRE.DEADLINE_EXCEEDED");
    assert_eq!(db.query_one("SHOW jit", &[]).await.unwrap().get::<_, String>(0), "on", "a cancelled read must also restore the setting");
}
