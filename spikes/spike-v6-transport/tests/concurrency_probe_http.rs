//! Exploratory concurrency probes over real HTTP and PostgreSQL (plan-docs/hardening-1.0/concurrency.md). Every probe prints a
//! `PROBE` tally line per round. They are `#[ignore]`d because the schema name is a process-wide setting: run one per process,
//! `cargo test --features test-db --test concurrency_probe_http -- --ignored --nocapture --exact <name>`, and `PROBE_ROUNDS=<n>`
//! sets the repetitions (default 5). Assertions cover only invariants that must always hold (no partial commit, bounds, one winner).
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, id_wire::IdWire, sqlgen};
use spike_v5_sdk::contract_fingerprint_with_all_extensions;
use spike_v6_transport::{
    idempotency_ddl,
    server::{
        listen_with_prototype_deployment_launch, DeploymentFence, ReadExtensions, Server, ServerOptions, WorkerIsolation, WorkerLang,
        WorkerLaunchConfig,
    },
};
use std::{
    collections::BTreeMap,
    fs,
    net::SocketAddr,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

const DATABASE: &str = aip_test_support::test_database_url!();
const DIGEST: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Product {
  fields { id: Id; owner: Member; stock: Int(0..1000); big: Int; seen: Bool; price: Int(0..1000) }
  rows read when true
  transition take { input { qty: Int(1..100) }; from stock >= input.qty; allow actor != null; to stock = stock - input.qty }
  transition restock { input { qty: Int(1..1000) }; from true; allow actor != null; to stock = stock + input.qty }
  transition grow { input { n: Int(1..100) }; from true; allow actor != null; to big = big + input.n }
  transition mark { from true; allow actor != null; to seen = true }
  expose apply take { target id; bulk maxRows 5 }
  expose apply restock { target id; bulk maxRows 5 }
  expose apply grow { target id; bulk maxRows 5 }
  expose apply mark { target where; bulk maxRows 5 }
  expose compose { bulk maxRows 5; transitions mark }
  expose read { select id, stock; filter id.eq; budget { rows 10; depth 1; deadline 1s; cost 100 } }
}
resource Order {
  fields { id: Id; owner: Member }
  rows read when owner = actor
  expose create { allow owner = actor; fields owner }
}
resource OrderLine {
  fields { id: Id; owner: Member; order: Order; product: Product; qty: Int(1..100); unitPrice: Int(0..1000) }
  rows read when owner = actor
  expose create { allow owner = actor and unitPrice = product.price; fields owner, order, product, qty, unitPrice }
}
resource Room { fields { id: Id; name: Text } rows read when true }
predicate roomFree(r: Room) = not exists Booking where room = r and status = "CONFIRMED"
resource Booking {
  fields { id: Id; room: Room; status: Text }
  rows read when true
  transition confirm { allow roomFree(this.room); from status = "REQUESTED"; to status = "CONFIRMED" }
  expose apply confirm { target id; bulk maxRows 1 }
}
"#;

struct Env {
    db: tokio_postgres::Client,
    schema: String,
    server: Server,
    fingerprint: String,
    dir: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

async fn env(tag: &str, spec: &str, worker: Option<&str>, seed: &str) -> Env {
    let unique = format!("{}_{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    let dir = std::env::temp_dir().join(format!("aip-conc-probe-{tag}-{unique}"));
    fs::create_dir(&dir).unwrap();
    if let Some(worker) = worker {
        let root = dir.to_string_lossy().replace('\\', "\\\\").replace('\'', "\\'");
        fs::write(dir.join("write.mjs"), format!("import fs from 'node:fs';\nconst root='{root}';\n{worker}\n")).unwrap();
    }
    let mut facts: Value = load_str(spec, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let schema = format!("aip_conc_{tag}_{unique}");
    sqlgen::set_schema(&schema);
    let db = connect_with_url(DATABASE).await.unwrap();
    let ddl = sqlgen::create_ddl(&facts).unwrap();
    db.batch_execute(&ddl[0]).await.expect("new isolated schema");
    for statement in ddl.iter().skip(1) {
        db.batch_execute(statement).await.unwrap();
    }
    db.batch_execute(&idempotency_ddl()).await.unwrap();
    db.batch_execute(&format!(
        "CREATE TABLE {schema}.aip_migrate_meta(singleton boolean PRIMARY KEY, facts_digest text NOT NULL); INSERT INTO {schema}.aip_migrate_meta VALUES(true,'{DIGEST}')"
    ))
    .await
    .unwrap();
    db.batch_execute(&seed.replace("{S}", &schema)).await.unwrap();
    let catalog = spike_v2_read::external::snapshot(&db, &schema).await.unwrap();
    facts["externalStorage"] = json!({"mapping":{"schema":schema},"structureDigest":spike_v1_fixture::digest(&catalog)});
    let wire = IdWire::DecimalString;
    let fingerprint = contract_fingerprint_with_all_extensions(&facts, wire);
    let extensions = worker.map(|_| ReadExtensions { lang: WorkerLang::Node, dir: dir.clone() });
    let server = listen_with_prototype_deployment_launch(
        facts,
        wire,
        "127.0.0.1:0".parse().unwrap(),
        DATABASE.into(),
        extensions,
        ServerOptions { developer_diagnostics: true, ..ServerOptions::default() },
        worker.is_some(),
        WorkerLaunchConfig { isolation: WorkerIsolation::None, ..Default::default() },
        Some(DeploymentFence::new(schema.clone(), DIGEST.to_string()).unwrap()),
        true,
    )
    .await
    .unwrap();
    Env { db, schema, server, fingerprint, dir }
}

async fn http(address: SocketAddr, path: &str, token: &str, fingerprint: &str, body: &Value, drop_response: bool) -> Value {
    let body = body.to_string();
    let mut socket = TcpStream::connect(address).await.unwrap();
    let fault = if drop_response { "x-spike-drop-response: 1\r\n" } else { "" };
    socket
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nhost: {address}\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fingerprint}\r\n{fault}content-length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = String::new();
    let _ = timeout(Duration::from_secs(15), socket.read_to_string(&mut response)).await.expect("HTTP deadline");
    match response.split_once("\r\n\r\n") {
        Some((_, body)) if !body.is_empty() => serde_json::from_str(body).unwrap_or_else(|_| json!({"unparsed": body})),
        _ => json!({"dropped": true}),
    }
}

impl Env {
    fn token(&self, actor: i64) -> String {
        self.server.keys.issue(actor, 600)
    }
    async fn post(&self, actor: i64, body: &Value) -> Value {
        http(self.server.address, "/apply", &self.token(actor), &self.fingerprint, body, false).await
    }
    async fn post_at(&self, path: &str, actor: i64, body: &Value) -> Value {
        http(self.server.address, path, &self.token(actor), &self.fingerprint, body, false).await
    }
    /// Fire all bodies at once; returns (sorted outcome tally, elapsed, raw results).
    async fn burst(&self, calls: Vec<(i64, Value)>) -> (BTreeMap<String, usize>, Duration, Vec<Value>) {
        let started = Instant::now();
        let handles: Vec<_> = calls
            .into_iter()
            .map(|(actor, body)| {
                let (address, token, fingerprint) = (self.server.address, self.token(actor), self.fingerprint.clone());
                tokio::spawn(async move { http(address, "/apply", &token, &fingerprint, &body, false).await })
            })
            .collect();
        let mut results = vec![];
        for handle in handles {
            results.push(handle.await.unwrap());
        }
        let elapsed = started.elapsed();
        let mut tally = BTreeMap::new();
        for result in &results {
            let name = outcome(result);
            *tally.entry(name).or_insert(0) += 1;
        }
        (tally, elapsed, results)
    }
    async fn num(&self, sql: &str) -> i64 {
        self.db.query_one(&sql.replace("{S}", &self.schema), &[]).await.unwrap().get(0)
    }
}

fn outcome(result: &Value) -> String {
    if result["ok"] == true {
        if result["replayed"] == true {
            "OK(replayed)".into()
        } else {
            "OK".into()
        }
    } else if let Some(code) = result["code"].as_str() {
        code.into()
    } else {
        format!("?{result}")
    }
}

fn probe(name: &str, round: usize, tally: &BTreeMap<String, usize>, elapsed: Duration, extra: &str) {
    println!("PROBE {name} round={round} elapsed={}ms tally={tally:?} {extra}", elapsed.as_millis());
}

fn rounds() -> usize {
    std::env::var("PROBE_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(5)
}

fn take(id: i64, qty: i64) -> Value {
    json!({"apply": "Product.take", "target": {"ids": [id.to_string()]}, "input": {"qty": qty}})
}

fn seed_basic(products: i64) -> String {
    let members: Vec<String> = (1..=12).map(|id| format!("({id})")).collect();
    let rows: Vec<String> = (1..=products).map(|id| format!("({id},1,1000,0,false,10)")).collect();
    format!(
        "INSERT INTO {{S}}.member(id) VALUES {}; INSERT INTO {{S}}.product(id,owner_id,stock,big,seen,price) VALUES {}",
        members.join(","),
        rows.join(",")
    )
}

fn order_line(actor: i64, product: i64, first_ref: usize) -> Vec<Value> {
    vec![
        json!({"create": "Order", "values": {"owner": actor.to_string()}}),
        json!({"create": "OrderLine", "values": {"owner": actor.to_string(), "product": product.to_string(), "qty": 1, "unitPrice": 10}, "refs": {"order": {"created": first_ref}}}),
    ]
}

// ---------------------------------------------------------------------------------------------------------------------
// A. bundle lock order
// ---------------------------------------------------------------------------------------------------------------------

/// A1: two bundles that take the same two products in opposite orders. E9 documents "operation order is lock order".
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn a1_opposite_order_two_product_bundles() {
    let e = env("a1", SPEC, None, &seed_basic(2)).await;
    let n: i64 = std::env::var("PROBE_N").ok().and_then(|v| v.parse().ok()).unwrap_or(6);
    for round in 0..rounds() {
        let calls: Vec<_> = (1..=n)
            .map(|i| {
                let ops = if i % 2 == 0 { vec![take(1, 1), take(2, 1)] } else { vec![take(2, 1), take(1, 1)] };
                (i, json!({"key": format!("a1-{round}-{i}"), "request": {"atomic": ops}}))
            })
            .collect();
        let (tally, elapsed, _) = e.burst(calls).await;
        let stock = e.num("SELECT stock FROM {S}.product WHERE id IN (1,2) ORDER BY id LIMIT 1").await;
        probe("a1-opposite-order", round, &tally, elapsed, &format!("stock1={stock}"));
    }
}

/// A2: line (FK + policy read of the product) first, then a compose that updates the same product row.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn a2_line_first_then_compose_on_same_product() {
    let e = env("a2", SPEC, None, &seed_basic(1)).await;
    for round in 0..rounds() {
        let calls: Vec<_> = (1..=6)
            .map(|i| {
                let mut ops = order_line(i, 1, 0);
                ops.push(json!({"compose": "Product", "targets": {"ids": ["1"]}, "steps": [{"transition": "mark"}]}));
                (i, json!({"key": format!("a2-{round}-{i}"), "request": {"atomic": ops}}))
            })
            .collect();
        let (tally, elapsed, results) = e.burst(calls).await;
        let sample = results.iter().find(|r| r["ok"] != true).map(|r| r.to_string()).unwrap_or_default();
        probe("a2-line-then-compose", round, &tally, elapsed, &sample);
    }
}

/// A3: line first, then a `target where` transition that updates the same product row.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn a3_line_first_then_where_target_apply() {
    let e = env("a3", SPEC, None, &seed_basic(1)).await;
    for round in 0..rounds() {
        let calls: Vec<_> = (1..=6)
            .map(|i| {
                let mut ops = order_line(i, 1, 0);
                ops.push(json!({"apply": "Product.mark", "target": {"where": [{"field": "id", "op": "eq", "value": "1"}]}}));
                (i, json!({"key": format!("a3-{round}-{i}"), "request": {"atomic": ops}}))
            })
            .collect();
        let (tally, elapsed, results) = e.burst(calls).await;
        let sample = results.iter().find(|r| r["ok"] != true).map(|r| r.to_string()).unwrap_or_default();
        probe("a3-line-then-where", round, &tally, elapsed, &sample);
    }
}

/// A4: line first, then an apply on the same product through `target.ids` but the transition is `grow` (non-key column).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn a4_line_first_then_ids_apply_control() {
    let e = env("a4", SPEC, None, &seed_basic(1)).await;
    for round in 0..rounds() {
        let calls: Vec<_> = (1..=6)
            .map(|i| {
                let mut ops = order_line(i, 1, 0);
                ops.push(json!({"apply": "Product.grow", "target": {"ids": ["1"]}, "input": {"n": 1}}));
                (i, json!({"key": format!("a4-{round}-{i}"), "request": {"atomic": ops}}))
            })
            .collect();
        let (tally, elapsed, _) = e.burst(calls).await;
        let big = e.num("SELECT big FROM {S}.product WHERE id=1").await;
        probe("a4-line-then-ids", round, &tally, elapsed, &format!("big={big}"));
    }
}

/// A5: partial failure inside a bundle (line/order created, then a take that fails) leaves nothing behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn a5_failure_after_creates_leaves_no_rows_and_no_idempotency_record() {
    let e = env("a5", SPEC, None, &seed_basic(1)).await;
    for round in 0..rounds() {
        let calls: Vec<_> = (1..=8)
            .map(|i| {
                let mut ops = order_line(i, 1, 0);
                ops.push(take(1, 100));
                (i, json!({"key": format!("a5-{round}-{i}"), "request": {"atomic": ops}}))
            })
            .collect();
        let (tally, elapsed, _) = e.burst(calls).await;
        let stock = e.num("SELECT stock FROM {S}.product WHERE id=1").await;
        let orders = e.num("SELECT count(*) FROM {S}.\"order\"").await;
        let lines = e.num("SELECT count(*) FROM {S}.order_line").await;
        let idem = e.num("SELECT count(*) FROM {S}.aip_idem").await;
        probe("a5-take-drain", round, &tally, elapsed, &format!("stock={stock} orders={orders} lines={lines} idem={idem}"));
        let ok = tally.get("OK").copied().unwrap_or(0) as i64;
        assert_eq!((orders, lines, idem), (ok, ok, ok), "only committed bundles leave rows");
        e.db.batch_execute(&format!("UPDATE {}.product SET stock=1000", e.schema)).await.unwrap();
        e.db.batch_execute(&format!("DELETE FROM {}.order_line; DELETE FROM {}.\"order\"; DELETE FROM {}.aip_idem", e.schema, e.schema, e.schema))
            .await
            .unwrap();
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// I. idempotency
// ---------------------------------------------------------------------------------------------------------------------

/// I1: many concurrent duplicates of one key (READ COMMITTED bundle): exactly one executes.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn i1_same_key_concurrent_duplicates_read_committed() {
    let e = env("i1", SPEC, None, &seed_basic(1)).await;
    for round in 0..rounds() {
        let body = json!({"key": format!("i1-{round}"), "request": {"atomic": [take(1, 7)]}});
        let calls: Vec<_> = (0..12).map(|_| (1, body.clone())).collect();
        let before = e.num("SELECT stock FROM {S}.product WHERE id=1").await;
        let (tally, elapsed, _) = e.burst(calls).await;
        let after = e.num("SELECT stock FROM {S}.product WHERE id=1").await;
        probe("i1-dup-rc", round, &tally, elapsed, &format!("delta={}", before - after));
        assert_eq!(before - after, 7, "exactly one execution");
    }
}

/// I2: same, SERIALIZABLE transition (absence predicate).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn i2_same_key_concurrent_duplicates_serializable() {
    let seed = "INSERT INTO {S}.member(id) VALUES (1),(2),(3),(4),(5),(6); INSERT INTO {S}.room(id,name) VALUES (1,'r'),(2,'r'); INSERT INTO {S}.booking(id,room_id,status) VALUES (1,1,'REQUESTED'),(2,1,'REQUESTED'),(3,2,'REQUESTED'),(4,2,'REQUESTED')";
    let e = env("i2", SPEC, None, seed).await;
    for round in 0..rounds().min(2) {
        let (booking, other) = if round == 0 { (1, 3) } else { (3, 1) };
        let _ = other;
        let body = json!({"key": format!("i2-{round}"), "request": {"apply": "Booking.confirm", "target": {"ids": [booking.to_string()]}}});
        let calls: Vec<_> = (0..12).map(|_| (1, body.clone())).collect();
        let (tally, elapsed, _) = e.burst(calls).await;
        let confirmed = e.num("SELECT count(*) FROM {S}.booking WHERE status='CONFIRMED'").await;
        probe("i2-dup-ser", round, &tally, elapsed, &format!("confirmed={confirmed}"));
    }
}

/// I3: same key, two different requests racing. Exactly one wins, the other is an idempotency mismatch (never both run).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn i3_same_key_different_requests_race() {
    let e = env("i3", SPEC, None, &seed_basic(1)).await;
    for round in 0..rounds() {
        let key = format!("i3-{round}");
        let calls: Vec<_> = (0..10).map(|i| (1, json!({"key": key, "request": {"atomic": [take(1, if i % 2 == 0 { 3 } else { 5 })]}}))).collect();
        let before = e.num("SELECT stock FROM {S}.product WHERE id=1").await;
        let (tally, elapsed, _) = e.burst(calls).await;
        let after = e.num("SELECT stock FROM {S}.product WHERE id=1").await;
        probe("i3-key-mismatch-race", round, &tally, elapsed, &format!("delta={}", before - after));
        assert!(matches!(before - after, 3 | 5), "exactly one of the two requests ran, delta={}", before - after);
    }
}

/// I4: response lost after commit, then the same key is re-sent, also after a deployment-digest change.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn i4_response_loss_then_replay_and_fence() {
    let e = env("i4", SPEC, None, &seed_basic(1)).await;
    let token = e.token(1);
    let body = json!({"key": "i4", "request": {"atomic": [take(1, 9)]}});
    let lost = http(e.server.address, "/apply", &token, &e.fingerprint, &body, true).await;
    println!("PROBE i4 lost={lost}");
    let replay = e.post(1, &body).await;
    println!("PROBE i4 replay={replay}");
    let status = e.post_at("/status", 1, &json!({"key": "i4", "request": {"atomic": [take(1, 9)]}})).await;
    println!("PROBE i4 status={status}");
    let stock = e.num("SELECT stock FROM {S}.product WHERE id=1").await;
    assert_eq!(stock, 991);
    e.db.execute(&format!("UPDATE {}.aip_migrate_meta SET facts_digest=$1", e.schema), &[&"b".repeat(64)]).await.unwrap();
    let replay_changed = e.post(1, &body).await;
    let status_changed = e.post_at("/status", 1, &json!({"key": "i4", "request": {"atomic": [take(1, 9)]}})).await;
    println!("PROBE i4 after-fence-change replay={replay_changed} status={status_changed}");
    e.db.execute(&format!("DELETE FROM {}.aip_migrate_meta", e.schema), &[]).await.unwrap();
    let replay_missing = e.post(1, &body).await;
    let status_missing = e.post_at("/status", 1, &json!({"key": "i4", "request": {"atomic": [take(1, 9)]}})).await;
    println!("PROBE i4 marker-missing replay={replay_missing} status={status_missing}");
}

// ---------------------------------------------------------------------------------------------------------------------
// E. write extensions (ctx.data.create/apply)
// ---------------------------------------------------------------------------------------------------------------------

const EXT_SPEC: &str = r#"
resource Member { fields { id: Id } rows read when true }
actor Member
resource Product {
  fields { id: Id; owner: Member; stock: Int(0..1000) }
  rows read when true
  transition take { input { qty: Int(1..100) }; from stock >= input.qty; allow actor != null; to stock = stock - input.qty }
  expose apply take { target id; bulk maxRows 5 }
  extension write buy {
    input { id: Product.Id; qty: Int(1..100) }
    output { id: Product.Id; count: Int }
    access Product.take, create.Claim
    effect db
    deadline 2s
    implementation "write.buy"
  }
}
resource Claim {
  fields { id: Id; owner: Member; tag: Text(1..20) }
  rows read when true
  unique tag
  expose create { allow actor != null; fields owner, tag }
}
"#;

const EXT_WORKER: &str = "export async function buy(input,ctx){fs.appendFileSync(root+'/started',String(input.id)+'\\n');if(fs.existsSync(root+'/slow'))await new Promise(r=>setTimeout(r,Number(fs.readFileSync(root+'/slow','utf8'))||300));const r=await ctx.data.apply('Product','take',[input.id],undefined,undefined,undefined,{qty:input.qty});await ctx.data.create('Claim',{owner:'1',tag:'t'+input.qty});return {id:input.id,count:r.changed.length};}";

fn seed_ext() -> String {
    "INSERT INTO {S}.member(id) VALUES (1),(2),(3),(4),(5),(6); INSERT INTO {S}.product(id,owner_id,stock) VALUES (1,1,1000),(2,1,1000),(3,1,1000),(4,1,1000),(5,1,1000),(6,1,1000)".into()
}

fn ext_body(key: &str, qty: i64) -> Value {
    ext_body_on(key, qty, 1)
}

fn ext_body_on(key: &str, qty: i64, id: i64) -> Value {
    json!({"key": key, "request": {"extension": "Product.buy", "input": {"id": id.to_string(), "qty": qty}}})
}

/// E1: concurrent duplicates of one key on a slow extension. The unique claim row arbitrates; the worker must run once.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn e1_extension_same_key_concurrent_duplicates() {
    let e = env("e1", EXT_SPEC, Some(EXT_WORKER), &seed_ext()).await;
    for round in 0..rounds() {
        fs::write(e.dir.join("slow"), "400").unwrap();
        let _ = fs::remove_file(e.dir.join("started"));
        e.db.batch_execute(&format!("DELETE FROM {}.claim", e.schema)).await.ok();
        let body = ext_body(&format!("e1-{round}"), 1 + round as i64 % 3);
        let calls: Vec<_> = (0..4).map(|_| (1, body.clone())).collect();
        let before = e.num("SELECT stock FROM {S}.product WHERE id=1").await;
        let (tally, elapsed, results) = e.burst(calls).await;
        let after = e.num("SELECT stock FROM {S}.product WHERE id=1").await;
        let starts = fs::read_to_string(e.dir.join("started")).map(|s| s.lines().count()).unwrap_or(0);
        let sample = results.iter().find(|r| r["ok"] != true).map(|r| r.to_string()).unwrap_or_default();
        probe("e1-ext-dup", round, &tally, elapsed, &format!("delta={} starts={starts} {sample}", before - after));
    }
}

/// E2: different keys, same stock row, extension does apply+create; saturate and contend.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn e2_extension_contention_distinct_keys() {
    let e = env("e2", EXT_SPEC, Some(EXT_WORKER), &seed_ext()).await;
    e.db.batch_execute(&format!("UPDATE {}.product SET stock=4", e.schema)).await.unwrap();
    for round in 0..rounds() {
        e.db.batch_execute(&format!("UPDATE {}.product SET stock=4; DELETE FROM {}.claim", e.schema, e.schema)).await.unwrap();
        // 6 callers each take 2 of 4, but all create the same unique claim tag
        let calls: Vec<_> = (0..6).map(|i| (1 + i % 2, ext_body(&format!("e2-{round}-{i}"), 2))).collect();
        let (tally, elapsed, results) = e.burst(calls).await;
        let stock = e.num("SELECT stock FROM {S}.product WHERE id=1").await;
        let claims = e.num("SELECT count(*) FROM {S}.claim").await;
        let sample = results
            .iter()
            .filter(|r| r["ok"] != true)
            .map(|r| format!("{}/{}/{}", r["code"], r["cause"], r["detail"]))
            .collect::<Vec<_>>()
            .join(" | ");
        probe("e2-ext-contend", round, &tally, elapsed, &format!("stock={stock} claims={claims} {sample}"));
    }
}

/// E3: distinct product rows (no row-lock serialization), the same unique claim tag: the inserts race on the unique index.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn e3_extension_unique_create_race_on_distinct_rows() {
    let e = env("e3", EXT_SPEC, Some(EXT_WORKER), &seed_ext()).await;
    for round in 0..rounds() {
        e.db.batch_execute(&format!("DELETE FROM {}.claim; UPDATE {}.product SET stock=1000", e.schema, e.schema)).await.unwrap();
        fs::write(e.dir.join("slow"), "100").unwrap();
        let calls: Vec<_> = (0..4).map(|i| (1 + i, ext_body_on(&format!("e3-{round}-{i}"), 2, 1 + i))).collect();
        let (tally, elapsed, results) = e.burst(calls).await;
        let claims = e.num("SELECT count(*) FROM {S}.claim").await;
        let taken = e.num("SELECT count(*) FROM {S}.product WHERE stock < 1000").await;
        let sample = results
            .iter()
            .filter(|r| r["ok"] != true)
            .map(|r| format!("{}/{}/{}", r["code"], r["cause"], r["detail"]))
            .collect::<Vec<_>>()
            .join(" | ");
        probe("e3-ext-unique-race", round, &tally, elapsed, &format!("claims={claims} products_taken={taken} {sample}"));
        // a loser must roll back its stock take too
        assert_eq!(claims, taken, "stock taken without a claim (partial commit)");
        // the same key re-sent by a loser either replays nothing (not recorded) and reports the final truth
        let again = e.post(1 + 3, &ext_body_on(&format!("e3-{round}-3"), 2, 4)).await;
        println!("PROBE e3 resend-of-loser={}", again);
    }
}

const SER_SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Room { fields { id: Id; name: Text } rows read when true }
predicate roomFree(r: Room) = not exists Booking where room = r and status = "CONFIRMED"
resource Booking {
  fields { id: Id; room: Room; status: Text }
  rows read when true
  transition confirm { allow roomFree(this.room); from status = "REQUESTED"; to status = "CONFIRMED" }
  expose apply confirm { target id; bulk maxRows 1 }
  extension write confirmRoom {
    input { id: Booking.Id }
    output { id: Booking.Id; count: Int }
    access Booking.confirm
    effect db
    deadline 2s
    implementation "write.confirmRoom"
  }
}
"#;

const SER_WORKER: &str = "export async function confirmRoom(input,ctx){fs.appendFileSync(root+'/started',String(input.id)+'\\n');if(fs.existsSync(root+'/slow'))await new Promise(r=>setTimeout(r,Number(fs.readFileSync(root+'/slow','utf8'))||100));const r=await ctx.data.apply('Booking','confirm',[input.id]);return {id:input.id,count:r.changed.length};}";

fn ser_body(key: &str, booking: i64) -> Value {
    json!({"key": key, "request": {"extension": "Booking.confirmRoom", "input": {"id": booking.to_string()}}})
}

const SER_SEED: &str = "INSERT INTO {S}.member(id) VALUES (1),(2),(3),(4),(5),(6); INSERT INTO {S}.room(id,name) VALUES (1,'r'),(2,'r'); INSERT INTO {S}.booking(id,room_id,status) VALUES (1,1,'REQUESTED'),(2,1,'REQUESTED'),(3,1,'REQUESTED'),(4,1,'REQUESTED'),(5,2,'REQUESTED'),(6,2,'REQUESTED')";

/// E4: SERIALIZABLE write extension. Distinct keys race for one room; the same key is also sent in parallel.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn e4_serializable_extension_race_and_same_key_duplicates() {
    let e = env("e4", SER_SPEC, Some(SER_WORKER), SER_SEED).await;
    for round in 0..rounds() {
        e.db.batch_execute(&format!("UPDATE {}.booking SET status='REQUESTED'", e.schema)).await.unwrap();
        fs::write(e.dir.join("slow"), "80").unwrap();
        let calls: Vec<_> = (0..4).map(|i| (1 + i, ser_body(&format!("e4-{round}-{i}"), 1 + i))).collect();
        let (tally, elapsed, results) = e.burst(calls).await;
        let confirmed = e.num("SELECT count(*) FROM {S}.booking WHERE room_id=1 AND status='CONFIRMED'").await;
        let sample = results.iter().filter(|r| r["ok"] != true).map(|r| format!("{}/{}", r["code"], r["cause"])).collect::<Vec<_>>().join(" | ");
        probe("e4-ser-distinct", round, &tally, elapsed, &format!("confirmed={confirmed} {sample}"));
        assert!(confirmed <= 1, "two bookings of one room confirmed");
        // same key in parallel on the other room
        let _ = fs::remove_file(e.dir.join("started"));
        let body = ser_body(&format!("e4-dup-{round}"), 5);
        let dup: Vec<_> = (0..4).map(|_| (1, body.clone())).collect();
        let (tally, elapsed, results) = e.burst(dup).await;
        let starts = fs::read_to_string(e.dir.join("started")).map(|s| s.lines().count()).unwrap_or(0);
        let confirmed = e.num("SELECT count(*) FROM {S}.booking WHERE room_id=2 AND status='CONFIRMED'").await;
        let sample = results.iter().filter(|r| r["ok"] != true).map(|r| format!("{}/{}", r["code"], r["cause"])).collect::<Vec<_>>().join(" | ");
        probe("e4-ser-dup", round, &tally, elapsed, &format!("confirmed={confirmed} starts={starts} {sample}"));
        e.db.batch_execute(&format!("UPDATE {}.booking SET status='REQUESTED' WHERE room_id=2", e.schema)).await.unwrap();
    }
}

/// N1: concurrent increments against the declared upper bound and against i64 overflow; concurrent decrements below zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn n1_concurrent_increment_bounds_and_overflow() {
    let e = env("n1", SPEC, None, &seed_basic(1)).await;
    let restock = |q: i64| json!({"apply": "Product.restock", "target": {"ids": ["1"]}, "input": {"qty": q}});
    let grow = |n: i64| json!({"apply": "Product.grow", "target": {"ids": ["1"]}, "input": {"n": n}});
    for round in 0..rounds() {
        // stock 0..1000: start 0, six callers add 400 -> exactly two fit
        e.db.batch_execute(&format!("UPDATE {}.product SET stock=0", e.schema)).await.unwrap();
        let calls: Vec<_> = (1..=6).map(|i| (i, json!({"key": format!("n1r-{round}-{i}"), "request": {"atomic": [restock(400)]}}))).collect();
        let (tally, elapsed, results) = e.burst(calls).await;
        let stock = e.num("SELECT stock FROM {S}.product WHERE id=1").await;
        let sample = results.iter().find(|r| r["ok"] != true).map(|r| r["code"].to_string()).unwrap_or_default();
        probe("n1-restock-upper-bound", round, &tally, elapsed, &format!("stock={stock} {sample}"));
        assert_eq!(stock, 800);
        // plain (non-bundle) single-op form too
        e.db.batch_execute(&format!("UPDATE {}.product SET stock=0", e.schema)).await.unwrap();
        let calls: Vec<_> = (1..=6).map(|i| (i, json!({"key": format!("n1s-{round}-{i}"), "request": restock(400)}))).collect();
        let (tally, elapsed, _) = e.burst(calls).await;
        let stock = e.num("SELECT stock FROM {S}.product WHERE id=1").await;
        probe("n1-restock-single", round, &tally, elapsed, &format!("stock={stock}"));
        assert_eq!(stock, 800);
        // i64 overflow: big = max-250, six callers add 100 -> exactly two fit
        e.db.batch_execute(&format!("UPDATE {}.product SET big=9223372036854775557", e.schema)).await.unwrap();
        let calls: Vec<_> = (1..=6).map(|i| (i, json!({"key": format!("n1g-{round}-{i}"), "request": {"atomic": [grow(100)]}}))).collect();
        let (tally, elapsed, results) = e.burst(calls).await;
        let big = e.num("SELECT big FROM {S}.product WHERE id=1").await;
        let sample = results.iter().find(|r| r["ok"] != true).map(|r| r.to_string()).unwrap_or_default();
        probe("n1-grow-overflow", round, &tally, elapsed, &format!("big={big} {sample}"));
        assert!(big >= 9223372036854775557 && big <= i64::MAX);
        // decrement below zero: stock 5, ten callers take 1..: exactly five fit
        e.db.batch_execute(&format!("UPDATE {}.product SET stock=5", e.schema)).await.unwrap();
        let calls: Vec<_> = (1..=10).map(|i| (1 + i % 6, json!({"key": format!("n1t-{round}-{i}"), "request": {"atomic": [take(1, 1)]}}))).collect();
        let (tally, elapsed, _) = e.burst(calls).await;
        let stock = e.num("SELECT stock FROM {S}.product WHERE id=1").await;
        probe("n1-take-floor", round, &tally, elapsed, &format!("stock={stock}"));
        assert_eq!(stock, 0);
    }
}

/// F1: failing bundles (policy refusal on the line, a failing take after a compose) mixed with succeeding ones, concurrently.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn f1_failing_bundles_leave_nothing_behind_under_concurrency() {
    let e = env("f1", SPEC, None, &seed_basic(1)).await;
    e.db.batch_execute(&format!("UPDATE {}.product SET stock=3", e.schema)).await.unwrap();
    for round in 0..rounds() {
        e.db.batch_execute(&format!(
            "UPDATE {}.product SET stock=3, seen=false; DELETE FROM {}.order_line; DELETE FROM {}.\"order\"; DELETE FROM {}.aip_idem",
            e.schema, e.schema, e.schema, e.schema
        ))
        .await
        .unwrap();
        let mut calls = vec![];
        for i in 1..=12i64 {
            let mut ops = order_line(i % 6 + 1, 1, 0);
            match i % 4 {
                0 => ops[1]["values"]["unitPrice"] = json!(11), // policy refuses the line after the order was created
                1 => {
                    ops.push(json!({"compose": "Product", "targets": {"ids": ["1"]}, "steps": [{"transition": "mark"}]}));
                    ops.push(take(1, 2));
                } // stock 3 covers one
                2 => {
                    ops.push(json!({"compose": "Product", "targets": {"ids": ["1"]}, "steps": [{"transition": "mark"}]}));
                    ops.push(take(1, 100));
                } // always fails after the compose
                _ => {}
            }
            calls.push((i % 6 + 1, json!({"key": format!("f1-{round}-{i}"), "request": {"atomic": ops}})));
        }
        let (tally, elapsed, _) = e.burst(calls).await;
        let ok = tally.get("OK").copied().unwrap_or(0) as i64;
        let (orders, lines, idem) = (
            e.num("SELECT count(*) FROM {S}.\"order\"").await,
            e.num("SELECT count(*) FROM {S}.order_line").await,
            e.num("SELECT count(*) FROM {S}.aip_idem").await,
        );
        let (stock, seen) =
            (e.num("SELECT stock FROM {S}.product WHERE id=1").await, e.num("SELECT seen::int::bigint FROM {S}.product WHERE id=1").await);
        probe("f1-mixed-failures", round, &tally, elapsed, &format!("orders={orders} lines={lines} idem={idem} stock={stock} seen={seen}"));
        assert_eq!((orders, lines, idem), (ok, ok, ok), "partial commit");
        assert!(stock >= 0);
    }
}

/// E5: all four worker slots busy. In-flight duplicates, completed replay and `/status` under saturation.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn e5_saturated_slots_replay_status_and_inflight_duplicates() {
    let e = env("e5", EXT_SPEC, Some(EXT_WORKER), &seed_ext()).await;
    // one completed request first
    let done = e.post(1, &ext_body_on("done", 1, 1)).await;
    println!("PROBE e5 first={done}");
    fs::write(e.dir.join("slow"), "1200").unwrap();
    let _ = fs::remove_file(e.dir.join("started"));
    let mut inflight = vec![];
    for i in 0..4 {
        let (address, token, fingerprint) = (e.server.address, e.token(1 + i), e.fingerprint.clone());
        let body = ext_body_on(&format!("busy-{i}"), 1, 2 + i);
        inflight.push(tokio::spawn(async move { http(address, "/apply", &token, &fingerprint, &body, false).await }));
    }
    loop {
        if fs::read_to_string(e.dir.join("started")).map(|s| s.lines().count() >= 4).unwrap_or(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // (a) completed replay, (b) status of completed, (c) status of in-flight, (d) in-flight duplicate, (e) fresh key
    let replay = e.post(1, &ext_body_on("done", 1, 1)).await;
    let status_done = e.post_at("/status", 1, &ext_body_on("done", 1, 1)).await;
    let status_inflight = e.post_at("/status", 1, &ext_body_on("busy-0", 1, 2)).await;
    let dup_inflight = e.post(1, &ext_body_on("busy-0", 1, 2)).await;
    let fresh = e.post(1, &ext_body_on("fresh", 1, 6)).await;
    println!(
        "PROBE e5 saturated: replay={} status_done={} status_inflight={} dup_inflight={} fresh={}",
        outcome(&replay),
        outcome(&status_done),
        outcome(&status_inflight),
        outcome(&dup_inflight),
        outcome(&fresh)
    );
    let mut settled = vec![];
    for handle in inflight {
        settled.push(outcome(&handle.await.unwrap()));
    }
    fs::remove_file(e.dir.join("slow")).unwrap();
    let after_dup = e.post(1, &ext_body_on("busy-0", 1, 2)).await;
    let starts = fs::read_to_string(e.dir.join("started")).map(|s| s.lines().count()).unwrap_or(0);
    let stock2 = e.num("SELECT stock FROM {S}.product WHERE id=2").await;
    println!("PROBE e5 settled={settled:?} after_dup={} starts={starts} stock2={stock2}", outcome(&after_dup));
}

/// I5: SERIALIZABLE plain /apply, distinct keys racing for one room. Losers must never be WIRE.INTERNAL.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "exploratory probe: run one at a time with --ignored --exact (sqlgen::set_schema is once per process)"]
async fn i5_serializable_apply_distinct_keys_never_internal() {
    let e = env("i5", SPEC, None, SER_SEED_PLAIN).await;
    let mut all: BTreeMap<String, usize> = BTreeMap::new();
    for round in 0..rounds() {
        e.db.batch_execute(&format!("UPDATE {}.booking SET status='REQUESTED'", e.schema)).await.unwrap();
        let calls: Vec<_> = (0..6)
            .map(|i| {
                (
                    1 + i,
                    json!({"key": format!("i5-{round}-{i}"), "request": {"apply": "Booking.confirm", "target": {"ids": [(1 + i % 4).to_string()]}}}),
                )
            })
            .collect();
        let (tally, _, _) = e.burst(calls).await;
        for (k, v) in tally {
            *all.entry(k).or_insert(0) += v;
        }
        let confirmed = e.num("SELECT count(*) FROM {S}.booking WHERE room_id=1 AND status='CONFIRMED'").await;
        assert!(confirmed <= 1);
    }
    println!("PROBE i5-ser-apply rounds={} total_tally={all:?}", rounds());
}

const SER_SEED_PLAIN: &str = "INSERT INTO {S}.member(id) VALUES (1),(2),(3),(4),(5),(6); INSERT INTO {S}.room(id,name) VALUES (1,'r'); INSERT INTO {S}.booking(id,room_id,status) VALUES (1,1,'REQUESTED'),(2,1,'REQUESTED'),(3,1,'REQUESTED'),(4,1,'REQUESTED')";
