//! 독립 도메인 정의를 사용하는 OSS 업무 규칙 probe. DB 실행은 V3 integration test에서 한다.
//!
//! 원본은 공개 GitHub raw 파일을 고정 commit에서 읽었고 실행하지 않았다.
//! - https://github.com/pretix/pretix/blob/fe43862fa473c43b139bc00710aa1fc4a06ef726/src/tests/concurrency_tests/test_order_creation_locking.py#L91-L126:
//!   quota size 1에서
//!   잠금을 끄면 동시 두 주문이 모두 생성되고, 정상 잠금이면 하나만 생성된다.
//! - https://github.com/adamspd/django-appointment/blob/3e176df7141d10ea9447730b5e4d7b7d5dfc4899/appointment/models.py#L468-L492
//!   and `https://github.com/adamspd/django-appointment/blob/3e176df7141d10ea9447730b5e4d7b7d5dfc4899/appointment/tests/test_services.py#L750-L780`:
//!   예약 시작/종료·과거·서비스 길이 검증과 실제 겹치는 구간 가용성 검사를 둔다.
//! - https://github.com/docusealco/docuseal/blob/c6a7555f545ea8ea96207f198c2d8e42d16b091f/spec/requests/submissions_spec.rb#L172-L197:
//!   동일 submission의 role 중복 및
//!   템플릿 signer 수 초과를 거부한다.
//! - https://github.com/inventree/InvenTree/blob/575fbdc9072dee89bc5624cb7b2604829826f552/src/backend/InvenTree/order/test_sales_order.py#L311-L341:
//!   첫 취소가 할당을 풀고
//!   stale 두 번째 취소는 반복 side effect 없이 거부되어야 한다.
//!
//! 이 파일의 Appointment overlap 기대는 미지원 진단이다. 이를 지원 기능으로 세지 않는다.

use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};

const DOMAINS: &str = include_str!("../fixture/oss-business-domains.aip");

fn checked(src: &str) -> Value {
    load_str(src, Form::A).unwrap_or_else(|diagnostics| panic!("unexpected diagnostics: {diagnostics:?}")).execution
}

#[test]
fn independent_ticket_booking_signer_stock_rules_compile_to_explicit_facts() {
    let facts = checked(DOMAINS);
    assert_eq!(facts["resources"]["Ticket"]["invariants"]["ticketCapacity"]["enforcement"]["kind"], json!("lockedCountCheck"));
    assert_eq!(facts["resources"]["Ticket"]["invariants"]["ticketCapacity"]["enforcement"]["max"], json!(2));
    assert_eq!(facts["resources"]["Signer"]["unique"][0], json!(["submission", "role"]));
    assert_eq!(facts["resources"]["StockItem"]["checks"]["withinAvailableStock"]["and"].as_array().unwrap().len(), 2);
    assert_eq!(facts["resources"]["StockItem"]["transitions"]["reserve"]["to"]["allocated"]["increment"], json!(1));
    assert_eq!(facts["resources"]["Appointment"]["checks"]["validInterval"]["cmp"], json!("<"));
    assert_eq!(facts["resources"]["Appointment"]["transitions"]["cancel"]["effects"][0]["notify"]["topic"], json!("booking.cancelled"));
}

#[test]
fn appointment_overlap_expression_is_rejected_as_an_executable_invariant() {
    let attempt = format!(
        "{DOMAINS}\nresource OverlapProbe {{\n  fields {{ id: Id; appointment: Appointment; staff: Member; startsAt: Time; endsAt: Time }}\n  invariant noOverlap per staff\n}}\npredicate overlaps(a: OverlapProbe) = exists OverlapProbe where staff = a.staff and startsAt < a.endsAt and endsAt > a.startsAt\nlimit noOverlap on OverlapProbe = atMost 1 where overlaps(this)\n"
    );
    let errors = match load_str(&attempt, Form::A) {
        Ok(_) => panic!("overlap invariant unexpectedly passed check"),
        Err(errors) => errors,
    };
    assert!(errors.iter().any(|error| error.code == "UNSUPPORTED_INVARIANT"), "{errors:?}");
}
