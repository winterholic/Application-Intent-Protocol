use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::Caller;
use spike_v2_read::{connect, sqlgen};
use spike_v4_worker::{invoke, Isolation, Lang, Worker};
use std::time::Instant;

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const NOW: &str = "2026-10-04T00:00:00Z";

fn facts(implementation: &str) -> Value {
    let s = A.replacen("implementation \"recruitment.stats\"", &format!("implementation \"recruitment.{implementation}\""), 1);
    load_str(&s, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution
}
fn who(id: Option<i64>) -> Caller {
    Caller { actor_id: id, now: NOW.into() }
}
fn short(r: Result<Value, spike_v2_read::plan::Reject>) -> String {
    match r {
        Ok(v) => format!("OK {v}"),
        Err(e) if e.code == "EXTENSION_ERROR" => format!("EXTENSION_ERROR({})", e.msg.rsplit(": ").next().unwrap_or("")),
        Err(e) => e.code.to_string(),
    }
}

const SEED: &str = "
INSERT INTO S.school (id, name) VALUES (1, 'A대');
INSERT INTO S.member (id, school_id) VALUES (1, 1), (2, 1), (3, 1);
INSERT INTO S.club (id, name, logo, school_id) VALUES (10, 'A동아리', NULL, 1), (11, 'B동아리', NULL, 1);
INSERT INTO S.club_member (club_id, member_id, role) VALUES (10, 1, 'MANAGER'), (10, 2, 'MEMBER'), (11, 3, 'ADMIN');
INSERT INTO S.recruitment (id, title, period_end, status, views, club_id, internal_note) VALUES
 (100, 'A 모집', '2026-10-10T00:00:00Z', 'PUBLISHED', 0, 10, NULL), (101, 'B 모집', '2026-10-12T00:00:00Z', 'PUBLISHED', 0, 11, NULL);
INSERT INTO S.apply (id, recruitment_id, status) VALUES (200, 100, 'APPROVE'), (201, 100, 'PENDING'), (202, 101, 'APPROVE'), (203, 101, 'APPROVE');
";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn v4_1_read_extension_node_and_python() {
    sqlgen::set_schema("aip_v4");
    let s = sqlgen::schema();
    let base = facts("stats");
    let mut db = connect().await;
    for st in sqlgen::ddl(&base).unwrap() {
        db.batch_execute(&st).await.unwrap_or_else(|e| panic!("DDL {st}: {e}"));
    }
    db.batch_execute(&SEED.replace("S.", &format!("{s}."))).await.expect("seed");
    let ext_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/extensions");
    let mut fails: Vec<String> = vec![];
    let mut table: Vec<String> = vec![];
    let c10 = json!({ "clubId": "10" });

    for lang in [Lang::Node, Lang::Python] {
        let mut w = Worker::start(lang, ext_dir).await;
        let cases: Vec<(&str, &str, Option<i64>, Value, &str)> = vec![
            ("정상: 관리자 자기 동아리", "stats", Some(1), c10.clone(), "OK {\"approvedApplicants\":1}"),
            ("다른 동아리 관리자는 자기 동아리만", "stats", Some(3), json!({ "clubId": "11" }), "OK {\"approvedApplicants\":2}"),
            ("일반 회원", "stats", Some(2), c10.clone(), "EXTENSION_ERROR(ACCESS_DENIED)"),
            ("익명", "stats", None, c10.clone(), "EXTENSION_ERROR(ACCESS_DENIED)"),
            ("확장이 동아리 id를 바꿈", "statsOtherClub", Some(1), c10.clone(), "EXTENSION_ERROR(INPUT_BINDING)"),
            ("계약에 없는 집계 접근", "statsUndeclared", Some(1), c10.clone(), "EXTENSION_ERROR(ACCESS_NOT_DECLARED)"),
            ("ctx 요청에 actor 끼워 넣기", "statsImpersonate", Some(2), c10.clone(), "EXTENSION_ERROR(UNKNOWN_KEY)"),
            ("출력 타입 위반", "statsBadOutput", Some(1), c10.clone(), "OUTPUT_INVALID"),
            ("선언 안 된 출력 키", "statsExtraOutput", Some(1), c10.clone(), "OUTPUT_INVALID"),
            ("입력 타입 위반", "stats", Some(1), json!({ "clubId": 10 }), "BAD_VALUE"),
            ("선언 안 된 입력 키", "stats", Some(1), json!({ "clubId": "10", "all": true }), "BAD_VALUE"),
            ("worker 환경의 DB 변수", "probeEnv", Some(1), c10.clone(), "OK {\"approvedApplicants\":0}"),
            // 격리 없음: 환경 변수를 지워도 확장은 로컬 DB 포트에 직접 연결할 수 있다(RK-05).
            ("worker의 DB 포트 직접 연결", "probeDb", Some(1), c10.clone(), "OK {\"approvedApplicants\":1}"),
        ];
        for (name, imp, actor, input, want) in &cases {
            let got = short(invoke(&mut db, &mut w, &facts(imp), "Recruitment.stats", input, &who(*actor)).await);
            table.push(format!("{lang:?} | {name} | {got}"));
            if got != *want {
                fails.push(format!("{lang:?} {name}: 기대 {want}, 실제 {got}"));
            }
        }
        // 기한 초과 뒤, 그 호출의 ctx를 다음 호출에서 재사용
        let st = Instant::now();
        let slow = short(invoke(&mut db, &mut w, &facts("statsSlow"), "Recruitment.stats", &c10, &who(Some(1))).await);
        let ms = st.elapsed().as_millis();
        table.push(format!("{lang:?} | 기한 초과 | {slow} ({ms}ms)"));
        if slow != "DEADLINE_EXCEEDED" || !(1900..2600).contains(&ms) {
            fails.push(format!("{lang:?} 기한 초과: {slow} {ms}ms"));
        }
        let reuse = short(invoke(&mut db, &mut w, &facts("statsReuse"), "Recruitment.stats", &c10, &who(Some(1))).await);
        table.push(format!("{lang:?} | 끝난 호출의 ctx 재사용 | {reuse}"));
        if reuse != "EXTENSION_ERROR(TOKEN_EXPIRED)" {
            fails.push(format!("{lang:?} ctx 재사용: {reuse}"));
        }
        // 느린 호출의 늦은 done이 섞여도 다음 호출은 정상
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        let after = short(invoke(&mut db, &mut w, &base, "Recruitment.stats", &c10, &who(Some(1))).await);
        if after != "OK {\"approvedApplicants\":1}" {
            fails.push(format!("{lang:?} 늦은 응답 뒤 정상 호출: {after}"));
        }
        // worker 비정상 종료
        let crash = short(invoke(&mut db, &mut w, &facts("statsCrash"), "Recruitment.stats", &c10, &who(Some(1))).await);
        table.push(format!("{lang:?} | worker 종료 | {crash}"));
        if crash != "WORKER_FAILED" {
            fails.push(format!("{lang:?} worker 종료: {crash}"));
        }
        w.stop().await;
        let mut w2 = Worker::start(lang, ext_dir).await;
        let again = short(invoke(&mut db, &mut w2, &base, "Recruitment.stats", &c10, &who(Some(1))).await);
        if again != "OK {\"approvedApplicants\":1}" {
            fails.push(format!("{lang:?} 재기동 후: {again}"));
        }
        w2.stop().await;
    }
    // Codex r4b: ctx 집계가 DB 잠금을 기다리는 동안 기한 초과(F05), 생략된 nullable 키(F08), enum 출력(F09)
    let f100 = load_str(&A.replacen("deadline 2s\n    implementation", "deadline 100ms\n    implementation", 1), Form::A).unwrap().execution;
    let fnote =
        load_str(&A.replacen("output { approvedApplicants: Int }", "output { approvedApplicants: Int; note: Text? }", 1), Form::A).unwrap().execution;
    let fenum = load_str(
        &A.replacen("output { approvedApplicants: Int }", "output { approvedApplicants: ClubRole }", 1).replacen(
            "implementation \"recruitment.stats\"",
            "implementation \"recruitment.roleOut\"",
            1,
        ),
        Form::A,
    )
    .unwrap()
    .execution;
    for lang in [Lang::Node, Lang::Python] {
        let mut w = Worker::start(lang, ext_dir).await;
        let locker = connect().await;
        locker.batch_execute(&format!("BEGIN; LOCK TABLE {s}.apply IN ACCESS EXCLUSIVE MODE")).await.unwrap();
        let rel = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(450)).await;
            locker.batch_execute("ROLLBACK").await.unwrap();
        });
        let st = Instant::now();
        let r = short(invoke(&mut db, &mut w, &f100, "Recruitment.stats", &c10, &who(Some(1))).await);
        let ms = st.elapsed().as_millis();
        rel.await.unwrap();
        table.push(format!("{lang:?} | DB 대기 중 기한(100ms) | {r} ({ms}ms)"));
        if r != "DEADLINE_EXCEEDED" || ms > 300 {
            fails.push(format!("{lang:?} F05 DB 대기 기한: {r} {ms}ms"));
        }
        w.stop().await;
        let mut w = Worker::start(lang, ext_dir).await;
        let r = short(invoke(&mut db, &mut w, &fnote, "Recruitment.stats", &c10, &who(Some(1))).await);
        if r != "OUTPUT_INVALID" {
            fails.push(format!("{lang:?} F08 nullable 키 생략: {r}"));
        }
        let r = short(invoke(&mut db, &mut w, &fenum, "Recruitment.stats", &c10, &who(Some(1))).await);
        if r != "OK {\"approvedApplicants\":\"ADMIN\"}" {
            fails.push(format!("{lang:?} F09 enum 출력: {r}"));
        }
        w.stop().await;
    }
    // 격리 대안: macOS sandbox-exec로 worker 네트워크 차단. ctx 경로는 동작하고 DB 직접 연결만 막혀야 한다.
    for lang in [Lang::Node, Lang::Python] {
        let mut w = Worker::start_with(lang, ext_dir, Isolation::MacNetDeny).await;
        let db_probe = short(invoke(&mut db, &mut w, &facts("probeDb"), "Recruitment.stats", &c10, &who(Some(1))).await);
        let ok = short(invoke(&mut db, &mut w, &base, "Recruitment.stats", &c10, &who(Some(1))).await);
        table.push(format!("{lang:?} | 네트워크 차단 worker: DB 직접 연결 {db_probe}, ctx 경로 {ok}"));
        if db_probe != "OK {\"approvedApplicants\":0}" || ok != "OK {\"approvedApplicants\":1}" {
            fails.push(format!("{lang:?} 네트워크 차단: DB {db_probe}, ctx {ok}"));
        }
        w.stop().await;
    }
    eprintln!("{}", table.join("\n"));
    db.batch_execute(&format!("DROP SCHEMA IF EXISTS {s} CASCADE")).await.ok();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}
