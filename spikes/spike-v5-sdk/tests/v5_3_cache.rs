//! V5-3 SDK 캐시 런타임. 실제 facts에서 읽기 deps·쓰기 태그를 만들고 node 테스트로 캐시 동작을 확인한다.
use serde_json::json;
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::plan::{plan_read, Caller};
use spike_v5_sdk::write_tags;
use std::process::Command;

const A: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const EXTRA: &str = "
resource MemberAlarm {
  fields { id: Id; member: Member; isChecked: Bool }
  rows read when member = actor
  transition read { allow member = actor; from isChecked = false; to isChecked = true; repeat unchanged }
  expose read { select id, isChecked; filter isChecked.eq; budget { rows 100; depth 1; deadline 1s; cost 100 } }
  expose apply read { target id; bulk maxRows 10 }
}
";

#[test]
fn v5_3_cache_runtime() {
    let src = A
        .replacen("  fields { id: Id; recruitment: Recruitment; status: ApplyStatus }", "  fields { id: Id; recruitment: Recruitment; member: Member; status: ApplyStatus }", 1)
        .replacen(
            "  expose aggregate approvedCount\n",
            "  expose aggregate approvedCount\n  transition approve { allow managerOf(actor, recruitment.club); from status = PENDING; to status = APPROVE\n    create ClubMember { club = recruitment.club; member = member; role = MEMBER }\n    notify member \"apply.approved\" }\n  expose apply approve { target id; bulk maxRows 100 }\n",
            1,
        );
    let f = load_str(&format!("{src}\n{EXTRA}"), Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let c = Caller { actor_id: Some(1), now: "2026-10-04T00:00:00Z".into() };
    let list = plan_read(&f, &json!({ "read": "Recruitment", "select": ["id", "internalNote"] }), &c).unwrap();
    let alarms = plan_read(&f, &json!({ "read": "MemberAlarm", "select": ["id"] }), &c).unwrap();
    let fx = json!({
        "listDeps": list.deps, "alarmDeps": alarms.deps,
        "writeTags": { "MemberAlarm.read": write_tags(&f, "MemberAlarm.read"), "Apply.approve": write_tags(&f, "Apply.approve") }
    });
    eprintln!("{fx}");
    std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/sdk/cache-fixture.json"), fx.to_string()).unwrap();
    let out = Command::new("node")
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/sdk"))
        .args(["--test", "cache.test.ts", "lifetime.test.ts"])
        .output()
        .unwrap();
    let txt = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{txt}{}", String::from_utf8_lossy(&out.stderr));
    eprintln!("{}", txt.lines().filter(|l| l.starts_with("# ")).collect::<Vec<_>>().join("\n"));
}
