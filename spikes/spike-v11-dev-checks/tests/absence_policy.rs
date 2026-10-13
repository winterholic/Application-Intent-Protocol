//! Delegated decision CD-5 (security audit O-26): a write whose condition needs another row to be absent is correct
//! but expensive (SERIALIZABLE plus retries). The development check says that uniqueness and capacity are cheaper as
//! `unique` or `limit`/`invariant`. The definition stays valid and the contract does not change.
use serde_json::json;
use spike_v11_dev_checks::load_checked;
use spike_v1_fixture::{digest, Form};

const SPEC: &str = r#"
enum BookingStatus { REQUESTED, CONFIRMED }
resource Member { fields { id: Id } }
actor Member
resource Room { fields { id: Id } rows read when true }
predicate roomFree(r: Room) = not exists Booking where room = r and status = CONFIRMED
predicate roomHasBooking(r: Room) = exists Booking where room = r
resource Booking {
  fields { id: Id; room: Room; status: BookingStatus }
  rows read when true
  transition confirm { allow roomFree(this.room); from status = REQUESTED; to status = CONFIRMED }
  transition inspect { allow roomHasBooking(this.room); from status = REQUESTED; to status = REQUESTED }
  transition plain { allow true; from status = REQUESTED; to status = CONFIRMED }
  expose create { allow roomFree(this.room); fields room, status }
}
"#;

#[test]
fn only_the_not_exists_writes_are_advised() {
    let on = load_checked(SPEC, Form::A, &json!({"devChecks": true})).expect("valid");
    let mut anchors: Vec<(&str, &str)> = on.advisories.iter().map(|a| (a.code, a.anchor.as_str())).collect();
    anchors.sort();
    assert_eq!(
        anchors,
        vec![("ADVISORY_ABSENCE_POLICY", "create:Booking"), ("ADVISORY_ABSENCE_POLICY", "transition:Booking.confirm")],
        "{anchors:?}"
    );
    assert!(on.advisories[0].msg.contains("unique"), "the advice names the cheaper tools: {}", on.advisories[0].msg);

    let off = load_checked(SPEC, Form::A, &json!({"devChecks": false})).expect("valid");
    assert!(off.advisories.is_empty(), "advice is opt-in");
    assert_eq!(digest(&on.output.execution), digest(&off.output.execution), "advice never changes the contract");
}
