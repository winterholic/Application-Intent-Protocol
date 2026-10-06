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
