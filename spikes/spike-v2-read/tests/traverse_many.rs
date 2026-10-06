//! 1:N traverse. 자식 행마다 rowRead·fieldRead를 다시 적용하고, 개수 상한·정렬은 정의가 정하며, 값은 전부 바인딩된다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller, Plan, Reject};
use spike_v2_read::{connect, execute, sqlgen};

const DEF: &str = "
enum CommentState { VISIBLE, HIDDEN }
resource Member { fields { id: Id } }
actor Member
resource Post {
  fields { id: Id; title: Text(1..100) }
  rows read when true
  expose read {
    select id, title
    sort id
    traverse comments via Comment.post { select id, body, secret; sort id; limit 3 }
    budget { rows 10; depth 2; deadline 2s; cost 100 }
  }
}
resource Comment {
  fields { id: Id; post: Post; author: Member; body: Text(1..200); secret: Text?; state: CommentState }
  rows read when state = VISIBLE
  field secret read when author = actor
  expose read { select id, body, secret, state }
}
";

fn facts() -> Value {
    load_str(DEF, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}

fn who(id: Option<i64>) -> Caller {
    Caller { actor_id: id, now: "2026-10-07T00:00:00Z".into() }
}

fn code(r: Result<Plan, Reject>) -> &'static str {
    match r {
        Ok(_) => "OK",
        Err(e) => e.code,
    }
}

fn req(sub: Value) -> Value {
    json!({ "read": "Post", "select": ["id", { "comments": sub }], "sort": [{ "field": "id" }] })
}

#[test]
fn plan_rejects_what_the_definition_does_not_allow() {
    let f = facts();
    let c = who(Some(1));
    // 허용 목록 밖 필드(자식의 state는 expose에 있지만 이 경로 select에 없다)
    assert_eq!(code(plan_read(&f, &req(json!({ "select": ["id", "state"] })), &c)), "FIELD_NOT_EXPOSED");
    assert_eq!(code(plan_read(&f, &req(json!({ "select": ["post"] })), &c)), "FIELD_NOT_EXPOSED");
    // 알 수 없는 키: 정렬·필터는 호출자가 정하지 않는다
    assert_eq!(code(plan_read(&f, &req(json!({ "select": ["id"], "sort": [{ "field": "id" }] })), &c)), "UNKNOWN_KEY");
    assert_eq!(code(plan_read(&f, &req(json!({ "select": ["id"], "filter": [] })), &c)), "UNKNOWN_KEY");
    // 계약에 없는 관계
    let bad = json!({ "read": "Post", "select": [{ "replies": { "select": ["id"] } }] });
    assert_eq!(code(plan_read(&f, &bad, &c)), "TRAVERSE_NOT_ALLOWED");
    // 중첩 금지(자식 안의 관계)
    assert_eq!(code(plan_read(&f, &req(json!({ "select": ["id", { "post": { "select": ["id"] } }] })), &c)), "NESTED_TRAVERSE_NOT_ALLOWED");
    // limit는 정의 상한 이하의 양의 정수
    assert_eq!(code(plan_read(&f, &req(json!({ "select": ["id"], "limit": 3 })), &c)), "OK");
    assert_eq!(code(plan_read(&f, &req(json!({ "select": ["id"], "limit": 4 })), &c)), "ROWS_EXCEEDED");
    for bad in [json!(0), json!(-1), json!("2"), json!(1.5), json!(null)] {
        assert_eq!(code(plan_read(&f, &req(json!({ "select": ["id"], "limit": bad.clone() })), &c)), "BAD_VALUE", "{bad}");
    }
    assert_eq!(code(plan_read(&f, &req(json!({ "select": [] })), &c)), "BAD_REQUEST");
}

#[test]
fn cost_counts_parents_times_child_limit_and_depth_is_checked() {
    let f = facts();
    let c = who(Some(1));
    // 부모 10 x (1 + 자식 3 x (1 + secret 정책 1)) = 70, 호출자 limit를 줄이면 줄어든다.
    let p = plan_read(&f, &req(json!({ "select": ["id", "secret"] })), &c).unwrap();
    assert_eq!(p.cost, 10 * (1 + 3 * 2), "{}", p.cost);
    let p = plan_read(&f, &req(json!({ "select": ["id"], "limit": 1 })), &c).unwrap();
    assert_eq!(p.cost, 10 * (1 + 1), "{}", p.cost);
    // budget cost를 낮추면 거부
    let mut tight = f.clone();
    tight["resources"]["Post"]["exposeRead"]["budget"]["cost"] = json!(50);
    assert_eq!(code(plan_read(&tight, &req(json!({ "select": ["id", "secret"] })), &c)), "COST_EXCEEDED");
    // depth 1이면 관계 탐색 불가(sema를 건너뛴 facts 방어)
    let mut shallow = f.clone();
    shallow["resources"]["Post"]["exposeRead"]["budget"]["depth"] = json!(1);
    assert_eq!(code(plan_read(&shallow, &req(json!({ "select": ["id"] })), &c)), "DEPTH_EXCEEDED");
}

#[test]
fn sql_binds_every_value_and_aggregates_to_json_array() {
    let f = facts();
    let p = plan_read(&f, &req(json!({ "select": ["id", "body"], "limit": 2 })), &who(Some(7))).unwrap();
    assert!(p.sql.contains("LEFT JOIN LATERAL"), "{}", p.sql);
    assert!(p.sql.contains("json_agg("), "{}", p.sql);
    assert!(p.sql.contains("'[]'"), "{}", p.sql);
    assert!(p.params.contains(&Some("2".to_string())), "{:?}", p.params);
    assert_eq!(p.output_type["rows"]["comments"]["array"]["body"]["ty"], "Text");
    assert_eq!(p.output_type["rows"]["comments"]["nullable"], false);
    assert!(p.deps.contains(&"Comment".to_string()), "{:?}", p.deps);
}

#[tokio::test]
async fn pg_policy_redaction_limit_and_empty_array() {
    sqlgen::set_schema(&format!("aip_v2_many_{}", std::process::id()));
    let f = facts();
    let mut db = connect().await;
    for stmt in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&stmt).await.unwrap_or_else(|e| panic!("DDL {stmt}: {e}"));
    }
    let s = sqlgen::schema();
    // post 1: 댓글 6개(5번은 숨김, 작성자는 1/2 섞임). post 2: 댓글 없음. post 3: 숨김 댓글만.
    db.batch_execute(&format!(
        "INSERT INTO {s}.member (id) VALUES (1), (2);
         INSERT INTO {s}.post (id, title) VALUES (1, 'a'), (2, 'b'), (3, 'c');
         INSERT INTO {s}.comment (id, post_id, author_id, body, secret, state) VALUES
           (10, 1, 1, 'c10', 's10', 'VISIBLE'),
           (11, 1, 2, 'c11', 's11', 'VISIBLE'),
           (12, 1, 2, 'c12', NULL, 'VISIBLE'),
           (13, 1, 1, 'c13', 's13', 'VISIBLE'),
           (14, 1, 1, 'c14', 's14', 'HIDDEN'),
           (15, 1, 2, 'c15', 's15', 'VISIBLE'),
           (30, 3, 1, 'c30', 's30', 'HIDDEN');
         INSERT INTO {s}.post (id, title) VALUES (4, 'd');
         INSERT INTO {s}.comment (id, post_id, author_id, body, state) VALUES
           (41, 4, 1, 'x', 'VISIBLE'), (42, 4, 1, 'x', 'VISIBLE'), (43, 4, 1, 'x', 'HIDDEN')"
    ))
    .await
    .unwrap();
    let run = |actor: Option<i64>, sub: Value| (plan_read(&f, &req(sub), &who(actor)).unwrap(), ());
    // 상한: 정의 3, 보이는 댓글은 5개. 정렬은 id 오름차순 고정이라 10, 11, 12.
    let (p, _) = run(Some(1), json!({ "select": ["id", "body", "secret"] }));
    let rows = execute(&mut db, &p).await.unwrap();
    assert_eq!(rows.len(), 4);
    let c1 = rows[0]["comments"].as_array().unwrap();
    assert_eq!(c1.iter().map(|c| c["id"].as_i64().unwrap()).collect::<Vec<_>>(), vec![10, 11, 12]);
    // 가려진 필드: 내 댓글(10)만 secret이 보이고 남의 댓글(11)은 null, 원래 null(12)도 null
    assert_eq!(c1[0]["secret"], "s10");
    assert_eq!(c1[1]["secret"], Value::Null);
    assert_eq!(c1[2]["secret"], Value::Null);
    // 자식 없는 부모, 숨김 댓글만 있는 부모는 빈 배열(null 아님)
    assert_eq!(rows[1]["comments"], json!([]));
    assert_eq!(rows[2]["comments"], json!([]));
    let (p, _) = run(Some(2), json!({ "select": ["id"], "limit": 3 }));
    let rows = execute(&mut db, &p).await.unwrap();
    assert_eq!(rows[0]["comments"], json!([{ "id": 10 }, { "id": 11 }, { "id": 12 }]));
    // 호출자 limit가 더 작으면 그만큼
    let (p, _) = run(None, json!({ "select": ["id", "secret"], "limit": 1 }));
    let rows = execute(&mut db, &p).await.unwrap();
    assert_eq!(rows[0]["comments"], json!([{ "id": 10, "secret": null }]));
    // 정렬 결정성: 다시 실행해도 같다
    let rows2 = execute(&mut db, &p).await.unwrap();
    assert_eq!(rows, rows2);
    // 내림차순 정의: 숨김 댓글(43)이 상한 2를 차지하지 않는다. post 4는 42, 41.
    let d = DEF.replacen("sort id; limit 3", "sort id desc; limit 2", 1);
    let fd = load_str(&d, Form::A).unwrap().execution;
    let p = plan_read(&fd, &req(json!({ "select": ["id"] })), &who(Some(1))).unwrap();
    let rows = execute(&mut db, &p).await.unwrap();
    assert_eq!(rows[3]["comments"], json!([{ "id": 42 }, { "id": 41 }]));
    assert_eq!(rows[0]["comments"], json!([{ "id": 15 }, { "id": 13 }]));
    db.batch_execute(&format!("DROP SCHEMA {s} CASCADE")).await.unwrap();
}
