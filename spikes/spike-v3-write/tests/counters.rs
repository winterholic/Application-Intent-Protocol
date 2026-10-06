//! 카운터·재고 증감 전이. 값은 정의의 상수만큼만 바뀌고, 동시 요청이 서로를 덮어쓰지 않는다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::Caller;
use spike_v2_read::{connect, sqlgen};
use spike_v3_write::{apply, Knobs};

const DEF: &str = "
resource Member { fields { id: Id } }
actor Member
resource Product {
  fields { id: Id; stock: Int(0..1000); likes: Int }
  rows read when true
  transition sell {
    allow actor != null
    from stock > 0
    to stock = stock - 1
  }
  transition like {
    allow actor != null
    from true
    to likes = this.likes + 1
  }
  expose read { select id, stock, likes; budget { rows 10; depth 1; deadline 1s; cost 100 } }
  expose apply sell { target id; bulk maxRows 5 }
  transition drain {
    allow actor != null
    from true
    to stock = stock - 1
  }
  expose apply like { target id; bulk maxRows 5 }
  expose apply drain { target id; bulk maxRows 5 }
}
";

fn facts(src: &str) -> Result<Value, String> {
    load_str(src, Form::A).map(|l| l.execution).map_err(|e| format!("{e:?}"))
}
fn who(id: Option<i64>) -> Caller {
    Caller { actor_id: id, now: "2026-10-07T00:00:00Z".into() }
}

#[test]
fn only_same_field_plus_or_minus_a_constant_is_accepted() {
    for (bad, code) in [
        ("to stock = stock - 1", "to stock = likes - 1"),
        ("to stock = stock - 1", "to stock = stock - 0"),
        ("to stock = stock - 1", "to stock = stock - likes"),
        ("to stock = stock - 1", "to stock = 1 + stock"),
        ("from stock > 0", "from stock + 1 > 0"),
    ]
    .map(|(old, new)| (DEF.replacen(old, new, 1), "ARITH_NOT_ALLOWED"))
    {
        let got = facts(&bad).unwrap_err();
        assert!(got.contains(code), "{bad}\n=> {got}");
    }
    let unchanged = DEF.replacen("to likes = this.likes + 1", "to likes = this.likes + 1\n    repeat unchanged", 1);
    assert!(facts(&unchanged).unwrap_err().contains("UNSUPPORTED"));
    let nullable = DEF.replacen("likes: Int }", "likes: Int? }", 1);
    assert!(facts(&nullable).unwrap_err().contains("TYPE_MISMATCH"));
    let constant = DEF.replacen("to stock = stock - 1\n  }\n  transition like", "to stock = 2000\n  }\n  transition like", 1);
    assert!(facts(&constant).unwrap_err().contains("BAD_RANGE"), "범위 밖 상수 대입");
    let f = facts(DEF).unwrap();
    assert_eq!(f["resources"]["Product"]["transitions"]["sell"]["to"]["stock"], json!({ "increment": -1 }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_increments_and_stock_floor() {
    sqlgen::set_schema(&format!("aip_v3_counter_{}", std::process::id()));
    let f = facts(DEF).unwrap();
    let mut db = connect().await;
    for s in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&s).await.unwrap_or_else(|e| panic!("DDL {s}: {e}"));
    }
    let s = sqlgen::schema();
    db.batch_execute(&format!("INSERT INTO {s}.member (id) VALUES (1); INSERT INTO {s}.product (id, stock, likes) VALUES (1, 3, 0)")).await.unwrap();
    let k = Knobs::default();
    let req = |t: &str| json!({ "apply": format!("Product.{t}"), "target": { "ids": ["1"] } });

    // 20개 동시 좋아요가 전부 반영된다(덮어쓰기 없음).
    let mut tasks = vec![];
    for _ in 0..20 {
        let (f, r) = (f.clone(), req("like"));
        tasks.push(tokio::spawn(async move {
            let mut db = connect().await;
            apply(&mut db, &f, &r, &who(Some(1)), &Knobs::default()).await.map(|_| ()).map_err(|e| e.code)
        }));
    }
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    let likes: i64 = db.query_one(format!("SELECT likes FROM {s}.product WHERE id = 1").as_str(), &[]).await.unwrap().get(0);
    assert_eq!(likes, 20);

    // 재고 3에서 판매 3번은 성공, 4번째는 from(stock > 0)으로 거부된다.
    for _ in 0..3 {
        apply(&mut db, &f, &req("sell"), &who(Some(1)), &k).await.unwrap();
    }
    let fourth = apply(&mut db, &f, &req("sell"), &who(Some(1)), &k).await.unwrap_err();
    assert_eq!(fourth.code, "INVALID_STATE", "{}", fourth.msg);
    let stock: i64 = db.query_one(format!("SELECT stock FROM {s}.product WHERE id = 1").as_str(), &[]).await.unwrap().get(0);
    assert_eq!(stock, 0);
    // from 검사가 없는 감소도 선언 범위(0..1000) 밖으로는 못 간다. 전체 롤백.
    let below = apply(&mut db, &f, &req("drain"), &who(Some(1)), &k).await.unwrap_err();
    assert_eq!(below.code, "BAD_VALUE", "{}", below.msg);
    let stock: i64 = db.query_one(format!("SELECT stock FROM {s}.product WHERE id = 1").as_str(), &[]).await.unwrap().get(0);
    assert_eq!(stock, 0);
    // 익명은 증감할 수 없다.
    assert_eq!(apply(&mut db, &f, &req("like"), &who(None), &k).await.unwrap_err().code, "FORBIDDEN");
    db.batch_execute(&format!("DROP SCHEMA {s} CASCADE")).await.unwrap();
}
