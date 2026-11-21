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

const CLUB_OK: &str = "expose read { select id, name, logo; filter school.eq, school.in, school.isNull, logo.isNull; budget { rows 50; depth 1; deadline 2s; cost 1000 } }";

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
        ("filter title.isNull", "DEF.OP_TYPE_MISMATCH"),
        ("filter club.isNull", "DEF.OP_TYPE_MISMATCH"),
        // in은 Enum/Text/Id/Ref만. Time·Int는 범위 연산 대상이다
        ("filter periodEnd.in", "DEF.OP_TYPE_MISMATCH"),
        ("filter views.in", "DEF.OP_TYPE_MISMATCH"),
        // 이름이 다른 비슷한 연산은 계속 모르는 연산
        ("filter club.isnull", "DEF.UNKNOWN_OPERATOR"),
        ("filter club.notIn", "DEF.UNKNOWN_OPERATOR"),
        ("filter club.ne", "DEF.UNKNOWN_OPERATOR"),
        ("filter nothing.in", "DEF.UNKNOWN_FIELD"),
    ];
    for (rec, want) in cases {
        let got = codes(&with(rec, CLUB_EXPOSE));
        assert!(got.iter().any(|g| g.contains(want)), "`{rec}`: 기대 {want}, 실제 {got:?}");
    }
    // nullable 허용은 CLUB_OK(school/logo.isNull)가 보인다. field read 정책 필드는 isNull로도 존재를 추론하므로 거부된다.
    assert!(codes(&with("filter internalNote.isNull", CLUB_EXPOSE)).iter().any(|g| g.contains("DEF.POLICY_FIELD_NOT_FILTERABLE")));
    assert_eq!(codes(&with(REC_FILTER, CLUB_OK)), Vec::<String>::new());
}

#[test]
fn contains_and_like_are_text_only_and_icontains_stays_unknown() {
    let ok = with("filter periodEnd.gte, periodEnd.lte, title.contains", CLUB_EXPOSE);
    let facts = load_str(&ok, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    let rec = facts["resources"]["Recruitment"]["exposeRead"]["filter"].as_array().unwrap().clone();
    assert!(rec.iter().any(|x| x == "title.contains"), "{rec:?}");
    let like = with("filter periodEnd.gte, periodEnd.lte, title.like", CLUB_EXPOSE);
    let like_facts = load_str(&like, Form::A).unwrap_or_else(|e| panic!("{e:?}")).execution;
    assert!(like_facts["resources"]["Recruitment"]["exposeRead"]["filter"].as_array().unwrap().iter().any(|x| x == "title.like"));
    let cases: &[(&str, &str)] = &[
        ("filter views.contains", "DEF.OP_TYPE_MISMATCH"),
        ("filter periodEnd.contains", "DEF.OP_TYPE_MISMATCH"),
        ("filter status.contains", "DEF.OP_TYPE_MISMATCH"),
        ("filter club.contains", "DEF.OP_TYPE_MISMATCH"),
        ("filter views.like", "DEF.OP_TYPE_MISMATCH"),
        ("filter periodEnd.like", "DEF.OP_TYPE_MISMATCH"),
        ("filter status.like", "DEF.OP_TYPE_MISMATCH"),
        ("filter club.like", "DEF.OP_TYPE_MISMATCH"),
        // 별도 icontains 명칭은 열지 않는다. SQL LIKE 패턴은 `like`로 명시한다.
        ("filter title.icontains", "DEF.UNKNOWN_OPERATOR"),
        ("filter title.contain", "DEF.UNKNOWN_OPERATOR"),
        // field read 정책 필드는 부분일치로도 값을 추론할 수 있다
        ("filter internalNote.contains", "DEF.POLICY_FIELD_NOT_FILTERABLE"),
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
        ("budget { rows 50; depth 2; deadline 2s; cost 1000; offset 0 }", "DEF.BAD_BUDGET"),
        ("budget { rows 50; depth 2; deadline 2s; cost 1000; offset 10; offset 20 }", "DEF.DUPLICATE"),
    ] {
        let got = codes(&budget_src(b));
        assert!(got.iter().any(|g| g.contains(want)), "`{b}`: 기대 {want}, 실제 {got:?}");
    }
}

/// `spec/definition-syntax.md`(`filter` 행)가 말하는 타입별 연산 표. 문서와 파서가 어긋나면 여기서 깨진다.
#[test]
fn the_documented_operator_matrix_is_what_the_parser_accepts() {
    const OPS: [&str; 10] = ["eq", "in", "isNull", "gte", "lte", "gt", "lt", "prefix", "contains", "like"];
    // (필드 타입, 항상 되는 연산). isNull은 nullable(`?`)일 때만 더해진다.
    let table: &[(&str, &[&str])] = &[
        ("Int(0..9)", &["eq", "gte", "lte", "gt", "lt"]),
        ("Decimal(5,2)", &["eq", "gte", "lte", "gt", "lt"]),
        ("Time", &["eq", "gte", "lte", "gt", "lt"]),
        ("Date", &["eq", "gte", "lte", "gt", "lt"]),
        ("Text", &["eq", "in", "prefix", "contains", "like"]),
        ("Email", &["eq", "in"]),
        ("Url", &["eq"]),
        ("Bool", &["eq"]),
        ("Json", &["eq"]),
        ("St", &["eq", "in"]),
        ("Member", &["eq", "in"]),
    ];
    let accepted = |ty: &str, op: &str| -> bool {
        let src = format!(
            "enum St {{ A, B }}\nresource Member {{ fields {{ id: Id }} rows read when id = actor }}\nactor Member\nresource P {{ fields {{ id: Id; v: {ty}; owner: Member }} rows read when owner = actor\n  expose read {{ select id, v; filter v.{op}; budget {{ rows 10; depth 1; deadline 1s; cost 100 }} }} }}\n"
        );
        load_str(&src, Form::A).is_ok()
    };
    for (ty, ops) in table {
        for nullable in [false, true] {
            let decl = if nullable { format!("{ty}?") } else { (*ty).to_string() };
            for op in OPS {
                let want = ops.contains(&op) || (nullable && op == "isNull");
                assert_eq!(accepted(&decl, op), want, "{decl}.{op}: 문서 표와 파서가 다름");
            }
        }
    }
    // Id·Uuid 기본키는 eq in gte lte gt lt이고 isNull·문자열 연산은 거부한다.
    for id_ty in ["Id", "Uuid"] {
        for op in OPS {
            let src = format!("actor Member\nresource Member {{ fields {{ id: {id_ty} }} rows read when id = actor\n  expose read {{ select id; filter id.{op}; budget {{ rows 10; depth 1; deadline 1s; cost 100 }} }} }}\n");
            let want = ["eq", "in", "gte", "lte", "gt", "lt"].contains(&op);
            assert_eq!(load_str(&src, Form::A).is_ok(), want, "{id_ty} id.{op}");
        }
    }
}

/// `release min|max`는 groupKey가 있고 결과 타입이 `Int?`일 때만 된다(문서의 aggregate `release` 행).
#[test]
fn min_and_max_need_a_group_key_and_a_nullable_int_result() {
    let def = |ty: &str, rel: &str| {
        format!(
            "actor Member\naccess ofMember = totalOfVisible(Member)\nresource Member {{ fields {{ id: Id }} rows read when actor != null\n  aggregate a: {ty} {{ source Item; sourceAccess ofMember; groupKey owner; callerFilter none; rowOutput none; release {rel} }} }}\nresource Item {{ fields {{ id: Id; owner: Member; qty: Int(0..100) }} rows read when owner = actor }}\n"
        )
    };
    for rel in ["min(qty)", "max(qty)"] {
        assert!(load_str(&def("Int?", rel), Form::A).is_ok(), "{rel} as Int?");
        let got = load_str(&def("Int", rel), Form::A).map(|_| ()).unwrap_err();
        assert!(got.iter().any(|d| format!("{d}").contains("DEF.UNSUPPORTED")), "{rel} as Int: {got:?}");
    }
    assert!(load_str(&def("Int", "sum(qty)"), Form::A).is_ok(), "sum stays a plain Int");
}
