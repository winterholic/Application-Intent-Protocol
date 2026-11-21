//! P0-B in a PostgreSQL write extension: `ctx.data.apply(resource, transition, ids, key, values, expect, input)` forwards `input`
//! to the server (Node `worker.mjs` and Python `worker.py`). The transition subtracts the amount from the locked row.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, id_wire::IdWire, plan::Caller, sqlgen};
use spike_v4_worker::{write::prepare_write_in_with_wire, Isolation, Lang, Worker, WorkerLimits};
use std::{fs, path::PathBuf};

const DB: &str = aip_test_support::test_database_url!();
const NODE: &str = include_str!("fixtures/expr-ext-apply-input/ex.mjs");
const PYTHON: &str = include_str!("fixtures/expr-ext-apply-input/ex.py");

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Product {
  fields { id: Id; owner: Member; stock: Int(0..1000) }
  rows read when true
  transition take {
    input { qty: Int(1..100) }
    from stock >= input.qty
    allow actor != null
    to stock = stock - input.qty
  }
  expose apply take { target id; bulk maxRows 5 }
  extension write takeStock { input { id: Product.Id; qty: Int(1..100) } output { ok: Bool } access Product.take effect db deadline 5s implementation "ex.takeStock" }
  extension write takeWithoutInput { input { id: Product.Id; qty: Int(1..100) } output { ok: Bool } access Product.take effect db deadline 5s implementation "ex.takeWithoutInput" }
}
"#;

struct OwnedDir(PathBuf);
impl Drop for OwnedDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn caller() -> Caller {
    Caller { actor_id: Some(1), now: "2026-11-21T00:00:00Z".into() }
}

async fn stock(db: &tokio_postgres::Client, schema: &str) -> String {
    db.query_one(&format!("SELECT stock::text FROM {schema}.product WHERE id = 7"), &[]).await.unwrap().get::<_, String>(0)
}

async fn run(
    db: &mut tokio_postgres::Client,
    worker: &mut Worker,
    facts: &Value,
    extension: &str,
    input: Value,
) -> Result<Value, spike_v2_read::plan::Reject> {
    let tx = db.transaction().await.unwrap();
    let result = prepare_write_in_with_wire(&tx, worker, facts, &format!("Product.{extension}"), &input, &caller(), IdWire::DecimalString).await;
    match result {
        Ok(prepared) => {
            tx.commit().await.unwrap();
            Ok(prepared.output)
        }
        Err(error) => {
            tx.rollback().await.unwrap();
            Err(error)
        }
    }
}

#[tokio::test]
async fn ctx_data_apply_forwards_input_from_node_and_python_extensions() {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let dir = OwnedDir(std::env::temp_dir().join(format!("aip-expr-ext-apply-input-{unique}")));
    fs::create_dir(&dir.0).unwrap();
    fs::write(dir.0.join("ex.mjs"), NODE).unwrap();
    fs::write(dir.0.join("ex.py"), PYTHON).unwrap();
    let facts: Value = load_str(SPEC, Form::A).unwrap_or_else(|errors| panic!("{errors:?}")).execution;
    let schema = format!("aip_expr_ext_apply_input_{unique}");
    sqlgen::set_schema(&schema);
    let mut db = connect_with_url(DB).await.expect("local PostgreSQL");
    for statement in sqlgen::create_ddl(&facts).unwrap() {
        db.batch_execute(&statement).await.unwrap();
    }
    db.batch_execute(&format!("INSERT INTO {schema}.member(id) VALUES (1); INSERT INTO {schema}.product(id, owner_id, stock) VALUES (7, 1, 50)"))
        .await
        .unwrap();
    let isolation = if cfg!(target_os = "macos") { Isolation::MacNetDeny } else { Isolation::None };
    for lang in [Lang::Node, Lang::Python] {
        let mut worker = Worker::try_start_with(lang, dir.0.to_str().unwrap(), isolation, WorkerLimits::default()).await.expect("official worker");
        let before: i64 = stock(&db, &schema).await.parse().unwrap();
        // The amount reaches the transition and the server computes on the locked row.
        let out = run(&mut db, &mut worker, &facts, "takeStock", json!({"id": "7", "qty": 8})).await.unwrap_or_else(|e| panic!("{lang:?}: {e:?}"));
        assert_eq!(out["ok"], true, "{lang:?}");
        assert_eq!(stock(&db, &schema).await, (before - 8).to_string(), "{lang:?}");
        // The amount failing the transition's `from` ends the call and commits nothing.
        let refused = run(&mut db, &mut worker, &facts, "takeStock", json!({"id": "7", "qty": 100})).await.expect_err("stock 42 cannot cover 100");
        assert_eq!(refused.code, "WIRE.EXTENSION_ERROR", "{lang:?}: {refused:?}");
        assert_eq!(stock(&db, &schema).await, (before - 8).to_string(), "{lang:?}");
        // The block is required: no `input` is refused by the server and the call does not commit.
        let missing = run(&mut db, &mut worker, &facts, "takeWithoutInput", json!({"id": "7", "qty": 1})).await.expect_err("input is declared");
        assert_eq!(missing.code, "WIRE.EXTENSION_ERROR", "{lang:?}: {missing:?}");
        assert_eq!(stock(&db, &schema).await, (before - 8).to_string(), "{lang:?}");
        worker.stop().await;
    }
    assert_eq!(stock(&db, &schema).await, "34");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
}
