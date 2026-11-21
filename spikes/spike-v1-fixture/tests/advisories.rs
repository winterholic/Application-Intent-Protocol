use spike_v1_fixture::{load_str, Form};

const OPEN_AGGREGATE: &str = r#"
actor Member
access allMembers(a: Member) = true
resource Member {
  fields { id: Uuid; owner: Member; name: Text(1..100) }
  rows read when owner = actor
  aggregate nameCount: Int {
    source Member
    sourceAccess allMembers(actor)
    callerFilter read
    rowOutput none
    release count
  }
  expose aggregate nameCount
}
"#;

fn codes(src: &str) -> Vec<&'static str> {
    match load_str(src, Form::A) {
        Ok(checked) => checked.advisories.iter().map(|d| d.code).collect(),
        Err(ds) => ds.iter().map(|d| d.code).collect(),
    }
}

fn is_rejected(src: &str) -> bool {
    load_str(src, Form::A).is_err()
}

#[test]
fn caller_filter_read_without_where_is_an_error_when_source_has_row_policy() {
    // 감사 §6.2: 수정 전에는 통과해 숨은 행의 필드 값을 filter로 셀 수 있었다.
    assert_eq!(codes(OPEN_AGGREGATE), ["DEF.CALLER_FILTER_POLICY_REQUIRED"]);
}

#[test]
fn a_where_that_implies_the_row_policy_closes_the_oracle() {
    let same = OPEN_AGGREGATE.replace("    callerFilter read", "    where owner = actor\n    callerFilter read");
    assert!(codes(&same).is_empty(), "정책과 같은 where는 오류도 경고도 없다: {:?}", codes(&same));

    let narrower = OPEN_AGGREGATE.replace("    callerFilter read", "    where owner = actor and name = \"x\"\n    callerFilter read");
    assert!(!is_rejected(&narrower), "정책을 AND 항으로 포함한 더 좁은 where는 통과");

    let reordered = OPEN_AGGREGATE.replace("    callerFilter read", "    where name = \"x\" and owner = actor\n    callerFilter read");
    assert!(!is_rejected(&reordered), "AND 항 순서는 상관없다");
}

#[test]
fn a_where_that_does_not_imply_the_row_policy_is_rejected() {
    for wrong in ["where name = \"x\"", "where owner = actor or name = \"x\"", "where true", "where owner != actor"] {
        let src = OPEN_AGGREGATE.replace("    callerFilter read", &format!("    {wrong}\n    callerFilter read"));
        assert_eq!(codes(&src), ["DEF.CALLER_FILTER_POLICY_REQUIRED"], "{wrong}");
    }
}

#[test]
fn one_branch_of_an_or_policy_is_enough_for_where() {
    let admin = r#"
actor Member
predicate isAdmin(a: Member) = a.admin = true
access adminOnly(a: Member) = isAdmin(a)
resource Member {
  fields { id: Uuid; admin: Bool; name: Text(1..100) }
  rows read when id = actor or isAdmin(actor)
  aggregate total: Int {
    source Member
    sourceAccess adminOnly(actor)
    where isAdmin(actor)
    callerFilter read
    rowOutput none
    release count
  }
  expose aggregate total
}
"#;
    assert!(codes(admin).is_empty(), "{:?}", codes(admin));
    let other_branch = admin.replace("where isAdmin(actor)", "where id = actor");
    assert!(codes(&other_branch).is_empty(), "다른 OR 가지도 정책을 함의한다");
    let wrong = admin.replace("where isAdmin(actor)", "where name = \"x\"");
    assert_eq!(codes(&wrong), ["DEF.CALLER_FILTER_POLICY_REQUIRED"]);
}

#[test]
fn caller_filter_read_is_free_when_the_source_has_no_row_policy_or_the_filter_is_off() {
    let no_policy = OPEN_AGGREGATE.replace("  rows read when owner = actor\n", "");
    assert!(!is_rejected(&no_policy), "원본에 rows read가 없으면 새 오류의 대상이 아니다");
    let none = OPEN_AGGREGATE.replace("callerFilter read", "callerFilter none");
    assert!(!is_rejected(&none));
}

#[test]
fn a_count_without_where_reports_the_public_count_advisory_and_keeps_the_facts() {
    let none = OPEN_AGGREGATE.replace("callerFilter read", "callerFilter none");
    let checked = load_str(&none, Form::A).expect("공개 count는 오류가 아니다");
    let advisories: Vec<_> = checked.advisories.iter().map(|d| d.code).collect();
    assert_eq!(advisories, ["DEF.ADVISORY_PUBLIC_COUNT"]);
    let message = &checked.advisories[0].msg;
    assert!(message.contains("nameCount") && message.contains("rows read"), "{message}");
    // 응답 의미는 그대로다: facts의 where는 null이고 count는 원본 정책 없이 센다.
    assert!(checked.execution["resources"]["Member"]["aggregates"]["nameCount"]["where"].is_null());

    let scoped = none.replace("    callerFilter none", "    where owner = actor\n    callerFilter none");
    assert!(codes(&scoped).is_empty(), "where가 있으면 공개 count 경고가 없다");
}

#[test]
fn explicit_where_and_facereview_have_no_error_and_only_the_documented_advisories() {
    let facereview = include_str!("../../../product/tests/fixtures/facereview-mysql/facereview.aip");
    let checked = load_str(facereview, Form::A).expect("FaceReview definition");
    let anchors: Vec<_> = checked.advisories.iter().map(|d| (d.code, d.msg.split('`').nth(1).unwrap_or(""))).collect();
    // 뒤의 둘은 P1 정책 작성 경고다(p1-sema-notes.md 분류): 관리자가 `is_deleted`를 정해 만든 Video는 자기에게 안 보일 수 있고,
    // Comment는 `is_deleted`를 채우는 값이 정의에 없다. 둘 다 오류가 아니다.
    assert_eq!(
        anchors,
        [
            ("DEF.ADVISORY_PUBLIC_COUNT", "watchCount"),
            ("DEF.ADVISORY_PUBLIC_COUNT", "commentCount"),
            ("DEF.ADVISORY_PUBLIC_COUNT", "likeCount"),
            ("DEF.ADVISORY_WRITE_WIDER_THAN_READ", "Video"),
            ("DEF.ADVISORY_WRITE_WIDER_THAN_READ", "Comment"),
        ]
    );
}

#[test]
fn unsupported_forms_name_what_is_allowed_without_mentioning_spike() {
    let bad_filter = OPEN_AGGREGATE.replace("callerFilter read", "callerFilter bad");
    let bad_row_output = OPEN_AGGREGATE.replace("rowOutput none", "rowOutput rows");
    for src in [bad_filter, bad_row_output] {
        let ds = load_str(&src, Form::A).err().expect("rejected");
        let message = ds.iter().map(|d| d.msg.clone()).collect::<Vec<_>>().join("; ");
        assert!(message.contains("지원하지 않음") && message.contains("허용"), "{message}");
        assert!(!message.to_lowercase().contains("spike"), "{message}");
    }
}
