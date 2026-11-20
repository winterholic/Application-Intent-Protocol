//! The public-count advisory is a contract notice: it is reported with the development checks off or on.
//! The caller-filter oracle is a definition error and never depends on the options.

use serde_json::{json, Value};
use spike_v11_dev_checks::load_checked;
use spike_v1_fixture::{digest, Form};

const OPEN_COUNT: &str = r#"
actor Member
access allMembers(a: Member) = true
resource Member {
  fields { id: Uuid; owner: Member; name: Text(1..100) }
  rows read when owner = actor
  aggregate nameCount: Int {
    source Member
    sourceAccess allMembers(actor)
    callerFilter none
    rowOutput none
    release count
  }
  expose aggregate nameCount
}
"#;

const OPTIONS: [fn() -> Value; 3] = [|| json!({}), || json!({"devChecks": false}), || json!({"devChecks": true})];

#[test]
fn public_count_advisory_is_reported_without_dev_checks_and_does_not_change_facts() {
    let mut digests = vec![];
    for options in OPTIONS {
        let checked = load_checked(OPEN_COUNT, Form::A, &options()).expect("공개 count는 오류가 아니다");
        let found: Vec<_> = checked.advisories.iter().map(|a| a.code).collect();
        assert_eq!(found, ["DEF.ADVISORY_PUBLIC_COUNT"], "{}", options());
        assert!(checked.advisories[0].msg.contains("nameCount"));
        digests.push(digest(&checked.output.execution));
    }
    assert!(digests.windows(2).all(|w| w[0] == w[1]), "경고는 facts(실행 digest)를 바꾸지 않는다");
}

#[test]
fn caller_filter_oracle_is_an_error_whatever_the_options() {
    let oracle = OPEN_COUNT.replace("callerFilter none", "callerFilter read");
    for options in OPTIONS {
        let err = load_checked(&oracle, Form::A, &options()).err().expect("오라클 정의는 거부");
        assert!(err.iter().any(|d| d.code == "DEF.CALLER_FILTER_POLICY_REQUIRED"), "{}: {err:?}", options());
    }
    let closed = oracle.replace("    callerFilter read", "    where owner = actor\n    callerFilter read");
    for options in OPTIONS {
        let checked = load_checked(&closed, Form::A, &options()).expect("정책을 where에 둔 정의는 통과");
        assert!(checked.advisories.is_empty(), "where가 있으면 공개 count 경고도 없다");
    }
}

#[test]
fn other_compiler_advisories_stay_behind_dev_checks() {
    // 암호화 정의 advisory는 계속 devChecks에서만 나온다.
    let encrypted = include_str!("../../../porting/infisical/tests/fixtures/encrypted-app.aip");
    let off = load_checked(encrypted, Form::A, &json!({})).expect("encrypted fixture");
    let on = load_checked(encrypted, Form::A, &json!({"devChecks": true})).expect("encrypted fixture");
    assert!(off.advisories.iter().all(|a| a.code == "DEF.ADVISORY_PUBLIC_COUNT"), "{:?}", off.advisories);
    assert!(on.advisories.iter().any(|a| a.code == "DEF.ADVISORY_ENCRYPTION_RUNTIME_DISABLED"));
}
