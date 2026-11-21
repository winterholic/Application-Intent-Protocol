//! P0-A: bundle 단계 간 타입 검증 참조. create 연산의 `refs:{field:{created:i}}`와 apply 대상 `target:{created:i}`는
//! 같은 묶음 앞 create가 만든 행의 id를 서버가 풀어 넣는다(test-plan A-S1~S4, A-F1~F8, R-1, K-3, 종단 시나리오).
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{bundle, bundle_plan, Knobs};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::OnceCell;

const DB: &str = aip_test_support::test_database_url!();

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
  expose create { allow owner = actor; fields owner, stock }
}
resource Order {
  fields { id: Id; owner: Member; status: Text(1..12) }
  rows read when owner = actor
  transition confirm { from status = "open"; allow owner = actor; to status = "confirmed" }
  transition hide { from true; allow owner = actor; to status = "hidden" }
  transition mark { from true; allow owner = actor; to status = "marked" }
  transition sweep { from status = "open"; allow owner = actor; to status = "swept" }
  expose create { allow owner = actor; fields owner, status }
  expose apply confirm { target id; bulk maxRows 5 }
  expose apply hide { target id; bulk maxRows 5; internal }
  expose apply mark { target id; bulk maxRows 100 }
  expose apply sweep { target where; bulk maxRows 5 }
  expose read { select id, status; filter status.eq; budget { rows 10; depth 1; deadline 1s; cost 100 } }
}
resource OrderLine {
  fields { id: Id; owner: Member; order: Order; product: Product?; parent: Order?; qty: Int(1..100) }
  rows read when owner = actor
  expose create { allow owner = actor; fields owner, order, product, qty }
}
predicate hasItem(b: Box) = exists BoxItem where box = b
resource Box {
  fields { id: Id; owner: Member; label: Text(1..20) }
  rows read when owner = actor
  check hasItem when hasItem(this)
  expose create { allow owner = actor; fields owner, label }
}
resource BoxItem {
  fields { id: Id; owner: Member; box: Box; qty: Int(1..100) }
  rows read when owner = actor
  expose create { allow owner = actor; fields owner, box, qty }
}
resource Dossier {
  fields { id: Id; owner: Member; title: Text(1..20) }
  rows read when title = "shown"
  transition touch { from true; allow actor != null; to title = "touched" }
  expose create { allow true; fields owner, title }
  expose apply touch { target id; bulk maxRows 1 }
}
resource DossierNote {
  fields { id: Id; owner: Member; dossier: Dossier; body: Text(1..20) }
  rows read when true
  expose create { allow actor != null; fields owner, dossier, body }
}
resource Secret {
  fields { id: Id; owner: Member; label: Text(1..20) }
  rows read when owner = actor
}
"#;

tokio::task_local! {
    /// 시험마다 다른 actor(와 owner)를 쓴다. 같은 스키마·표를 병렬로 쓰는 시험이 서로의 행을 세지 않게 한다.
    static ACTOR: i64;
}

fn me() -> i64 {
    ACTOR.get()
}

static SETUP: OnceCell<Arc<Value>> = OnceCell::const_new();

async fn facts() -> Arc<Value> {
    SETUP
        .get_or_init(|| async {
            let facts = load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
            sqlgen::set_schema(&format!("aip_v3_expr_refs_{}", std::process::id()));
            let db = connect().await;
            for ddl in sqlgen::ddl(&facts).unwrap() {
                db.batch_execute(&ddl).await.unwrap_or_else(|e| panic!("DDL {ddl}: {e}"));
            }
            let s = sqlgen::schema();
            let members: Vec<String> = (1..=40).map(|id| format!("({id})")).collect();
            db.batch_execute(&format!("INSERT INTO {s}.member (id) VALUES {}", members.join(", "))).await.unwrap();
            Arc::new(facts)
        })
        .await
        .clone()
}

fn caller() -> Caller {
    Caller { actor_id: Some(me()), now: "2026-11-21T00:00:00Z".into() }
}

async fn db() -> tokio_postgres::Client {
    facts().await;
    connect_with_url(DB).await.unwrap()
}

async fn run(req: &Value) -> Result<spike_v3_write::BundleResult, (String, String)> {
    let facts = facts().await;
    let mut db = db().await;
    bundle(&mut db, &facts, req, &caller(), &Knobs::default()).await.map_err(|e| (e.code.to_string(), e.msg))
}

fn code<T>(result: Result<T, (String, String)>) -> String {
    result.map(|_| "OK".to_string()).unwrap_or_else(|e| e.0)
}

async fn text(db: &tokio_postgres::Client, sql: String) -> String {
    db.query_one(&sql, &[]).await.unwrap().get::<_, String>(0)
}

async fn count(db: &tokio_postgres::Client, table: &str) -> i64 {
    db.query_one(&format!("SELECT count(*) FROM {}.{table} WHERE owner_id = {}", sqlgen::schema(), me()), &[]).await.unwrap().get(0)
}

fn order() -> Value {
    json!({"create": "Order", "values": {"owner": me().to_string(), "status": "open"}})
}

fn line(refs: Value, qty: i64) -> Value {
    json!({"create": "OrderLine", "values": {"owner": me().to_string(), "qty": qty}, "refs": refs})
}

fn bundle_of(ops: Vec<Value>) -> Value {
    json!({ "atomic": ops })
}

/// 같은 정의로 인접한 PG 시험이 서로의 행을 세지 않도록 시험마다 owner가 같아도 표를 비우지 않고 증가분을 본다.
async fn counts(db: &tokio_postgres::Client) -> (i64, i64) {
    (count(db, "order").await, count(db, "order_line").await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_s1_a_created_parent_id_is_referenced_by_two_lines_in_one_transaction() {
    ACTOR.scope(11, async {
        let db = db().await;
        let before = counts(&db).await;
        let out = run(&bundle_of(vec![order(), line(json!({"order": {"created": 0}}), 2), line(json!({"order": {"created": 0}}), 3)])).await.unwrap();
        assert_eq!(out.created, 3);
        let resources: Vec<&str> = out.created_rows.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(resources, ["Order", "OrderLine", "OrderLine"], "created[] 순서는 create 연산 순서");
        let parent = out.created_rows[0].1.unwrap();
        let s = sqlgen::schema();
        let owners = text(&db, format!("SELECT string_agg(order_id::text, ',' ORDER BY id) FROM {s}.order_line WHERE order_id = {parent}")).await;
        assert_eq!(owners, format!("{parent},{parent}"));
        assert_eq!(counts(&db).await, (before.0 + 1, before.1 + 2));
    }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_s2_a_created_row_is_the_target_of_a_later_transition() {
    ACTOR.scope(12, async {
        let db = db().await;
        let out = run(&bundle_of(vec![order(), json!({"apply": "Order.confirm", "target": {"created": 0}})])).await.unwrap();
        let parent = out.created_rows[0].1.unwrap();
        assert_eq!(out.applied[0].changed.len(), 1);
        assert_eq!(text(&db, format!("SELECT status FROM {}.order WHERE id = {parent}", sqlgen::schema())).await, "confirmed");
    }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_s3_the_commit_time_check_sees_the_lines_created_in_the_same_bundle() {
    ACTOR.scope(13, async {
        let db = db().await;
        let before = count(&db, "box").await;
        let boxed = json!({"create": "Box", "values": {"owner": me().to_string(), "label": "b"}});
        let item = json!({"create": "BoxItem", "values": {"owner": me().to_string(), "qty": 1}, "refs": {"box": {"created": 0}}});
        assert_eq!(code(run(&bundle_of(vec![boxed.clone(), item])).await), "OK");
        // 줄 없는 묶음은 커밋 시점 check가 막고 부모도 남지 않는다.
        assert_eq!(code(run(&bundle_of(vec![boxed])).await), "WIRE.CHECK_FAILED");
        assert_eq!(count(&db, "box").await, before + 1);
    }).await
}

/// 종단: 주문 생성 → 생성한 주문 id로 줄 생성 → input 수량으로 재고 차감 → 주문 확정을 한 묶음으로(P0-A와 P0-B 함께).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn order_flow_create_lines_deduct_stock_by_input_and_confirm_in_one_bundle() {
    ACTOR.scope(14, async {
        let db = db().await;
        let s = sqlgen::schema();
        let pid = 3000 + me();
        db.batch_execute(&format!("INSERT INTO {s}.product (id, owner_id, stock) VALUES ({pid}, {}, 10)", me())).await.unwrap();
        let flow = |qty: i64| {
            bundle_of(vec![
                order(),
                json!({"create": "OrderLine", "values": {"owner": me().to_string(), "product": pid.to_string(), "qty": qty}, "refs": {"order": {"created": 0}}}),
                json!({"apply": "Product.take", "target": {"ids": [pid.to_string()]}, "input": {"qty": qty}}),
                json!({"apply": "Order.confirm", "target": {"created": 0}}),
            ])
        };
        let before = counts(&db).await;
        let out = run(&flow(4)).await.unwrap();
        let parent = out.created_rows[0].1.unwrap();
        assert_eq!(text(&db, format!("SELECT stock::text FROM {s}.product WHERE id = {pid}")).await, "6");
        assert_eq!(text(&db, format!("SELECT status FROM {s}.order WHERE id = {parent}")).await, "confirmed");
        assert_eq!(text(&db, format!("SELECT count(*)::text FROM {s}.order_line WHERE order_id = {parent} AND product_id = {pid} AND qty = 4")).await, "1");
        assert_eq!(counts(&db).await, (before.0 + 1, before.1 + 1));
        // R-1: 재고가 모자라 세 번째 연산이 거절되면 주문·줄도 남지 않고 재고는 그대로다.
        let refused = run(&flow(7)).await.unwrap_err();
        assert_eq!(refused.0, "WIRE.INVALID_STATE", "{}", refused.1);
        assert_eq!(text(&db, format!("SELECT stock::text FROM {s}.product WHERE id = {pid}")).await, "6");
        assert_eq!(counts(&db).await, (before.0 + 1, before.1 + 1));
    }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_f1_a_reference_must_have_the_type_of_the_field() {
    ACTOR.scope(15, async {
        let db = db().await;
        let before = counts(&db).await;
        // Order를 만든 연산을 Product 참조 필드에, Ref가 아닌 필드에.
        for field in ["product", "qty"] {
            let req = bundle_of(vec![order(), json!({"create": "OrderLine", "values": {"owner": me().to_string(), "qty": 1}, "refs": {field: {"created": 0}}})]);
            let req = if field == "qty" {
                bundle_of(vec![order(), json!({"create": "OrderLine", "values": {"owner": me().to_string()}, "refs": {"qty": {"created": 0}}})])
            } else {
                req
            };
            assert_eq!(code(run(&req).await), "WIRE.TYPE_MISMATCH", "{field}");
        }
        // Product를 만든 연산은 order 필드에 못 들어간다.
        let product = json!({"create": "Product", "values": {"owner": me().to_string(), "stock": 5}});
        let wrong = bundle_of(vec![product, line(json!({"order": {"created": 0}}), 1)]);
        assert_eq!(code(run(&wrong).await), "WIRE.TYPE_MISMATCH");
        assert_eq!(counts(&db).await, before);
    }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_f2_the_step_number_must_name_an_earlier_operation() {
    ACTOR.scope(16, async {
        let db = db().await;
        let before = counts(&db).await;
        // 연산 3개: [order, line(created: N), order]. 5는 없음, 1은 자기 자신, 2는 뒤 연산이다.
        for bad in [json!(5), json!(1), json!(2), json!(-1), json!("0"), json!(1.5), json!(null), json!(true)] {
            let req = bundle_of(vec![order(), line(json!({"order": {"created": bad.clone()}}), 1), order()]);
            assert_eq!(code(run(&req).await), "WIRE.BAD_VALUE", "{bad}");
        }
        // 객체가 아닌 값(`values`가 객체가 아닐 때와 같은 `WIRE.BAD_REQUEST`)과 모르는 키.
        for bad in [json!(0), json!("0"), json!([0])] {
            assert_eq!(code(run(&bundle_of(vec![order(), line(json!({"order": bad.clone()}), 1)])).await), "WIRE.BAD_REQUEST", "{bad}");
        }
        let extra = bundle_of(vec![order(), line(json!({"order": {"created": 0, "x": 1}}), 1)]);
        assert_eq!(code(run(&extra).await), "WIRE.UNKNOWN_KEY");
        // apply 대상도 같다.
        for bad in [json!(1), json!(7), json!(-1), json!("0")] {
            let req = bundle_of(vec![order(), json!({"apply": "Order.confirm", "target": {"created": bad.clone()}})]);
            assert_eq!(code(run(&req).await), "WIRE.BAD_VALUE", "target {bad}");
        }
        assert_eq!(counts(&db).await, before);
    }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_f3_only_the_id_of_an_earlier_create_can_be_referenced() {
    ACTOR.scope(17, async {
        let db = db().await;
        let before = counts(&db).await;
        let pid = 3000 + me();
        // apply 연산을 가리킴.
        let to_apply = bundle_of(vec![json!({"apply": "Product.take", "target": {"ids": [pid.to_string()]}, "input": {"qty": 1}}), line(json!({"order": {"created": 0}}), 1)]);
        assert_eq!(code(run(&to_apply).await), "WIRE.VALUE_SOURCE_NOT_ALLOWED");
        let target_apply = bundle_of(vec![
            json!({"apply": "Product.take", "target": {"ids": [pid.to_string()]}, "input": {"qty": 1}}),
            json!({"apply": "Order.confirm", "target": {"created": 0}}),
        ]);
        assert_eq!(code(run(&target_apply).await), "WIRE.VALUE_SOURCE_NOT_ALLOWED");
        // compose 연산은 묶음에 아직 없다. 알 수 없는 연산은 create로 취급되어 모르는 키로 거절된다.
        let compose = bundle_of(vec![json!({"compose": "Order", "targets": ["1"], "steps": []}), line(json!({"order": {"created": 0}}), 1)]);
        assert!(matches!(code(run(&compose).await).as_str(), "WIRE.VALUE_SOURCE_NOT_ALLOWED" | "WIRE.UNKNOWN_KEY"));
        assert_eq!(counts(&db).await, before);
    }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_f4_a_ref_field_must_be_in_the_create_field_list() {
    ACTOR.scope(18, async {
        // parent는 Ref<Order>?지만 `expose create fields`에 없다.
        let req = bundle_of(vec![order(), line(json!({"parent": {"created": 0}, "order": {"created": 0}}), 1)]);
        assert_eq!(code(run(&req).await), "WIRE.FIELD_NOT_EXPOSED");
    }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_f5_a_field_cannot_come_from_both_values_and_refs() {
    ACTOR.scope(19, async {
        let both = json!({"create": "OrderLine", "values": {"owner": me().to_string(), "qty": 1, "order": "1"}, "refs": {"order": {"created": 0}}});
        assert_eq!(code(run(&bundle_of(vec![order(), both])).await), "WIRE.BAD_VALUE");
    }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_f6_the_resource_must_be_exposed_and_the_transition_resource_must_match() {
    ACTOR.scope(20, async {
        // 공개되지 않은 resource의 create에 refs.
        let secret = bundle_of(vec![order(), json!({"create": "Secret", "values": {"owner": me().to_string(), "label": "x"}, "refs": {"owner": {"created": 0}}})]);
        assert_eq!(code(run(&secret).await), "WIRE.NOT_EXPOSED");
        // Product를 만든 행을 Order 전이의 대상으로.
        let mismatch = bundle_of(vec![json!({"create": "Product", "values": {"owner": me().to_string(), "stock": 5}}), json!({"apply": "Order.confirm", "target": {"created": 0}})]);
        assert_eq!(code(run(&mismatch).await), "WIRE.TYPE_MISMATCH");
        // 공개되지 않은 전이.
        let unexposed = bundle_of(vec![order(), json!({"apply": "Order.nothing", "target": {"created": 0}})]);
        assert_eq!(code(run(&unexposed).await), "WIRE.NOT_EXPOSED");
    }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_f7_a_created_row_the_caller_cannot_read_cannot_be_referenced_and_everything_rolls_back() {
    ACTOR.scope(21, async {
        let db = db().await;
        let before = (count(&db, "dossier").await, count(&db, "dossier_note").await);
        // 호출자(1)가 만든 Dossier는 `rows read when owner != actor`라 호출자에게 안 보인다.
        let dossier = json!({"create": "Dossier", "values": {"owner": me().to_string(), "title": "hidden"}});
        let note = json!({"create": "DossierNote", "values": {"owner": me().to_string(), "body": "b"}, "refs": {"dossier": {"created": 0}}});
        assert_eq!(code(run(&bundle_of(vec![dossier.clone(), note])).await), "WIRE.MISSING_TARGET");
        // 대상 참조도 같은 읽기 정책을 거친다.
        let touch = json!({"apply": "Dossier.touch", "target": {"created": 0}});
        assert_eq!(code(run(&bundle_of(vec![dossier.clone(), touch])).await), "WIRE.MISSING_TARGET");
        // 읽기 정책에 맞는 제목으로 만들면 호출자에게 보이므로 같은 모양이 통과한다.
        let visible = json!({"create": "Dossier", "values": {"owner": me().to_string(), "title": "shown"}});
        let note = json!({"create": "DossierNote", "values": {"owner": me().to_string(), "body": "b"}, "refs": {"dossier": {"created": 0}}});
        assert_eq!(code(run(&bundle_of(vec![visible, note])).await), "OK");
        assert_eq!((count(&db, "dossier").await, count(&db, "dossier_note").await), (before.0 + 1, before.1 + 1));
    }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_f8_target_created_cannot_open_an_internal_transition_or_an_unexposed_target_form() {
    ACTOR.scope(22, async {
        // internal 전이는 공개 경로에서 NOT_EXPOSED다.
        let internal = bundle_of(vec![order(), json!({"apply": "Order.hide", "target": {"created": 0}})]);
        assert_eq!(code(run(&internal).await), "WIRE.NOT_EXPOSED");
        // 검토 A-1: where 전용 전이는 id 대상 형식을 열지 않았으므로 created로 열 수 없다.
        let where_only = bundle_of(vec![order(), json!({"apply": "Order.sweep", "target": {"created": 0}})]);
        assert_eq!(code(run(&where_only).await), "WIRE.TARGET_NOT_ALLOWED");
        // created는 ids·where와 섞지 않는다.
        for mixed in [json!({"created": 0, "ids": ["1"]}), json!({"created": 0, "where": []})] {
            let req = bundle_of(vec![order(), json!({"apply": "Order.confirm", "target": mixed.clone()})]);
            assert_eq!(code(run(&req).await), "WIRE.BAD_VALUE", "{mixed}");
        }
        // 한 번에 하나의 apply로는 created를 쓸 수 없다(묶음 안에서만 앞 연산이 있다).
        let facts = facts().await;
        let mut db = db().await;
        let single = json!({"apply": "Order.confirm", "target": {"created": 0}});
        let rejected = spike_v3_write::apply(&mut db, &facts, &single, &caller(), &Knobs::default()).await.unwrap_err();
        assert_eq!(rejected.code, "WIRE.UNKNOWN_KEY");
    }).await
}

/// 규칙 ⑥: 모든 참조는 트랜잭션 전에 판정한다. 계획 함수는 DB를 쓰지 않고, 뒤 연산의 오류도 앞 연산이 실행되기 전에 나온다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refs_are_decided_before_the_transaction_so_no_earlier_operation_runs() {
    ACTOR.scope(23, async {
        let db = db().await;
        let facts = facts().await;
        let ops = vec![order(), line(json!({"order": {"created": 0}}), 1), line(json!({"order": {"created": 9}}), 1)];
        let rejected = bundle_plan(&facts, &ops, &Knobs::default()).err().expect("a missing step is refused by the plan alone");
        assert_eq!(rejected.code, "WIRE.BAD_VALUE");
        let before = counts(&db).await;
        assert_eq!(code(run(&bundle_of(ops)).await), "WIRE.BAD_VALUE");
        assert_eq!(counts(&db).await, before, "앞의 두 create는 실행되지 않았다");
        // 올바른 계획은 연산마다 하나씩 나온다.
        let ok = vec![order(), line(json!({"order": {"created": 0}}), 1)];
        assert_eq!(bundle_plan(&facts, &ok, &Knobs::default()).ok().map(|plan| plan.len()), Some(2));
    }).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refs_and_target_created_count_one_row_toward_the_row_limit() {
    ACTOR.scope(40, async {
        // `mark`이 한 번에 대상 100행까지 열려 있어 19개를 선언 상한으로 세면 1900 > 1000(BULK_LIMIT)이다. `target.created`는 행 하나이므로
        // 19행으로 세어 묶음이 실행된다(첫 `mark`가 바꾸고 나머지는 이미 목표 상태인 행이라 `unchanged`다, 검토 A-3).
        let mut ops = vec![order()];
        for _ in 0..19 {
            ops.push(json!({"apply": "Order.mark", "target": {"created": 0}}));
        }
        assert_eq!(ops.len(), 20);
        assert_eq!(code(run(&bundle_of(ops)).await), "OK");
    }).await
}

async fn race(requests: Vec<Value>) -> Vec<String> {
    let (facts, actor) = (facts().await, me());
    let gate = Arc::new(tokio::sync::Barrier::new(requests.len()));
    let mut tasks = vec![];
    for req in requests {
        let (facts, gate) = (facts.clone(), gate.clone());
        tasks.push(tokio::spawn(ACTOR.scope(actor, async move {
            let mut conn = connect_with_url(DB).await.unwrap();
            gate.wait().await;
            tokio::time::timeout(Duration::from_secs(20), bundle(&mut conn, &facts, &req, &caller(), &Knobs::default()))
                .await
                .expect("동시 묶음은 끝나야 한다")
                .map(|_| "OK".to_string())
                .unwrap_or_else(|e| e.code.to_string())
        })));
    }
    let mut out = vec![];
    for task in tasks {
        out.push(task.await.unwrap());
    }
    out
}

/// K-3: 같은 부모에 자식 묶음 둘과 부모 전이를 서로 다른 순서로 동시에 보낸다. 교착은 서버가 재시도하고,
/// 남는 결과는 성공 또는 결정적인 업무 거절(`INVALID_STATE`)뿐이며 줄은 성공한 묶음의 것만 남는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn k3_child_bundles_and_a_parent_transition_on_one_parent_settle_without_leaking_a_deadlock() {
    ACTOR.scope(25, async {
        let db = db().await;
        let s = sqlgen::schema();
        for round in 0..4 {
            let parent: i64 = 7000 + me() * 10 + round;
            db.batch_execute(&format!("INSERT INTO {s}.order (id, owner_id, status) VALUES ({parent}, {}, 'open')", me())).await.unwrap();
            let child = || json!({"create": "OrderLine", "values": {"owner": me().to_string(), "order": parent.to_string(), "qty": 1}});
            let confirm = || json!({"apply": "Order.confirm", "target": {"ids": [parent.to_string()]}});
            let outcomes = race(vec![
                bundle_of(vec![child(), confirm()]),
                bundle_of(vec![confirm(), child()]),
                bundle_of(vec![child(), child()]),
                bundle_of(vec![order(), line(json!({"order": {"created": 0}}), 1)]),
            ])
            .await;
            let tally: BTreeMap<&str, usize> = outcomes.iter().fold(BTreeMap::new(), |mut acc, o| {
                *acc.entry(o.as_str()).or_default() += 1;
                acc
            });
            assert!(tally.keys().all(|k| matches!(*k, "OK" | "WIRE.INVALID_STATE")), "round {round}: {outcomes:?}");
            // 서로 다른 순서의 두 confirm 묶음은 하나만 이긴다(둘 다 이기거나 둘 다 지면 안 된다).
            assert_eq!(outcomes[..2].iter().filter(|o| *o == "OK").count(), 1, "round {round}: {outcomes:?}");
            let lines: i64 = db.query_one(&format!("SELECT count(*) FROM {s}.order_line WHERE order_id = {parent}"), &[]).await.unwrap().get(0);
            let expected = 1 + if outcomes[2] == "OK" { 2 } else { 0 };
            assert_eq!(lines, expected, "round {round}: {outcomes:?}");
            assert_eq!(outcomes[3], "OK", "독립 부모를 만드는 묶음은 영향을 받지 않는다");
        }
    }).await
}
