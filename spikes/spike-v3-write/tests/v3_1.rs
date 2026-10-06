use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{Caller, Reject};
use spike_v2_read::{connect, sqlgen};
use spike_v3_write::{apply, Applied, Knobs};
use std::time::Instant;

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const ALARM: &str = "
resource MemberAlarm {
  fields { id: Id; member: Member; isChecked: Bool }
  rows read when member = actor
  transition read {
    allow member = actor
    from isChecked = false
    to isChecked = true
    repeat unchanged
  }
  expose read { select id, isChecked; filter isChecked.eq; budget { rows 100; depth 1; deadline 1s; cost 100 } }
  expose apply read { target id, where; bulk maxRows 3 }
}
";
const NOW: &str = "2026-10-04T00:00:00Z";

fn facts() -> Value {
    let a = A.replacen(
        "  invariant atMostOnePublished per club",
        "  expose apply close { target id; bulk maxRows 10 }\n  invariant atMostOnePublished per club",
        1,
    );
    load_str(&format!("{a}\n{ALARM}"), Form::A).unwrap_or_else(|e| panic!("facts {e:?}")).execution
}
fn who(id: Option<i64>) -> Caller {
    Caller { actor_id: id, now: NOW.into() }
}
fn ids(v: &[i64]) -> Value {
    json!({ "ids": v.iter().map(|x| x.to_string()).collect::<Vec<_>>() })
}
fn ok(changed: &[i64], unchanged: &[i64]) -> Result<Applied, &'static str> {
    Ok(Applied { changed: changed.to_vec(), unchanged: unchanged.to_vec() })
}
fn simplify(r: Result<Applied, Reject>) -> Result<Applied, &'static str> {
    r.map_err(|e| e.code)
}

const SEED: &str = "
INSERT INTO SCHEMA.school (id, name) VALUES (1, 'A대'), (2, 'B대');
INSERT INTO SCHEMA.member (id, school_id) VALUES (1, 1), (2, 1), (3, 2), (4, NULL);
INSERT INTO SCHEMA.club (id, name, logo, school_id) VALUES (10, 'A동아리', 'a.png', 1), (11, 'B동아리', NULL, 2), (12, '연합', 'u.png', NULL), (13, 'A2동아리', NULL, 1);
INSERT INTO SCHEMA.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER'), (13, 2, 'MEMBER'), (11, 3, 'ADMIN');
INSERT INTO SCHEMA.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
 (100, 'A 모집', '2026-10-10T00:00:00Z', 'PUBLISHED', 5, 10, 'A-note'),
 (101, 'B 모집', '2026-10-12T00:00:00Z', 'PUBLISHED', 1, 11, 'B-note');
INSERT INTO SCHEMA.member_alarm (id, member_id, is_checked) VALUES
 (1, 1, false), (2, 1, false), (3, 1, true), (4, 2, false), (5, 1, false), (6, 1, false), (7, 1, false), (8, 1, false), (9, 1, false);
";

async fn unchecked(db: &tokio_postgres::Client, id: i64) -> bool {
    let q = format!("SELECT NOT is_checked FROM {}.member_alarm WHERE id = $1", sqlgen::schema());
    db.query_one(q.as_str(), &[&id]).await.unwrap().get(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v3_1_standard_transition_write() {
    sqlgen::set_schema("aip_v3_spike");
    let f = facts();
    let mut db = connect().await;
    for s in sqlgen::ddl(&f).unwrap() {
        db.batch_execute(&s).await.unwrap_or_else(|e| panic!("DDL {s}: {e}"));
    }
    db.batch_execute(&SEED.replace("SCHEMA", sqlgen::schema())).await.expect("seed");
    let k = Knobs::default();
    let mut fails: Vec<String> = vec![];
    macro_rules! expect {
        ($name:expr, $got:expr, $want:expr) => {{
            let got = $got;
            let want = $want;
            if got != want {
                fails.push(format!("{}: 기대 {:?}, 실제 {:?}", $name, want, got));
            }
        }};
    }
    let alarm = |t: Value| json!({ "apply": "MemberAlarm.read", "target": t });

    // id 대상과 repeat unchanged
    expect!("내 알림 1,2 읽음", simplify(apply(&mut db, &f, &alarm(ids(&[1, 2])), &who(Some(1)), &k).await), ok(&[1, 2], &[]));
    expect!("다시 읽음은 unchanged", simplify(apply(&mut db, &f, &alarm(ids(&[1, 2, 3])), &who(Some(1)), &k).await), ok(&[], &[1, 2, 3]));
    // 남의 알림이 섞이면 전체 실패, 내 알림도 안 바뀜
    expect!("남의 알림 섞임", simplify(apply(&mut db, &f, &alarm(ids(&[5, 4])), &who(Some(1)), &k).await), Err("MISSING_TARGET"));
    expect!("실패 뒤 5 그대로", unchecked(&db, 5).await, true);
    expect!("없는 id", simplify(apply(&mut db, &f, &alarm(ids(&[5, 999])), &who(Some(1)), &k).await), Err("MISSING_TARGET"));
    expect!("익명", simplify(apply(&mut db, &f, &alarm(ids(&[4])), &who(None), &k).await), Err("MISSING_TARGET"));
    expect!("중복 id", simplify(apply(&mut db, &f, &alarm(ids(&[5, 5])), &who(Some(1)), &k).await), Err("DUPLICATE_TARGET"));
    expect!("id bulk 초과", simplify(apply(&mut db, &f, &alarm(ids(&[5, 6, 7, 8])), &who(Some(1)), &k).await), Err("BULK_LIMIT"));
    // where 대상: 내 미읽음 전부. 상한을 넘으면 아무것도 안 바뀜
    let unread = json!({ "where": [{ "field": "isChecked", "op": "eq", "value": false }] });
    expect!("where 상한 초과", simplify(apply(&mut db, &f, &alarm(unread.clone()), &who(Some(1)), &k).await), Err("BULK_LIMIT"));
    expect!("상한 초과 뒤 5 그대로", unchecked(&db, 5).await, true);
    expect!("8, 9 개별 읽음", simplify(apply(&mut db, &f, &alarm(ids(&[8, 9])), &who(Some(1)), &k).await), ok(&[8, 9], &[]));
    expect!("where 미읽음 3개", simplify(apply(&mut db, &f, &alarm(unread.clone()), &who(Some(1)), &k).await), ok(&[5, 6, 7], &[]));
    expect!("남의 알림은 where 대상 밖", unchecked(&db, 4).await, true);
    expect!("회원 2 where", simplify(apply(&mut db, &f, &alarm(unread.clone()), &who(Some(2)), &k).await), ok(&[4], &[]));
    let by_member = json!({ "where": [{ "field": "member", "op": "eq", "value": "1" }] });
    expect!("허용 안 된 where 필드", simplify(apply(&mut db, &f, &alarm(by_member), &who(Some(2)), &k).await), Err("FILTER_NOT_ALLOWED"));
    // 계약 밖 요청
    expect!(
        "모르는 키",
        simplify(apply(&mut db, &f, &json!({ "apply": "MemberAlarm.read", "target": ids(&[1]), "force": true }), &who(Some(1)), &k).await),
        Err("UNKNOWN_KEY")
    );
    expect!(
        "공개 안 된 동작",
        simplify(apply(&mut db, &f, &json!({ "apply": "Apply.approve", "target": ids(&[1]) }), &who(Some(1)), &k).await),
        Err("NOT_EXPOSED")
    );

    // 모집 마감: 읽을 수 있어도 쓸 권한은 별개
    let close = |t: Value| json!({ "apply": "Recruitment.close", "target": t });
    expect!("학생이 마감", simplify(apply(&mut db, &f, &close(ids(&[100])), &who(Some(2)), &k).await), Err("FORBIDDEN"));
    expect!("관리자가 타 학교 모집", simplify(apply(&mut db, &f, &close(ids(&[101])), &who(Some(1)), &k).await), Err("MISSING_TARGET"));
    expect!("없는 모집", simplify(apply(&mut db, &f, &close(ids(&[999])), &who(Some(1)), &k).await), Err("MISSING_TARGET"));
    expect!("where 마감은 계약 밖", simplify(apply(&mut db, &f, &close(json!({ "where": [] })), &who(Some(1)), &k).await), Err("TARGET_NOT_ALLOWED"));
    expect!("관리자 마감", simplify(apply(&mut db, &f, &close(ids(&[100])), &who(Some(1)), &k).await), ok(&[100], &[]));
    // 마감된 모집은 행 정책(active)에서 빠져 관리자에게도 안 보인다. unchanged가 아니라 MISSING_TARGET이 된다.
    expect!("마감 재시도", simplify(apply(&mut db, &f, &close(ids(&[100])), &who(Some(1)), &k).await), Err("MISSING_TARGET"));

    // 동시 실행: 잠금이 있으면 두 번째는 기다렸다가 unchanged
    db.batch_execute(&format!("UPDATE {}.member_alarm SET is_checked = false WHERE id = 9", sqlgen::schema())).await.unwrap();
    let (r1, r2, waited) =
        race(&f, Knobs { pause_after_lock_ms: 400, skip_row_lock: false, skip_policy_lock: false, pause_before_commit_ms: 0 }, Knobs::default())
            .await;
    expect!("잠금 T1", r1, ok(&[9], &[]));
    expect!("잠금 T2", r2, ok(&[], &[9]));
    expect!("T2가 잠금을 기다림", waited >= 300, true);
    // 잠금을 끄면 UPDATE의 from 재검사로 한쪽이 CONFLICT. 두 번 바뀌는 일은 없다.
    db.batch_execute(&format!("UPDATE {}.member_alarm SET is_checked = false WHERE id = 9", sqlgen::schema())).await.unwrap();
    let (r1, r2, _) = race(
        &f,
        Knobs { pause_after_lock_ms: 400, skip_row_lock: true, skip_policy_lock: false, pause_before_commit_ms: 0 },
        Knobs { pause_after_lock_ms: 0, skip_row_lock: true, skip_policy_lock: false, pause_before_commit_ms: 0 },
    )
    .await;
    let outcomes = [&r1, &r2];
    expect!("무잠금: 한쪽만 changed", outcomes.iter().filter(|r| ***r == ok(&[9], &[])).count(), 1);
    expect!("무잠금: 다른 쪽 CONFLICT", outcomes.iter().filter(|r| ***r == Err("CONFLICT")).count(), 1);

    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", sqlgen::schema())).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
    eprintln!("v3-1 scenarios ok");
}

async fn race(f: &Value, k1: Knobs, k2: Knobs) -> (Result<Applied, &'static str>, Result<Applied, &'static str>, u128) {
    let (f1, f2) = (f.clone(), f.clone());
    let req = json!({ "apply": "MemberAlarm.read", "target": ids(&[9]) });
    let (q1, q2) = (req.clone(), req);
    let t1 = tokio::spawn(async move {
        let mut db = connect().await;
        simplify(apply(&mut db, &f1, &q1, &who(Some(1)), &k1).await)
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let t2 = tokio::spawn(async move {
        let mut db = connect().await;
        let st = Instant::now();
        let r = simplify(apply(&mut db, &f2, &q2, &who(Some(1)), &k2).await);
        (r, st.elapsed().as_millis())
    });
    let r1 = t1.await.unwrap();
    let (r2, ms) = t2.await.unwrap();
    (r1, r2, ms)
}
