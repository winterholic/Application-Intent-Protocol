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

/// PG17 serve 재현: 효과 `create Log { val = this.risk }`가 가려진 risk(37)를 호출자가 읽는 `Log.val`에 복사했다.
/// 효과의 create·update 값, update·delete의 대상 조건 모두 전이 `to`와 같은 복사·비교 통로다.
#[test]
fn effects_cannot_copy_or_match_a_hidden_field() {
    let with_effect = |effect: &str| apply_t(&format!("transition t {{ from true; allow actor != null; to flag = true\n {effect} }}"));
    for effect in [
        "create Log { owner = this.owner; val = this.risk }",
        "create Log { owner = this.owner; val = 1; flag = big(this) }",
        "update Log where owner = this.owner { val = this.risk }",
        "update Log where owner = this.owner { flag = big(this) }",
        "update Log where val = this.risk { flag = true }",
        "delete Log where val = this.risk",
    ] {
        rejected(&with_effect(effect));
    }
    // 가려지지 않은 필드 복사와 `when`의 한 비트 상수 비교는 기존대로 된다.
    accepted(&with_effect("create Log { owner = this.owner; val = this.cap }"));
    accepted(&with_effect("update Log when big(this) where owner = this.owner { val = 1 }"));
}

/// PG17 serve 재현: compose `create OrderItem from this.sku`가 가려진 `Cart.sku`('SECRET-SKU-9')를 호출자가 읽는
/// `OrderItem.sku`에 복사했다. 출처 경로가 전이 `to` 값과 같은 복사 통로인데 검사가 없었다.
#[test]
fn compose_create_sources_cannot_copy_a_hidden_field() {
    const CART: &str = "
actor Member
resource Member { fields { id: Id; org: Int } rows read when true }
resource Cart {
  fields { id: Id; owner: Member; sku: Text(1..40); qty: Int(1..100); ordered: Bool }
  rows read when owner = actor
  HIDDEN
  transition checkout { allow owner = actor; from ordered = false; to ordered = true }
  expose apply checkout { target id; bulk maxRows 20 }
  expose compose { bulk maxRows 20; sameScope owner; transitions checkout; create OrderItem from this.owner, this.sku, this.qty }
}
resource OrderItem {
  fields { id: Id; owner: Member; sku: Text(1..40); qty: Int(1..100) }
  rows read when owner = actor
  expose create { allow owner = actor; fields owner, sku, qty }
}
";
    accepted(&CART.replace("HIDDEN", ""));
    rejected(&CART.replace("HIDDEN", "field sku read when ordered = true"));
}

/// PG17 serve 재현: `sameScope risk`에서 호출자가 `[내 행(7), 남의 행]`을 보내 NOT_SAME_SCOPE 여부로 남의 가려진 risk가 7인지
/// 알아냈다(같으면 성공, 다르면 거절). 행 가시성이 아니라 가려진 필드 값의 일치 여부가 응답으로 나간다.
#[test]
fn same_scope_cannot_be_a_hidden_field() {
    let apply_scope = |field: &str| {
        doc(
            "transition t { from true; allow actor != null; to flag = true }",
            &format!("expose apply t {{ target id; bulk maxRows 5; sameScope {field} }}"),
        )
    };
    rejected(&apply_scope("risk"));
    // 참조 이동 뒤의 가려진 필드도 같다.
    rejected(&apply_scope("owner.score"));
    accepted(&apply_scope("org"));
    accepted(&apply_scope("owner"));
    let compose_scope = |field: &str| {
        doc(
            "transition t { from true; allow actor != null; to flag = true }",
            &format!("expose compose {{ bulk maxRows 5; sameScope {field}; transitions t }}"),
        )
    };
    rejected(&compose_scope("risk"));
    accepted(&compose_scope("org"));
}

/// PG17 serve 재현: `expose create`의 allow는 호출자가 고른 새 행 값(`cap`)과 남이 못 읽는 필드를 같은 조건에서 견줄 수 있었다.
/// `allow riskAbove(cap)`(술어 안 exists)와 `allow owner.score >= cap`(참조 이동)이 각각 성공·FORBIDDEN 경계로 가려진 값(37)을 내줬다.
#[test]
fn create_allow_cannot_compare_caller_values_with_a_hidden_field() {
    let create = |allow: &str| doc("", &format!("expose create {{ allow {allow}; fields owner, org, cap, flag, risk }}"));
    let above = "predicate riskAbove(n: Int) = exists Doc where org = 1 and risk >= n\n";
    rejected(&format!("{above}{}", create("riskAbove(cap)")));
    rejected(&create("owner.score >= cap"));
    rejected(&create("actor.score >= cap"));
    // 호출자가 고른 값이 끼지 않은 한 비트 상수 비교, 가려지지 않은 필드, 새 행 자신의 필드는 기존대로 된다.
    accepted(&format!("{above}{}", create("riskAbove(1)")));
    accepted(&create("owner.org >= cap"));
    accepted(&create("owner = actor and org = actor.org and risk >= cap"));
    accepted(&create("actor.score >= 3"));
    // exists 안쪽의 `this`는 안쪽 행이라 호출자가 고른 값이 아니다.
    accepted(&create("exists Doc where owner.score >= 3"));
}

/// PG17 serve 재현: `check c when risk <= cap`과 `patch cap`으로 호출자가 cap을 바꿔 가며 WIRE.CHECK_FAILED의 경계에서
/// 가려진 risk(7)를 복원했다. check는 커밋 전에 저장된 행 전체로 평가돼서 호출자가 정하는 값과 가려진 필드를 한 조건에서 견준다.
#[test]
fn a_check_cannot_compare_a_hidden_field_with_a_caller_written_field() {
    let with = |check: &str, transition: &str| doc(&format!("check c when {check}\n{transition}"), "expose apply t { target id; bulk maxRows 1 }");
    let patch = |f: &str| format!("transition t {{ from true; allow actor != null; patch {f} }}");
    rejected(&with("risk <= cap", &patch("cap")));
    // 참조 이동 뒤의 가려진 필드와 술어 안의 비교도 같다.
    rejected(&with("owner.score >= cap", &patch("cap")));
    rejected(&format!("predicate within(d: Doc) = d.risk <= d.cap\n{}", with("within(this)", &patch("cap"))));
    // 증감 항이 `input`이면 호출자가 정하는 값이다.
    rejected(&with("risk <= cap", "transition t { input { n: Int(1..9) } from true; allow actor != null; to cap = cap + input.n }"));
    // 호출자가 쓰지 못하는 필드와의 비교, 가려지지 않은 필드끼리의 비교, 가려진 필드 자신만 쓰는 patch는 기존대로 된다.
    accepted(&with("risk <= cap", "transition t { from true; allow actor != null; to cap = cap + 1 }"));
    accepted(&with("org <= cap", &patch("cap")));
    accepted(&with("risk >= 0", &patch("risk")));
    accepted(&with("risk <= 100", &patch("cap")));
}
