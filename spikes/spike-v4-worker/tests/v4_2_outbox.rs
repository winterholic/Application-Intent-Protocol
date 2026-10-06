//! V4-2 외부 효과 전달(RK-08): outbox → 공급자. 커밋된 효과만, 재시도, 멱등 키, 동시 소비자, 격리.
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect, sqlgen};
use spike_v4_worker::outbox::{consume, prepare, MockProvider, Round};
use std::sync::{Arc, Mutex};

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v4_2_outbox_delivery() {
    sqlgen::set_schema("aip_v4_outbox");
    let s = sqlgen::schema();
    let f = load_str(A, Form::A).unwrap().execution;
    let mut db = connect().await;
    for st in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&st).await.unwrap();
    }
    prepare(&db, s).await;
    let ins = |n: i64, recipient: i64| {
        format!("INSERT INTO {s}.aip_outbox (topic, recipient_id, source, source_id) SELECT 'apply.approved', {recipient}, 'Apply', g FROM generate_series(1, {n}) g")
    };
    let mut fails: Vec<String> = vec![];
    let mut check = |name: &str, got: String, want: &str| {
        if got != want {
            fails.push(format!("{name}: 기대 {want}, 실제 {got}"));
        }
    };
    let reset = |p: &Arc<Mutex<MockProvider>>| *p.lock().unwrap() = MockProvider::default();
    let p = Arc::new(Mutex::new(MockProvider::default()));
    let clear = format!("TRUNCATE {s}.aip_outbox RESTART IDENTITY");

    // 1. rollback된 쓰기는 outbox도 남기지 않는다
    db.batch_execute(&format!("BEGIN; {}; ROLLBACK", ins(1, 5))).await.unwrap();
    check("rollback 뒤 전달", format!("{:?}", consume(&mut db, s, &p, 10, false).await), &format!("{:?}", Round::default()));

    // 2. 정상 전달
    db.batch_execute(&ins(3, 5)).await.unwrap();
    let r = consume(&mut db, s, &p, 10, false).await;
    check("정상 전달", format!("{} effects={}", r.delivered, p.lock().unwrap().effects.len()), "3 effects=3");

    // 3. 일시 실패 뒤 재시도
    db.batch_execute(&clear).await.unwrap();
    reset(&p);
    p.lock().unwrap().fail_first = 2;
    db.batch_execute(&ins(3, 6)).await.unwrap();
    let r1 = consume(&mut db, s, &p, 10, false).await;
    let r2 = consume(&mut db, s, &p, 10, false).await;
    check("일시 실패 1회차", format!("{r1:?}"), &format!("{:?}", Round { delivered: 1, failed: 2, dead: 0 }));
    check(
        "일시 실패 2회차",
        format!("{r2:?} effects={}", p.lock().unwrap().effects.len()),
        &format!("{:?} effects=3", Round { delivered: 2, failed: 0, dead: 0 }),
    );

    // 4. 공급자 호출 뒤 전달 기록 전에 중단 → 재전달되지만 멱등 키로 효과는 한 번
    db.batch_execute(&clear).await.unwrap();
    reset(&p);
    db.batch_execute(&ins(1, 7)).await.unwrap();
    consume(&mut db, s, &p, 10, true).await;
    let r = consume(&mut db, s, &p, 10, false).await;
    {
        let pv = p.lock().unwrap();
        check(
            "중단 뒤 재전달",
            format!("delivered={} calls={} effects={}", r.delivered, pv.calls, pv.effects.len()),
            "delivered=1 calls=2 effects=1",
        );
    }

    // 5. 동시 소비자 두 개: 같은 행을 두 번 보내지 않는다
    db.batch_execute(&clear).await.unwrap();
    reset(&p);
    db.batch_execute(&ins(40, 8)).await.unwrap();
    let (pa, pb) = (p.clone(), p.clone());
    let a = tokio::spawn(async move {
        let mut c = connect().await;
        let mut n = 0;
        for _ in 0..10 {
            n += consume(&mut c, "aip_v4_outbox", &pa, 5, false).await.delivered;
        }
        n
    });
    let b = tokio::spawn(async move {
        let mut c = connect().await;
        let mut n = 0;
        for _ in 0..10 {
            n += consume(&mut c, "aip_v4_outbox", &pb, 5, false).await.delivered;
        }
        n
    });
    let (na, nb) = (a.await.unwrap(), b.await.unwrap());
    {
        let pv = p.lock().unwrap();
        check("동시 소비", format!("total={} calls={} effects={}", na + nb, pv.calls, pv.effects.len()), "total=40 calls=40 effects=40");
    }

    // 6. 영구 실패 행은 최대 시도 뒤 격리, 다른 행은 전달
    db.batch_execute(&clear).await.unwrap();
    reset(&p);
    p.lock().unwrap().always_fail_recipient = Some(99);
    db.batch_execute(&format!("{}; {}", ins(1, 99), ins(2, 9))).await.unwrap();
    let mut total = Round::default();
    for _ in 0..4 {
        let r = consume(&mut db, s, &p, 10, false).await;
        total.delivered += r.delivered;
        total.failed += r.failed;
        total.dead += r.dead;
    }
    check("영구 실패 격리", format!("{total:?}"), &format!("{:?}", Round { delivered: 2, failed: 2, dead: 1 }));

    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {s} CASCADE")).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}
