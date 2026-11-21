//! 1.0 정책 경화 회귀(plan-docs/hardening-1.0/policy.md). 숨겨진 필드(`field ... read when`)의 값이 정의 안의 다른 경로를 통해
//! 가려진 채로 남지 않고 호출자에게 건너가는 우회를 정의 단계에서 거절한다. 모든 반례는 PostgreSQL 17 serve로 값 복원을 확인한 뒤 고쳤다.
use spike_v1_fixture::{load_str, Form};

const POLICY: &str = "DEF.POLICY_FIELD_NOT_USABLE_IN_TRANSITION";

/// `Doc.risk`는 소유자만 읽는다. `Member.score`는 role이 9인 행에서만 읽힌다(호출자 자신의 행에서도 보통 가려진다).
const BASE: &str = "
actor Member
resource Member {
  fields { id: Id; org: Int; score: Int; role: Int }
  rows read when true
  field score read when role = 9
  expose read { select id, org, score, role; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
}
predicate big(d: Doc) = d.risk > 3
predicate scoreHigh(m: Member) = m.score > 3
resource Log { fields { id: Id; owner: Member; val: Int; flag: Bool? } rows read when true }
resource Doc {
  fields { id: Id; owner: Member; org: Int; cap: Int; flag: Bool?; risk: Int }
  rows read when true
  field risk read when owner = actor
  TRANSITIONS
  expose read { select id, owner, org, cap, flag, risk; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }
  EXPOSE
}
";

fn doc(transitions: &str, expose: &str) -> String {
    BASE.replace("TRANSITIONS", transitions).replace("EXPOSE", expose)
}

fn apply_t(transition: &str) -> String {
    doc(transition, "expose apply t { target id; bulk maxRows 1 }")
}

fn codes(src: &str) -> Vec<String> {
    match load_str(src, Form::A) {
        Ok(_) => vec!["OK".into()],
        Err(ds) => ds.iter().map(|d| d.code.to_string()).collect(),
    }
}

fn rejected(src: &str) {
    let c = codes(src);
    assert!(c.iter().any(|x| x == POLICY), "want {POLICY}, got {c:?}\n{src}");
}

fn accepted(src: &str) {
    let c = codes(src);
    assert_eq!(c, vec!["OK".to_string()], "{src}");
}

/// PG17 serve 재현: `from big(this)`는 호출자가 INVALID_STATE/성공으로 `risk > 3`을 읽었고, `to flag = big(this)`는 그 비트를
/// 읽을 수 있는 `flag`에 복사했다. 술어 본문이 호출 지점에서 안 보여 `from`·`to`의 직접 경로 검사를 지나갔다.
#[test]
fn from_and_to_values_cannot_read_a_hidden_field_through_a_predicate() {
    rejected(&apply_t("transition t { from big(this); allow actor != null; to flag = true }"));
    rejected(&apply_t("transition t { from true; allow actor != null; to flag = big(this) }"));
    // 다른 술어를 거쳐도, 논리 연산 안에 있어도 같다.
    let nested =
        format!("predicate viaBig(d: Doc) = big(d)\n{}", apply_t("transition t { from viaBig(this) and true; allow actor != null; to flag = true }"));
    rejected(&nested);
    // 가려지지 않은 필드만 읽는 술어와 `allow`의 한 비트 상수 비교는 기존대로 된다.
    accepted(&apply_t("transition t { from true; allow big(this); to flag = true }"));
    let plain = format!(
        "predicate capped(d: Doc) = d.cap > 3\n{}",
        apply_t("transition t { from capped(this); allow actor != null; to flag = capped(this) }")
    );
    accepted(&plain);
}
/// PG17 serve 재현: 호출자 자신의 행에서 가려진 `Member.score`(5)를 `allow actor.score >= input.n`의 FORBIDDEN 경계로 복원했다.
/// `hidden_reads`가 `this` 뿌리만 따라가 `actor` 뿌리 경로는 B-2 검사를 지나갔다.
#[test]
fn the_actor_root_is_checked_like_this_for_hidden_fields() {
    let with_input =
        |allow: &str, from: &str| apply_t(&format!("transition t {{ input {{ n: Int(1..100) }} from {from}; allow {allow}; to flag = true }}"));
    rejected(&with_input("actor.score >= input.n", "true"));
    rejected(&format!("predicate atLeast(m: Member, n: Int) = m.score >= n\n{}", with_input("atLeast(actor, input.n)", "true")));
    rejected(&apply_t("transition t { from actor.score >= 3; allow actor != null; to flag = true }"));
    // 호출자가 고르지 않은 한 비트(`allow actor.score >= 3`)와 가려지지 않은 actor 필드는 기존대로 된다.
    accepted(&apply_t("transition t { from true; allow actor.score >= 3; to flag = true }"));
    accepted(&with_input("actor.org >= input.n", "actor.org = 1"));
}
