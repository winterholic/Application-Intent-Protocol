//! P0-B 제한 산술의 PostgreSQL 실행: `input` 바인딩, 잠근 행에서의 계산, 오버플로 롤백, 동시 차감(test-plan B-S1~S4, B-F9~F12, K-1, K-2).
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, connect_with_url, plan::Caller, sqlgen};
use spike_v3_write::{apply, bundle, Knobs};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::OnceCell;

const DB: &str = aip_test_support::test_database_url!();

const SPEC: &str = r#"
resource Member { fields { id: Id } }
actor Member
resource Wallet {
  fields { id: Id; owner: Member; credits: Int(0..100); balance: Decimal(12,2) }
  rows read when true
}
resource Product {
  fields {
    id: Id; owner: Member; stock: Int(0..1000); reserved: Int; big: Int; cash: Decimal(12,2); tight: Decimal(5,2)
  }
  rows read when true
  transition take {
    input { qty: Int(1..100) }
    from stock >= input.qty
    allow actor != null
    to stock = stock - input.qty
  }
  transition restock {
    input { qty: Int(1..100) }
    from true
    allow actor != null
    to stock = stock + input.qty
  }
  transition pay {
    input { amount: Decimal(12,2) }
    from cash >= input.amount
    allow actor != null
    to cash = cash - input.amount
  }
  transition topup {
    input { points: Int }
    from true
    allow actor != null
    to cash = cash + input.points
  }
  transition grow {
    input { n: Int }
    from true
    allow actor != null
    to big = big + input.n
  }
  transition fill {
    input { amount: Decimal(5,2) }
    from true
    allow actor != null
    to tight = tight + input.amount
  }
  transition reserve {
    from true
    allow actor != null
    to stock = stock - this.reserved
  }
  transition bump {
    from true
    allow actor != null
    to stock = stock + 1
  }
  transition purge {
    input { min: Int }
    from stock >= input.min
    allow actor != null
    delete
  }
  transition both {
    input { qty: Int(1..100) }
    from stock >= input.qty
    allow actor != null
    to stock = stock - input.qty
    update Wallet where owner = this.owner { credits = credits - input.qty }
  }
  expose read { select id, stock; filter stock.eq; budget { rows 10; depth 1; deadline 1s; cost 100 } }
  expose apply take { target id, where; bulk maxRows 5 }
  expose apply restock { target id; bulk maxRows 5 }
  expose apply pay { target id; bulk maxRows 5 }
  expose apply topup { target id; bulk maxRows 5 }
  expose apply grow { target id; bulk maxRows 5 }
  expose apply fill { target id; bulk maxRows 5 }
  expose apply reserve { target id; bulk maxRows 5 }
  expose apply bump { target id; bulk maxRows 5 }
  expose apply purge { target id; bulk maxRows 5 }
  expose apply both { target id; bulk maxRows 5 }
}
resource Purchase {
  fields { id: Id; owner: Member; cost: Int(0..100); total: Decimal(12,2); paid: Bool }
  rows read when true
  transition pay {
    from paid = false
    allow actor != null
    to paid = true
    update Wallet where owner = this.owner { credits = credits - this.cost; balance = balance - this.total }
  }
  expose apply pay { target id; bulk maxRows 5 }
}
"#;

static SETUP: OnceCell<Arc<Value>> = OnceCell::const_new();

async fn facts() -> Arc<Value> {
    SETUP
        .get_or_init(|| async {
            let facts = load_str(SPEC, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
            sqlgen::set_schema(&format!("aip_v3_expr_arith_{}", std::process::id()));
            let db = connect().await;
            for ddl in sqlgen::ddl(&facts).unwrap() {
                db.batch_execute(&ddl).await.unwrap_or_else(|e| panic!("DDL {ddl}: {e}"));
            }
            let s = sqlgen::schema();
            db.batch_execute(&format!("INSERT INTO {s}.member (id) VALUES (1), (2)")).await.unwrap();
            Arc::new(facts)
        })
        .await
        .clone()
}

fn caller() -> Caller {
    Caller { actor_id: Some(1), now: "2026-11-21T00:00:00Z".into() }
}

async fn db() -> tokio_postgres::Client {
    // 스키마 이름은 프로세스에서 한 번 정한다. 어느 시험이든 DB를 만지기 전에 설정을 끝낸다.
    facts().await;
    connect_with_url(DB).await.unwrap()
}

async fn text(db: &tokio_postgres::Client, sql: String) -> String {
    db.query_one(&sql, &[]).await.unwrap().get::<_, String>(0)
}

async fn insert_product(db: &tokio_postgres::Client, id: i64, stock: i64) {
    let s = sqlgen::schema();
    db.batch_execute(&format!(
        "INSERT INTO {s}.product (id, owner_id, stock, reserved, big, cash, tight) VALUES ({id}, 1, {stock}, 0, 0, 100.00, 0.00)"
    ))
    .await
    .unwrap();
}

async fn stock(db: &tokio_postgres::Client, id: i64) -> String {
    text(db, format!("SELECT stock::text FROM {}.product WHERE id = {id}", sqlgen::schema())).await
}

fn take(ids: &[i64], qty: Value) -> Value {
    json!({"apply": "Product.take", "target": {"ids": ids.iter().map(i64::to_string).collect::<Vec<_>>()}, "input": {"qty": qty}})
}

async fn run(req: &Value) -> Result<spike_v3_write::Applied, (String, String)> {
    let facts = facts().await;
    let mut db = db().await;
    apply(&mut db, &facts, req, &caller(), &Knobs::default()).await.map_err(|e| (e.code.to_string(), e.msg))
}

fn code(result: Result<spike_v3_write::Applied, (String, String)>) -> String {
    result.map(|_| "OK".to_string()).unwrap_or_else(|e| e.0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b_s1_s2_s4_int_decimal_and_int_term_on_decimal_are_exact() {
    let facts = facts().await;
    let (db, s) = (db().await, sqlgen::schema());
    insert_product(&db, 101, 10).await;
    // B-S1: stock 10 - 3 = 7.
    assert_eq!(run(&take(&[101], json!(3))).await.unwrap().changed.len(), 1);
    assert_eq!(stock(&db, 101).await, "7");
    // B-S2: Decimal(12,2) 100.00 - "12.50" = 87.50(부동소수점 오차 없음).
    let pay = json!({"apply": "Product.pay", "target": {"ids": ["101"]}, "input": {"amount": "12.50"}});
    run(&pay).await.unwrap();
    assert_eq!(text(&db, format!("SELECT cash::text FROM {s}.product WHERE id = 101")).await, "87.50");
    // 같은 값을 열 번 빼도 0.1 계열 오차가 쌓이지 않는다.
    let dime = json!({"apply": "Product.pay", "target": {"ids": ["101"]}, "input": {"amount": "0.10"}});
    for _ in 0..10 {
        run(&dime).await.unwrap();
    }
    assert_eq!(text(&db, format!("SELECT cash::text FROM {s}.product WHERE id = 101")).await, "86.50");
    // B-S4: Int 항을 Decimal 필드에.
    run(&json!({"apply": "Product.topup", "target": {"ids": ["101"]}, "input": {"points": 5}})).await.unwrap();
    assert_eq!(text(&db, format!("SELECT cash::text FROM {s}.product WHERE id = 101")).await, "91.50");
    // 항이 같은 행의 필드인 형태.
    db.batch_execute(&format!("UPDATE {s}.product SET reserved = 4 WHERE id = 101")).await.unwrap();
    run(&json!({"apply": "Product.reserve", "target": {"ids": ["101"]}})).await.unwrap();
    assert_eq!(stock(&db, 101).await, "3");
    // 기존 리터럴 증감은 그대로 동작한다.
    run(&json!({"apply": "Product.bump", "target": {"ids": ["101"]}})).await.unwrap();
    assert_eq!(stock(&db, 101).await, "4");
    assert_eq!(facts["resources"]["Product"]["transitions"]["bump"]["to"]["stock"], json!({"increment": 1}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b_s3_effect_update_arith_runs_in_the_transition_transaction() {
    let (db, s) = (db().await, sqlgen::schema());
    db.batch_execute(&format!(
        "INSERT INTO {s}.member (id) VALUES (3) ON CONFLICT DO NOTHING; \
         INSERT INTO {s}.wallet (id, owner_id, credits, balance) VALUES (301, 3, 50, 100.00); \
         INSERT INTO {s}.purchase (id, owner_id, cost, total, paid) VALUES (301, 3, 20, 12.50, false)"
    ))
    .await
    .unwrap();
    let facts = facts().await;
    let mut conn = connect_with_url(DB).await.unwrap();
    let caller = Caller { actor_id: Some(3), now: "2026-11-21T00:00:00Z".into() };
    let req = json!({"apply": "Purchase.pay", "target": {"ids": ["301"]}});
    apply(&mut conn, &facts, &req, &caller, &Knobs::default()).await.unwrap();
    let wallet = db.query_one(&format!("SELECT credits::text, balance::text FROM {s}.wallet WHERE id = 301"), &[]).await.unwrap();
    assert_eq!((wallet.get::<_, String>(0), wallet.get::<_, String>(1)), ("30".into(), "87.50".into()));
    assert_eq!(text(&db, format!("SELECT paid::text FROM {s}.purchase WHERE id = 301")).await, "true");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn effect_arith_may_use_a_validated_input_and_rolls_back_with_the_transition() {
    let (db, s) = (db().await, sqlgen::schema());
    db.batch_execute(&format!(
        "INSERT INTO {s}.member (id) VALUES (4) ON CONFLICT DO NOTHING; \
         INSERT INTO {s}.wallet (id, owner_id, credits, balance) VALUES (401, 4, 10, 0.00); \
         INSERT INTO {s}.product (id, owner_id, stock, reserved, big, cash, tight) VALUES (401, 4, 50, 0, 0, 0.00, 0.00)"
    ))
    .await
    .unwrap();
    let facts = facts().await;
    let mut conn = connect_with_url(DB).await.unwrap();
    let caller = Caller { actor_id: Some(4), now: "2026-11-21T00:00:00Z".into() };
    let both = |qty: i64| json!({"apply": "Product.both", "target": {"ids": ["401"]}, "input": {"qty": qty}});
    apply(&mut conn, &facts, &both(4), &caller, &Knobs::default()).await.unwrap();
    assert_eq!((stock(&db, 401).await.as_str(), text(&db, format!("SELECT credits::text FROM {s}.wallet WHERE id = 401")).await.as_str()), ("46", "6"));
    // 효과의 결과(6 - 7 < 0)가 범위를 벗어나면 전이도 함께 롤백된다.
    let rejected = apply(&mut conn, &facts, &both(7), &caller, &Knobs::default()).await.unwrap_err();
    assert_eq!(rejected.code, "WIRE.BAD_VALUE", "{}", rejected.msg);
    assert_eq!((stock(&db, 401).await.as_str(), text(&db, format!("SELECT credits::text FROM {s}.wallet WHERE id = 401")).await.as_str()), ("46", "6"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b_f9_input_shape_errors_are_refused_before_any_write() {
    let db = db().await;
    insert_product(&db, 201, 10).await;
    let cases = [
        (json!({"apply": "Product.take", "target": {"ids": ["201"]}}), "WIRE.BAD_VALUE"),
        (json!({"apply": "Product.take", "target": {"ids": ["201"]}, "input": {}}), "WIRE.BAD_VALUE"),
        (json!({"apply": "Product.take", "target": {"ids": ["201"]}, "input": {"qty": 1, "extra": 1}}), "WIRE.FIELD_NOT_EXPOSED"),
        (json!({"apply": "Product.take", "target": {"ids": ["201"]}, "input": {"other": 1}}), "WIRE.FIELD_NOT_EXPOSED"),
        (json!({"apply": "Product.take", "target": {"ids": ["201"]}, "input": [1]}), "WIRE.BAD_VALUE"),
        (json!({"apply": "Product.bump", "target": {"ids": ["201"]}, "input": {"qty": 1}}), "WIRE.UNKNOWN_KEY"),
        (json!({"apply": "Product.take", "target": {"ids": ["201"]}, "input": {"qty": 1}, "inputs": {}}), "WIRE.UNKNOWN_KEY"),
    ];
    for (req, want) in cases {
        assert_eq!(code(run(&req).await), want, "{req}");
    }
    assert_eq!(stock(&db, 201).await, "10");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b_f10_input_range_type_and_decimal_digits_are_checked_before_the_database() {
    let db = db().await;
    insert_product(&db, 202, 10).await;
    for bad in [json!(0), json!(101), json!(-1), json!("3"), json!(1.5), json!(null), json!(true), json!(9223372036854775808u64)] {
        assert_eq!(code(run(&take(&[202], bad.clone())).await), "WIRE.BAD_VALUE", "{bad}");
    }
    for amount in [json!("1.234"), json!("abc"), json!(1.25), json!(12), json!(null), json!("1e2"), json!("12345678901.00")] {
        let req = json!({"apply": "Product.pay", "target": {"ids": ["202"]}, "input": {"amount": amount}});
        assert_eq!(code(run(&req).await), "WIRE.BAD_VALUE", "{amount}");
    }
    assert_eq!(stock(&db, 202).await, "10");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b_f11_overflow_is_bad_value_and_rolls_back_every_target() {
    let (db, s) = (db().await, sqlgen::schema());
    insert_product(&db, 211, 990).await;
    insert_product(&db, 212, 5).await;
    // Int range(0..1000) 밖. 같은 요청의 다른 대상(212)도 함께 롤백된다.
    let restock = |ids: &[i64], qty: i64| {
        json!({"apply": "Product.restock", "target": {"ids": ids.iter().map(i64::to_string).collect::<Vec<_>>()}, "input": {"qty": qty}})
    };
    assert_eq!(code(run(&restock(&[212, 211], 20)).await), "WIRE.BAD_VALUE");
    assert_eq!((stock(&db, 211).await.as_str(), stock(&db, 212).await.as_str()), ("990", "5"));
    // 범위 안 경계값은 성공한다.
    assert_eq!(code(run(&restock(&[211], 10)).await), "OK");
    assert_eq!(stock(&db, 211).await, "1000");
    // bigint 초과(22003).
    db.batch_execute(&format!("UPDATE {s}.product SET big = 9223372036854775807 WHERE id = 211")).await.unwrap();
    let grow = json!({"apply": "Product.grow", "target": {"ids": ["211"]}, "input": {"n": 1}});
    assert_eq!(code(run(&grow).await), "WIRE.BAD_VALUE");
    assert_eq!(text(&db, format!("SELECT big::text FROM {s}.product WHERE id = 211")).await, "9223372036854775807");
    // numeric(5,2) 초과.
    db.batch_execute(&format!("UPDATE {s}.product SET tight = 999.99 WHERE id = 211")).await.unwrap();
    let fill = json!({"apply": "Product.fill", "target": {"ids": ["211"]}, "input": {"amount": "0.01"}});
    assert_eq!(code(run(&fill).await), "WIRE.BAD_VALUE");
    assert_eq!(text(&db, format!("SELECT tight::text FROM {s}.product WHERE id = 211")).await, "999.99");
    // 범위 아래로 내려가는 차감도 같다(재고 5에서 같은 행의 reserved 9를 빼면 0 아래).
    db.batch_execute(&format!("UPDATE {s}.product SET reserved = 9 WHERE id = 212")).await.unwrap();
    assert_eq!(code(run(&json!({"apply": "Product.reserve", "target": {"ids": ["212"]}})).await), "WIRE.BAD_VALUE");
    assert_eq!(stock(&db, 212).await, "5");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn b_f12_a_false_from_is_invalid_state_and_changes_nothing() {
    let db = db().await;
    insert_product(&db, 221, 2).await;
    insert_product(&db, 222, 50).await;
    // 한 대상만 from을 만족하지 못해도 전체가 거절된다.
    assert_eq!(code(run(&take(&[222, 221], json!(5))).await), "WIRE.INVALID_STATE");
    assert_eq!((stock(&db, 221).await.as_str(), stock(&db, 222).await.as_str()), ("2", "50"));
    // 경계: stock == qty는 통과해 0이 된다.
    assert_eq!(code(run(&take(&[221], json!(2))).await), "OK");
    assert_eq!(stock(&db, 221).await, "0");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn from_with_input_also_filters_where_targets_and_delete_transitions() {
    let (db, s) = (db().await, sqlgen::schema());
    insert_product(&db, 231, 7).await;
    insert_product(&db, 232, 3).await;
    // where 대상: from(stock >= qty)을 만족하는 행만 대상이 되고, stock.eq 필터로 하나를 고른다.
    let by_where = json!({"apply": "Product.take", "target": {"where": [{"field": "stock", "op": "eq", "value": 7}]}, "input": {"qty": 5}});
    assert_eq!(code(run(&by_where).await), "OK");
    assert_eq!((stock(&db, 231).await.as_str(), stock(&db, 232).await.as_str()), ("2", "3"));
    // delete 전이도 input을 from에 쓴다.
    let purge = |id: &str, min: i64| json!({"apply": "Product.purge", "target": {"ids": [id]}, "input": {"min": min}});
    assert_eq!(code(run(&purge("232", 4)).await), "WIRE.INVALID_STATE");
    assert_eq!(code(run(&purge("232", 3)).await), "OK");
    assert_eq!(text(&db, format!("SELECT count(*)::text FROM {s}.product WHERE id = 232")).await, "0");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bundle_applies_input_per_operation_and_rolls_back_as_a_whole() {
    let facts = facts().await;
    let (db, mut conn) = (db().await, db().await);
    insert_product(&db, 241, 10).await;
    insert_product(&db, 242, 1).await;
    let ok = json!({"atomic": [take(&[241], json!(4)), take(&[242], json!(1))]});
    bundle(&mut conn, &facts, &ok, &caller(), &Knobs::default()).await.unwrap();
    assert_eq!((stock(&db, 241).await.as_str(), stock(&db, 242).await.as_str()), ("6", "0"));
    // 둘째 연산이 거절되면 첫 연산도 되돌린다.
    let bad = json!({"atomic": [take(&[241], json!(4)), take(&[242], json!(1))]});
    let err = bundle(&mut conn, &facts, &bad, &caller(), &Knobs::default()).await.unwrap_err();
    assert_eq!(err.code, "WIRE.INVALID_STATE", "{}", err.msg);
    assert_eq!((stock(&db, 241).await.as_str(), stock(&db, 242).await.as_str()), ("6", "0"));
}

/// 한 요청씩 자기 연결로 동시에 출발시킨다.
async fn race(requests: Vec<Value>, actor: i64) -> Vec<String> {
    let facts = facts().await;
    let gate = Arc::new(tokio::sync::Barrier::new(requests.len()));
    let mut tasks = vec![];
    for req in requests {
        let (facts, gate) = (facts.clone(), gate.clone());
        tasks.push(tokio::spawn(async move {
            let mut conn = connect_with_url(DB).await.unwrap();
            gate.wait().await;
            let caller = Caller { actor_id: Some(actor), now: "2026-11-21T00:00:00Z".into() };
            tokio::time::timeout(Duration::from_secs(20), apply(&mut conn, &facts, &req, &caller, &Knobs::default()))
                .await
                .expect("동시 요청은 끝나야 한다")
                .map(|_| "OK".to_string())
                .unwrap_or_else(|e| e.code.to_string())
        }));
    }
    let mut out = vec![];
    for task in tasks {
        out.push(task.await.unwrap());
    }
    out
}

fn tally(outcomes: &[String]) -> BTreeMap<&str, usize> {
    let mut map = BTreeMap::new();
    for outcome in outcomes {
        *map.entry(outcome.as_str()).or_default() += 1;
    }
    map
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn k1_twenty_concurrent_takes_of_one_unit_commit_exactly_ten_and_never_go_negative() {
    let db = db().await;
    insert_product(&db, 251, 10).await;
    insert_product(&db, 252, 10).await;
    let outcomes = race((0..20).map(|_| take(&[251], json!(1))).collect(), 1).await;
    assert_eq!(tally(&outcomes), BTreeMap::from([("OK", 10), ("WIRE.INVALID_STATE", 10)]), "{outcomes:?}");
    assert_eq!(stock(&db, 251).await, "0");
    // 서로 다른 수량이 섞여도 합이 재고를 넘지 않는다: 10 = 3+3+3 이고 남은 1에는 3이 못 들어간다.
    let outcomes = race((0..6).map(|_| take(&[252], json!(3))).collect(), 1).await;
    assert_eq!(tally(&outcomes), BTreeMap::from([("OK", 3), ("WIRE.INVALID_STATE", 3)]), "{outcomes:?}");
    assert_eq!(stock(&db, 252).await, "1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn k2_two_orders_paying_from_one_wallet_commit_only_the_one_the_balance_covers() {
    let (db, s) = (db().await, sqlgen::schema());
    db.batch_execute(&format!(
        "INSERT INTO {s}.member (id) VALUES (5) ON CONFLICT DO NOTHING; \
         INSERT INTO {s}.wallet (id, owner_id, credits, balance) VALUES (501, 5, 50, 100.00); \
         INSERT INTO {s}.purchase (id, owner_id, cost, total, paid) VALUES (501, 5, 40, 10.00, false), (502, 5, 40, 10.00, false)"
    ))
    .await
    .unwrap();
    let pay = |id: i64| json!({"apply": "Purchase.pay", "target": {"ids": [id.to_string()]}});
    let outcomes = race(vec![pay(501), pay(502)], 5).await;
    assert_eq!(tally(&outcomes), BTreeMap::from([("OK", 1), ("WIRE.BAD_VALUE", 1)]), "{outcomes:?}");
    let wallet = db.query_one(&format!("SELECT credits::text, balance::text FROM {s}.wallet WHERE id = 501"), &[]).await.unwrap();
    assert_eq!((wallet.get::<_, String>(0), wallet.get::<_, String>(1)), ("10".into(), "90.00".into()));
    assert_eq!(text(&db, format!("SELECT count(*)::text FROM {s}.purchase WHERE owner_id = 5 AND paid")).await, "1");
}
