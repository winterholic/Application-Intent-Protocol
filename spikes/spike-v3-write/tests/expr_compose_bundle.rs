//! P0-D: a bundle operation `{compose, targets, steps}` runs the server's compose (`compose_plan` before the transaction,
//! `compose_exec` inside it) next to applies and creates, in one transaction (test-plan D-S1, D-F1, D-F2, A-F3 for compose).
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, connect_with_url, id_wire::IdWire, plan::Caller, sqlgen};
use spike_v3_write::{bundle, bundle_needs_serializable, bundle_plan, Knobs, BUNDLE_MAX_ROWS};
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
}
resource Log {
  fields { id: Id; owner: Member; note: Text(1..20) }
  rows read when owner = actor
  expose create { allow owner = actor; fields owner, note }
}
resource Order {
  fields { id: Id; owner: Member; status: Text(1..12); paid: Int(0..1000) }
  rows read when owner = actor
  transition confirm { from status = "open"; allow owner = actor; to status = "confirmed" }
  transition ship { from status = "confirmed"; allow owner = actor; to status = "shipped" }
  transition prepay { input { n: Int(1..100) }; from true; allow owner = actor; to paid = paid + input.n }
  expose apply confirm { target id; bulk maxRows 5 }
  expose apply ship { target id; bulk maxRows 5 }
  expose apply prepay { target id; bulk maxRows 5 }
  expose compose { bulk maxRows 100; transitions confirm, ship; create Log from owner }
}
resource Note {
  fields { id: Id; owner: Member; order: Order; body: Text(1..20) }
  rows read when owner = actor
  expose create { allow owner = actor; fields owner, order, body }
}
predicate free(m: Member) = not exists Slot where owner = m and taken = true
resource Slot {
  fields { id: Id; owner: Member; taken: Bool }
  rows read when owner = actor
  expose create { allow free(this.owner); fields owner, taken }
}
resource Room {
  fields { id: Id; owner: Member; name: Text(1..20) }
  rows read when owner = actor
  transition touch { from true; allow owner = actor; to name = "touched" }
  expose compose { bulk maxRows 3; transitions touch; create Slot from owner }
}
"#;

tokio::task_local! {
    /// 시험마다 다른 actor. 같은 표를 병렬로 쓰는 시험이 서로의 행을 세지 않게 한다.
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
            sqlgen::set_schema(&format!("aip_v3_expr_compose_bundle_{}", std::process::id()));
            let db = connect().await;
            for ddl in sqlgen::ddl(&facts).unwrap() {
                db.batch_execute(&ddl).await.unwrap_or_else(|e| panic!("DDL {ddl}: {e}"));
            }
            let s = sqlgen::schema();
            let members: Vec<String> = (1..=40).map(|id| format!("({id})")).collect();
            // 격리 수준 확인용: INSERT가 어떤 격리 수준에서 일어났는지 표에 남긴다.
            db.batch_execute(&format!(
                "INSERT INTO {s}.member (id) VALUES {};
                 CREATE TABLE {s}.seen(tbl text NOT NULL, level text NOT NULL, actor bigint);
                 CREATE FUNCTION {s}.capture() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO {s}.seen VALUES(TG_TABLE_NAME, current_setting('transaction_isolation'), NEW.owner_id); RETURN NEW; END $$;
                 CREATE TRIGGER capture BEFORE INSERT ON {s}.slot FOR EACH ROW EXECUTE FUNCTION {s}.capture();
                 CREATE TRIGGER capture BEFORE INSERT ON {s}.log FOR EACH ROW EXECUTE FUNCTION {s}.capture()",
                members.join(", ")
            ))
            .await
            .unwrap();
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

async fn run_with(facts: &Value, req: &Value) -> Result<spike_v3_write::BundleResult, (String, String)> {
    let mut db = db().await;
    bundle(&mut db, facts, req, &caller(), &Knobs::default()).await.map_err(|e| (e.code.to_string(), e.msg))
}

async fn run(req: &Value) -> Result<spike_v3_write::BundleResult, (String, String)> {
    let facts = facts().await;
    run_with(&facts, req).await
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

fn bundle_of(ops: Vec<Value>) -> Value {
    json!({ "atomic": ops })
}

async fn insert_orders(db: &tokio_postgres::Client, ids: &[i64], status: &str) {
    for id in ids {
        db.batch_execute(&format!("INSERT INTO {}.order (id, owner_id, status, paid) VALUES ({id}, {}, '{status}', 0)", sqlgen::schema(), me())).await.unwrap();
    }
}

fn compose(ids: &[i64], steps: Vec<Value>) -> Value {
    json!({"compose": "Order", "targets": {"ids": ids.iter().map(i64::to_string).collect::<Vec<_>>()}, "steps": steps})
}

fn confirm_step() -> Value {
    json!({"transition": "confirm"})
}

fn log_step() -> Value {
    json!({"create": "Log", "values": {"owner": {"item": "owner"}, "note": {"const": "x"}}})
}

fn take(product: i64, qty: i64) -> Value {
    json!({"apply": "Product.take", "target": {"ids": [product.to_string()]}, "input": {"qty": qty}})
}

/// D-S1: apply(input) → compose(전이 + create 단계) → apply가 한 트랜잭션이고, 응답의 연산별 구간이 어긋난 항목 번호를 설명한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d_s1_apply_compose_apply_run_together_and_the_spans_name_each_operations_entries() {
    ACTOR.scope(11, async {
        let db = db().await;
        let s = sqlgen::schema();
        db.batch_execute(&format!("INSERT INTO {s}.product (id, owner_id, stock) VALUES (1101, 11, 10)")).await.unwrap();
        insert_orders(&db, &[1101, 1102, 1103], "open").await;
        let ship = json!({"apply": "Order.ship", "target": {"ids": ["1101"]}});
        let out = run(&bundle_of(vec![take(1101, 4), compose(&[1101, 1102, 1103], vec![confirm_step(), log_step()]), ship])).await.unwrap();
        // applied: take, compose의 confirm, ship / created: compose의 Log 3개.
        assert_eq!((out.applied.len(), out.created, out.created_rows.len()), (3, 3, 3));
        let spans: Vec<_> = out.ops.iter().map(|o| (o.kind, o.applied.clone(), o.created.clone())).collect();
        assert_eq!(spans, vec![("apply", 0..1, 0..0), ("compose", 1..2, 0..3), ("apply", 2..3, 3..3)]);
        assert_eq!(out.applied[1].changed.len(), 3, "compose의 confirm 단계는 대상 3개를 바꾼다");
        assert_eq!(text(&db, format!("SELECT stock::text FROM {s}.product WHERE id = 1101")).await, "6");
        assert_eq!(text(&db, format!("SELECT string_agg(status, ',' ORDER BY id) FROM {s}.order WHERE owner_id = 11")).await, "shipped,confirmed,confirmed");
        assert_eq!(count(&db, "log").await, 3);
    })
    .await
}

/// 대표 시나리오: input 전이(수량 차감)와 compose(여러 대상 상태 전이)를 한 묶음으로. 어느 단계든 실패하면 전체가 롤백된다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_input_transition_and_a_compose_commit_together_and_any_failure_rolls_back_all() {
    ACTOR.scope(12, async {
        let db = db().await;
        let s = sqlgen::schema();
        db.batch_execute(&format!("INSERT INTO {s}.product (id, owner_id, stock) VALUES (1201, 12, 10)")).await.unwrap();
        insert_orders(&db, &[1201, 1202], "open").await;
        insert_orders(&db, &[1203], "shipped").await;
        let state = || async {
            (
                text(&db, format!("SELECT stock::text FROM {s}.product WHERE id = 1201")).await,
                text(&db, format!("SELECT string_agg(status, ',' ORDER BY id) FROM {s}.order WHERE owner_id = 12")).await,
                count(&db, "log").await,
            )
        };
        // compose가 실패하는 경우: 세 번째 대상(shipped)은 confirm의 from을 만족하지 못한다. 앞의 재고 차감과 앞 두 대상의 전이도 취소된다.
        let failing = bundle_of(vec![take(1201, 3), compose(&[1201, 1202, 1203], vec![confirm_step(), log_step()])]);
        assert_eq!(code(run(&failing).await), "WIRE.INVALID_STATE");
        assert_eq!(state().await, ("10".into(), "open,open,shipped".into(), 0));
        // compose 뒤의 연산이 실패하는 경우: 재고가 모자란 차감(앞의 compose 결과도 취소).
        let late = bundle_of(vec![compose(&[1201, 1202], vec![confirm_step(), log_step()]), take(1201, 100)]);
        assert_eq!(code(run(&late).await), "WIRE.INVALID_STATE");
        assert_eq!(state().await, ("10".into(), "open,open,shipped".into(), 0));
        // 모두 맞으면 한 번에 커밋된다.
        let good = bundle_of(vec![take(1201, 3), compose(&[1201, 1202], vec![confirm_step(), log_step()])]);
        assert_eq!(code(run(&good).await), "OK");
        assert_eq!(state().await, ("7".into(), "confirmed,confirmed,shipped".into(), 2));
    })
    .await
}

/// D-2: compose 연산도 조합 계약의 격리 표지를 따른다. 표지만이 아니라 실제 트랜잭션이 SERIALIZABLE로 돈다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d2_a_compose_operation_runs_the_bundle_serializable_when_its_contract_needs_it() {
    ACTOR.scope(13, async {
        let db = db().await;
        let s = sqlgen::schema();
        let facts = facts().await;
        assert_eq!(facts["resources"]["Room"]["exposeCompose"]["isolation"], "serializable", "{}", facts["resources"]["Room"]["exposeCompose"]);
        assert!(facts["resources"]["Order"]["exposeCompose"]["isolation"].is_null());
        let room = json!({"compose": "Room", "targets": {"ids": ["1301"]}, "steps": [{"transition": "touch"}, {"create": "Slot", "values": {"owner": {"item": "owner"}, "taken": {"const": true}}}]});
        let plain = bundle_of(vec![compose(&[1301], vec![log_step()])]);
        assert!(bundle_needs_serializable(&facts, &bundle_of(vec![room.clone()])));
        assert!(!bundle_needs_serializable(&facts, &plain));
        db.batch_execute(&format!("INSERT INTO {s}.room (id, owner_id, name) VALUES (1301, 13, 'r')")).await.unwrap();
        insert_orders(&db, &[1301], "open").await;
        assert_eq!(code(run(&bundle_of(vec![room])).await), "OK");
        assert_eq!(code(run(&plain).await), "OK");
        let seen: Vec<(String, String)> = db
            .query(&format!("SELECT tbl, level FROM {s}.seen WHERE actor = 13 ORDER BY tbl"), &[])
            .await
            .unwrap()
            .iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect();
        assert_eq!(seen, vec![("log".to_string(), "read committed".to_string()), ("slot".to_string(), "serializable".to_string())]);
    })
    .await
}

/// D-3: compose 계획의 오류와 합산 행 수 상한은 DB에 닿기 전에 나온다. 앞 연산은 실행되지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn d3_compose_plans_and_the_row_total_are_decided_before_the_transaction() {
    ACTOR.scope(14, async {
        let db = db().await;
        let s = sqlgen::schema();
        let facts = facts().await;
        db.batch_execute(&format!("INSERT INTO {s}.product (id, owner_id, stock) VALUES (1401, 14, 10)")).await.unwrap();
        insert_orders(&db, &[1401], "open").await;
        let missing_step = vec![take(1401, 2), compose(&[1401], vec![json!({"transition": "nothing"})])];
        let refused = bundle_plan(&facts, &missing_step, &Knobs::default(), IdWire::Legacy).err().expect("the plan alone refuses an unknown step");
        assert_eq!(refused.code, "WIRE.STEP_NOT_ALLOWED");
        assert_eq!(code(run(&bundle_of(missing_step)).await), "WIRE.STEP_NOT_ALLOWED");
        // 대상이 공개되지 않은 resource인 compose, 타입이 맞지 않는 값도 같다.
        let unexposed = json!({"compose": "Product", "targets": {"ids": ["1401"]}, "steps": [{"transition": "take"}]});
        assert_eq!(code(run(&bundle_of(vec![take(1401, 2), unexposed])).await), "WIRE.NOT_EXPOSED");
        assert_eq!(text(&db, format!("SELECT stock::text FROM {s}.product WHERE id = 1401")).await, "10", "앞 연산(take)은 실행되지 않았다");
        // 합산 행 수: compose 하나는 대상 100 × 단계 10 = 1000으로 상한 이하이지만 둘이면 1000을 넘는다. 계획만으로 거절한다.
        let ids: Vec<i64> = (1..=100).collect();
        let big = |ids: &[i64]| compose(ids, (0..10).map(|_| confirm_step()).collect());
        assert!(bundle_plan(&facts, &[big(&ids)], &Knobs::default(), IdWire::Legacy).is_ok());
        let over = bundle_plan(&facts, &[big(&ids), big(&ids)], &Knobs::default(), IdWire::Legacy).err().expect("2000 rows");
        assert_eq!(over.code, "WIRE.BULK_LIMIT");
        assert!(BUNDLE_MAX_ROWS < 2000);
        // compose 하나에 다른 연산 하나만 더해도 합이 넘으면 막힌다(compose 1000 + create 1).
        let extra = bundle_plan(&facts, &[big(&ids), json!({"create": "Log", "values": {"owner": "14", "note": "n"}})], &Knobs::default(), IdWire::Legacy).err();
        assert_eq!(extra.map(|e| e.code), Some("WIRE.BULK_LIMIT"));
    })
    .await
}

/// D-F1·D-F2·A-F3: 중첩은 한 단계만, compose는 값 입력 슬롯을 열지 않고, 만든 행이 여럿인 compose는 refs의 출처가 될 수 없다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nesting_input_transitions_and_references_to_a_compose_are_refused() {
    ACTOR.scope(15, async {
        let db = db().await;
        let facts = facts().await;
        insert_orders(&db, &[1501], "open").await;
        let before = (count(&db, "order").await, count(&db, "log").await);
        // D-F1: compose 안에 compose, 묶음 안에 묶음.
        let nested_compose = bundle_of(vec![compose(&[1501], vec![json!({"compose": "Order", "targets": {"ids": ["1501"]}, "steps": [confirm_step()]})])]);
        assert_eq!(code(run(&nested_compose).await), "WIRE.UNKNOWN_KEY");
        let nested_atomic = bundle_of(vec![json!({"atomic": [compose(&[1501], vec![confirm_step()])]})]);
        assert_eq!(code(run(&nested_atomic).await), "WIRE.UNKNOWN_KEY");
        // compose 연산은 refs·values 같은 다른 형식의 키를 받지 않는다.
        let mut with_refs = compose(&[1501], vec![confirm_step()]);
        with_refs["refs"] = json!({"order": {"created": 0}});
        assert_eq!(code(run(&bundle_of(vec![with_refs])).await), "WIRE.UNKNOWN_KEY");
        // A-F3: compose 연산을 가리키는 참조(create의 refs, apply의 target.created).
        let note = json!({"create": "Note", "values": {"owner": me().to_string(), "body": "b"}, "refs": {"order": {"created": 0}}});
        assert_eq!(code(run(&bundle_of(vec![compose(&[1501], vec![confirm_step()]), note])).await), "WIRE.VALUE_SOURCE_NOT_ALLOWED");
        let by_target = json!({"apply": "Order.ship", "target": {"created": 0}});
        assert_eq!(code(run(&bundle_of(vec![compose(&[1501], vec![confirm_step()]), by_target])).await), "WIRE.VALUE_SOURCE_NOT_ALLOWED");
        // 입력·patch·expect가 있는 전이는 compose 단계로 받지 않는다. 정의 단계가 막지만 손으로 만든 facts가 열어도 실행이 거절한다.
        let mut opened = (*facts).clone();
        opened["resources"]["Order"]["exposeCompose"]["transitions"] = json!(["confirm", "ship", "prepay"]);
        let prepay = bundle_of(vec![compose(&[1501], vec![json!({"transition": "prepay"})])]);
        assert_eq!(code(run_with(&opened, &prepay).await), "WIRE.BAD_VALUE");
        assert_eq!((count(&db, "order").await, count(&db, "log").await), before);
        assert_eq!(text(&db, format!("SELECT status FROM {}.order WHERE id = 1501", sqlgen::schema())).await, "open");
    })
    .await
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

/// 잠금 순서: 연산 배열 순서가 곧 잠금 순서다. compose는 대상을 id순으로 한 번에 잠근다. 반대 순서의 두 묶음이 같은 두 행을 다투면
/// 교착은 서버가 재시도하고 남는 결과는 하나의 성공과 하나의 업무 거절(`INVALID_STATE`)뿐이다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn opposite_order_bundles_with_a_compose_settle_to_one_winner_without_leaking_a_deadlock() {
    ACTOR.scope(16, async {
        let db = db().await;
        for round in 0..4 {
            let (a, b) = (1600 + round * 2, 1601 + round * 2);
            insert_orders(&db, &[a, b], "open").await;
            let one = json!({"apply": "Order.confirm", "target": {"ids": [a.to_string()]}});
            let two = compose(&[b], vec![confirm_step()]);
            let two_again = compose(&[a], vec![confirm_step()]);
            let one_again = json!({"apply": "Order.confirm", "target": {"ids": [b.to_string()]}});
            let mut outcomes = race(vec![bundle_of(vec![one, two]), bundle_of(vec![two_again, one_again])]).await;
            outcomes.sort();
            assert_eq!(outcomes, ["OK", "WIRE.INVALID_STATE"], "round {round}");
            assert_eq!(text(&db, format!("SELECT count(*)::text FROM {}.order WHERE id IN ({a}, {b}) AND status = 'confirmed'", sqlgen::schema())).await, "2");
        }
    })
    .await
}

/// 기존 묶음의 결과는 그대로다: 연산별 구간은 있지만 compose가 없는 요청의 응답 모양을 바꾸는 쪽은 v6 전송층이 정한다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bundle_without_compose_still_reports_one_span_per_operation() {
    ACTOR.scope(17, async {
        let db = db().await;
        let s = sqlgen::schema();
        db.batch_execute(&format!("INSERT INTO {s}.product (id, owner_id, stock) VALUES (1701, 17, 10)")).await.unwrap();
        let out = run(&bundle_of(vec![take(1701, 1), json!({"create": "Log", "values": {"owner": "17", "note": "n"}})])).await.unwrap();
        let spans: Vec<_> = out.ops.iter().map(|o| (o.kind, o.applied.clone(), o.created.clone())).collect();
        assert_eq!(spans, vec![("apply", 0..1, 0..0), ("create", 1..1, 0..1)]);
        let _: BTreeMap<(), ()> = BTreeMap::new();
    })
    .await
}
