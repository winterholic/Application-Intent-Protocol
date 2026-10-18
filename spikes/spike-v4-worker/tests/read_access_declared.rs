//! 읽기 확장의 `ctx.data.read(Resource)`는 확장 계약에 `read.<Resource>` 권한이 있을 때만 host 실행으로 간다.
//! 현재 언어는 읽기 확장에서 `read.<Resource>`를 선언할 수 없으므로(`access R.aggregate`만 표현), 허용 경로는
//! 수동 facts의 `"read.<Resource>": true`(managed job과 같은 기존 표기)뿐이고 그 외 값은 fail closed다.
use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::id_wire::IdWire;
use spike_v2_read::plan::{Caller, Plan, Reject};
use spike_v2_read::sqlgen::Dialect;
use spike_v4_worker::host::HostFuture;
use spike_v4_worker::{invoke_with_host, Isolation, Lang, Worker, WorkerHost};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

const FIXTURE: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const EXT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/read_access");

struct CountingHost {
    executed: Arc<AtomicUsize>,
    rows: Vec<Value>,
}

impl WorkerHost for CountingHost {
    fn dialect(&self) -> Dialect {
        Dialect::MySql
    }
    fn execute<'a>(&'a mut self, _plan: &'a Plan, _wire: IdWire) -> HostFuture<'a, Result<Vec<Value>, Reject>> {
        self.executed.fetch_add(1, Ordering::SeqCst);
        let rows = self.rows.clone();
        Box::pin(async move { Ok(rows) })
    }
}

fn facts(implementation: &str, access: Value) -> Value {
    let mut facts = load_str(FIXTURE, Form::A).expect("valid fixture").execution;
    let extensions = facts["resources"]["Recruitment"]["extensions"].as_object_mut().unwrap();
    extensions.clear();
    extensions.insert(
        "probe".into(),
        json!({
            "kind": "read", "effect": "none", "deadlineMs": 5000,
            "input": [["clubId", "Id<Club>"]], "output": [["approvedApplicants", "Int"]],
            "access": access, "implementation": format!("read_access.{implementation}"),
        }),
    );
    facts
}

/// 확장을 실행하고 (결과, host가 실행한 plan 수)를 돌려준다.
async fn run(implementation: &str, access: Value) -> (Result<Value, Reject>, usize) {
    let executed = Arc::new(AtomicUsize::new(0));
    // 집계는 첫 행의 값, read는 빈 행 목록이면 충분하다.
    let rows = if implementation == "aggregateOnly" { vec![json!(0)] } else { Vec::new() };
    let mut host = CountingHost { executed: executed.clone(), rows };
    let mut worker = Worker::start_with(Lang::Python, EXT_DIR, Isolation::Auto).await;
    let caller = Caller { actor_id: Some(1), now: "2026-10-18T00:00:00Z".into() };
    let result = invoke_with_host(
        &mut host,
        &mut worker,
        &facts(implementation, access),
        "Recruitment.probe",
        &json!({"clubId": 10}),
        &caller,
        IdWire::SafeNumber,
    )
    .await;
    worker.stop().await;
    (result, executed.load(Ordering::SeqCst))
}

/// worker가 본 ctx.data.read 실패는 확장 오류 메시지로 오므로 코드 이름이 들어 있는지 본다.
fn denied_before_host(outcome: &(Result<Value, Reject>, usize)) -> bool {
    let (result, executed) = outcome;
    *executed == 0 && result.as_ref().err().is_some_and(|reject| format!("{}{}", reject.code, reject.msg).contains("ACCESS_NOT_DECLARED"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_without_any_declaration_never_reaches_the_host() {
    let outcome = run("readRecruitment", json!({})).await;
    assert!(denied_before_host(&outcome), "{outcome:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_grant_for_another_resource_or_a_look_alike_name_is_not_enough() {
    for access in [json!({"read.Apply": true}), json!({"read.RecruitmentX": true, "read.Recruit": true, "read.recruitment": true})] {
        let outcome = run("readRecruitment", access.clone()).await;
        assert!(denied_before_host(&outcome), "{access}: {outcome:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aggregate_permission_is_not_read_permission() {
    for access in [json!({"Apply.approvedCount": {"clubId": "input.clubId"}}), json!({"Recruitment.bookmarkCount": {}})] {
        let outcome = run("readRecruitment", access.clone()).await;
        assert!(denied_before_host(&outcome), "{access}: {outcome:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exactly_declared_read_and_declared_aggregate_still_work() {
    let (result, executed) = run("readRecruitment", json!({"read.Recruitment": true})).await;
    assert_eq!(result.expect("declared read"), json!({"approvedApplicants": 0}));
    assert_eq!(executed, 1);
    let (result, executed) = run("aggregateOnly", json!({"Apply.approvedCount": {"clubId": "input.clubId"}})).await;
    assert_eq!(result.expect("declared aggregate"), json!({"approvedApplicants": 0}));
    assert_eq!(executed, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_grant_in_any_other_representation_is_rejected_not_reinterpreted() {
    for access in [json!({"read.Recruitment": {}}), json!({"read.Recruitment": false}), json!({"read.Recruitment": "true"})] {
        let (result, executed) = run("readRecruitment", access.clone()).await;
        assert_eq!(executed, 0, "{access}");
        assert_eq!(result.expect_err("fail closed").code, "WIRE.BAD_VALUE", "{access}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writes_stay_rejected_in_a_read_extension_even_with_a_read_grant() {
    let outcome = run("applyThroughRead", json!({"read.Recruitment": true})).await;
    assert_eq!(outcome.1, 0);
    assert!(outcome.0.is_err(), "{outcome:?}");
}
