//! 읽기 패턴 연산자(Ref eq, in, isNull)의 정의 문법 검사. 허용 목록 방식은 기존 연산자와 같다.
use spike_v1_fixture::{load_str, Form};

const A: &str = include_str!("../fixture/recruitment.aip");

const REC_FILTER: &str = "filter periodEnd.gte, periodEnd.lte";
const CLUB_EXPOSE: &str = "expose read { select id, name, logo }";

fn with(rec_filter: &str, club_expose: &str) -> String {
    assert_eq!(A.matches(REC_FILTER).count(), 1);
    assert_eq!(A.matches(CLUB_EXPOSE).count(), 1);
    A.replacen(REC_FILTER, rec_filter, 1).replacen(CLUB_EXPOSE, club_expose, 1)
}

fn codes(src: &str) -> Vec<String> {
    match load_str(src, Form::A) {
        Ok(_) => vec![],
        Err(ds) => ds.iter().map(|d| format!("{d}")).collect(),
    }
}

const CLUB_OK: &str =
    "expose read { select id, name, logo; filter school.eq, school.in, school.isNull, logo.isNull; budget { rows 50; depth 1; deadline 2s; cost 1000 } }";

#[test]
fn new_operators_are_accepted_and_listed_in_facts() {
    let src = with("filter periodEnd.gte, periodEnd.lte, club.eq, club.in, status.in, title.in, id.in", CLUB_OK);
    let facts = load_str(&src, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let list = |r: &str| -> Vec<String> {
        facts["resources"][r]["exposeRead"]["filter"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
    };
    let rec = list("Recruitment");
    for want in ["club.eq", "club.in", "status.in", "title.in", "id.in", "periodEnd.gte"] {
        assert!(rec.iter().any(|x| x == want), "{want} 없음: {rec:?}");
    }
    let club = list("Club");
    for want in ["school.eq", "school.in", "school.isNull", "logo.isNull"] {
        assert!(club.iter().any(|x| x == want), "{want} 없음: {club:?}");
    }
}

#[test]
fn operators_are_rejected_on_types_that_do_not_fit() {
    let cases: &[(&str, &str)] = &[
        // isNull은 nullable 필드에만(항상 non-null인 필드에 쓰면 의미가 없다)
        ("filter title.isNull", "OP_TYPE_MISMATCH"),
        ("filter club.isNull", "OP_TYPE_MISMATCH"),
        // in은 Enum/Text/Id/Ref만. Time·Int는 범위 연산 대상이다
        ("filter periodEnd.in", "OP_TYPE_MISMATCH"),
        ("filter views.in", "OP_TYPE_MISMATCH"),
        // 이름이 다른 비슷한 연산은 계속 모르는 연산
        ("filter club.isnull", "UNKNOWN_OPERATOR"),
        ("filter club.notIn", "UNKNOWN_OPERATOR"),
        ("filter club.ne", "UNKNOWN_OPERATOR"),
        ("filter nothing.in", "UNKNOWN_FIELD"),
    ];
    for (rec, want) in cases {
        let got = codes(&with(rec, CLUB_EXPOSE));
        assert!(got.iter().any(|g| g.contains(want)), "`{rec}`: 기대 {want}, 실제 {got:?}");
    }
    // nullable 허용은 CLUB_OK(school/logo.isNull)가 보인다. field read 정책 필드는 isNull로도 존재를 추론하므로 거부된다.
    assert!(codes(&with("filter internalNote.isNull", CLUB_EXPOSE)).iter().any(|g| g.contains("POLICY_FIELD_NOT_FILTERABLE")));
    assert_eq!(codes(&with(REC_FILTER, CLUB_OK)), Vec::<String>::new());
}

#[test]
fn contains_is_text_only_and_icontains_stays_unknown() {
    let ok = with("filter periodEnd.gte, periodEnd.lte, title.contains", CLUB_EXPOSE);
    let facts = load_str(&ok, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let rec = facts["resources"]["Recruitment"]["exposeRead"]["filter"].as_array().unwrap().clone();
    assert!(rec.iter().any(|x| x == "title.contains"), "{rec:?}");
    let cases: &[(&str, &str)] = &[
        ("filter views.contains", "OP_TYPE_MISMATCH"),
        ("filter periodEnd.contains", "OP_TYPE_MISMATCH"),
        ("filter status.contains", "OP_TYPE_MISMATCH"),
        ("filter club.contains", "OP_TYPE_MISMATCH"),
        // 대소문자 무시는 이번 범위가 아니다(보류). 문법에도 열지 않는다.
        ("filter title.icontains", "UNKNOWN_OPERATOR"),
        ("filter title.contain", "UNKNOWN_OPERATOR"),
        ("filter title.like", "UNKNOWN_OPERATOR"),
        // field read 정책 필드는 부분일치로도 값을 추론할 수 있다
        ("filter internalNote.contains", "POLICY_FIELD_NOT_FILTERABLE"),
    ];
    for (rec, want) in cases {
        let got = codes(&with(rec, CLUB_EXPOSE));
        assert!(got.iter().any(|g| g.contains(want)), "`{rec}`: 기대 {want}, 실제 {got:?}");
    }
}

fn budget_src(budget: &str) -> String {
    let old = "budget { rows 50; depth 2; deadline 2s; cost 1000 }";
    assert_eq!(A.matches(old).count(), 1);
    A.replacen(old, budget, 1)
}

#[test]
fn offset_is_opt_in_budget_item() {
    // 선언이 없으면 facts에 maxOffset 키 자체가 없다(기존 facts 호환).
    let base = load_str(A, Form::A).unwrap().execution;
    assert!(base["resources"]["Recruitment"]["exposeRead"]["budget"].get("maxOffset").is_none());
    let f = load_str(&budget_src("budget { rows 50; depth 2; deadline 2s; cost 1000; offset 1000 }"), Form::A)
        .unwrap_or_else(|e| panic!("{e:?}"))
        .execution;
    assert_eq!(f["resources"]["Recruitment"]["exposeRead"]["budget"]["maxOffset"], 1000);
    for (b, want) in [
        ("budget { rows 50; depth 2; deadline 2s; cost 1000; offset 0 }", "BAD_BUDGET"),
        ("budget { rows 50; depth 2; deadline 2s; cost 1000; offset 10; offset 20 }", "DUPLICATE"),
    ] {
        let got = codes(&budget_src(b));
        assert!(got.iter().any(|g| g.contains(want)), "`{b}`: 기대 {want}, 실제 {got:?}");
    }
}
