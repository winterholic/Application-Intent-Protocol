use spike_v1_fixture::{digest, load_str, Form};

const A: &str = include_str!("../fixture/recruitment.aip");
const ETS: &str = include_str!("../fixture/recruitment.e.ts");
const EPY: &str = include_str!("../fixture/recruitment.e.py");
const HTS: &str = include_str!("../fixture/recruitment.h.ts");
const HPY: &str = include_str!("../fixture/recruitment.h.py");

fn src(f: Form) -> &'static str {
    match f {
        Form::A => A,
        Form::ETs => ETS,
        Form::EPy => EPY,
        Form::HTs => HTS,
        Form::HPy => HPY,
    }
}

fn run(f: Form, s: &str) -> Result<(String, String), Vec<String>> {
    load_str(s, f).map(|o| (digest(&o.execution), digest(&o.metadata))).map_err(|ds| ds.iter().map(|d| format!("{d}")).collect())
}

/// 변형이 실제로 적용됐는지 확인한다. 치환 대상이 없으면 음성 대조가 공허해진다.
fn mutate(f: Form, old: &str, new: &str) -> String {
    let s = src(f);
    assert_eq!(s.matches(old).count(), 1, "{f:?}: 치환 대상 `{old}`가 정확히 한 번 있어야 함");
    s.replacen(old, new, 1)
}

fn base(f: Form) -> (String, String) {
    run(f, src(f)).unwrap_or_else(|e| panic!("{f:?} 기준 fixture 실패: {e:?}"))
}

const ALL: [Form; 5] = [Form::A, Form::ETs, Form::EPy, Form::HTs, Form::HPy];

#[test]
fn eq_all_forms_same_typed_facts() {
    let (e0, m0) = base(Form::A);
    for f in ALL {
        let (e, m) = base(f);
        assert_eq!(e, e0, "{f:?} execution digest");
        assert_eq!(m, m0, "{f:?} metadata digest");
    }
}

#[test]
fn eq_facts_cover_eq01_to_eq11() {
    let o = load_str(A, Form::A).unwrap();
    let r = &o.execution["resources"]["Recruitment"];
    // EQ-01 타입
    assert_eq!(r["fields"]["title"]["ty"], "Text");
    assert_eq!(r["fields"]["title"]["range"], serde_json::json!([1, 100]));
    assert_eq!(r["fields"]["internalNote"]["ty"], "Text?");
    assert_eq!(r["fields"]["club"]["ty"], "Ref<Club>");
    // EQ-02 행 정책
    assert_eq!(r["rowRead"]["and"][0]["call"], "active");
    // EQ-03 독립 허용 목록
    assert_eq!(r["exposeRead"]["filter"], serde_json::json!(["periodEnd.gte", "periodEnd.lte"]));
    assert_eq!(r["exposeRead"]["sort"], serde_json::json!(["id", "periodEnd", "views"]));
    // EQ-04 관계 + 대상 정책 재적용
    assert_eq!(r["exposeRead"]["traverse"]["club"]["reapply"], serde_json::json!(["rowRead", "fieldRead"]));
    assert_eq!(o.execution["resources"]["Club"]["exposeRead"]["rootQueryable"], false);
    // EQ-05 내부 메모 필드 정책
    assert_eq!(r["exposeRead"]["select"]["internalNote"], "fieldWithPolicy");
    assert_eq!(r["fieldRead"]["internalNote"]["call"], "managerOf");
    // EQ-06 북마크 개인 행 vs 총수 집계
    assert_eq!(o.execution["resources"]["RecruitmentBookmark"]["rowRead"]["cmp"], "=");
    assert_eq!(r["aggregates"]["bookmarkCount"]["sourceAccess"]["ref"], "fixedTotalOfVisibleRecruitment");
    assert_eq!(r["aggregates"]["bookmarkCount"]["rowOutput"], "none");
    // EQ-07 비용 제한
    assert_eq!(r["exposeRead"]["budget"]["deadlineMs"], 2000);
    // EQ-08 전이
    assert_eq!(r["transitions"]["close"]["to"]["status"]["enum"], "RecruitmentStatus.CLOSED");
    // EQ-09 불변식 DB 집행
    assert_eq!(r["invariants"]["atMostOnePublished"]["enforcement"]["kind"], "partialUniqueIndex");
    // EQ-10 확장 계약
    assert_eq!(r["extensions"]["stats"]["access"]["Apply.approvedCount"]["clubId"], "input.clubId");
    assert_eq!(r["extensions"]["stats"]["effect"], "none");
    // EQ-11 설명은 실행 사실 밖
    assert!(r.get("docs").is_none());
    assert_eq!(o.metadata["resource:Recruitment"]["visibility"], "internal");
    // 행 정책 누락은 전부 거부
    assert_eq!(o.execution["resources"]["School"]["rowRead"]["default"], "denyAll");
}

#[test]
fn sensitivity_changes_change_execution_digest() {
    let cases: &[(Form, &str, &str)] = &[
        (Form::A, "rows 50;", "rows 51;"),
        (Form::A, "sort periodEnd, views, id", "sort periodEnd, id"),
        (Form::A, "or club.school = actor.school)", "and club.school = actor.school)"),
        (Form::ETs, "field internalNote read when managerOf(actor, club)", "field title read when managerOf(actor, club)"),
        (Form::EPy, "atMost 1 where status = PUBLISHED", "atMost 1 where status = CLOSED"),
        (Form::HTs, "role in (ADMIN, MANAGER)", "role in (ADMIN)"),
        (Form::HPy, "\"rowOutput\": \"none\", \"release\": \"count\",\n                },\n            },\n            \"transitions\"", "\"rowOutput\": \"none\", \"release\": \"count\",\n                },\n            },\n            \"invariants_\": [],\n            \"transitions\""),
    ];
    let (e0, _) = base(Form::A);
    for (f, old, new) in cases {
        match run(*f, &mutate(*f, old, new)) {
            Ok((e, _)) => assert_ne!(e, e0, "{f:?} `{old}`→`{new}` 이 digest를 바꾸지 않음"),
            // 모르는 키를 넣은 변형은 digest 변화 대신 거부가 기대 결과다.
            Err(ds) => assert!(ds.iter().any(|d| d.contains("UNKNOWN_KEY")), "{f:?} 예상 밖 실패 {ds:?}"),
        }
    }
}

#[test]
fn invariance_whitespace_comments_and_allowlist_order() {
    let (e0, m0) = base(Form::A);
    let spaced = A.replace("\n", "\n\n   // 주석\n").replace("; ", " ;  ");
    assert_eq!(run(Form::A, &spaced).unwrap(), (e0.clone(), m0.clone()));
    let reordered = mutate(Form::A, "sort periodEnd, views, id", "sort id, views, periodEnd");
    assert_eq!(run(Form::A, &reordered).unwrap().0, e0);
    let keys = mutate(
        Form::HTs,
        "callerFilter: \"none\", rowOutput: \"none\", release: \"count\",\n        },\n      },\n      transitions",
        "release: \"count\", rowOutput: \"none\", callerFilter: \"none\",\n        },\n      },\n      transitions",
    );
    assert_eq!(run(Form::HTs, &keys).unwrap().0, e0);
}

#[test]
fn docs_change_or_removal_keeps_execution_digest() {
    for f in ALL {
        let (e0, m0) = base(f);
        let (old, new_summary, removed) = match f {
            Form::A | Form::ETs | Form::EPy => {
                ("docs { summary \"모집 정보\"; visibility internal }", "docs { summary \"채용 공고 목록\"; visibility internal }", "")
            }
            Form::HTs => (
                "docs: { summary: \"모집 정보\", visibility: \"internal\" },",
                "docs: { summary: \"채용 공고 목록\", visibility: \"internal\" },",
                "",
            ),
            Form::HPy => (
                "\"docs\": {\"summary\": \"모집 정보\", \"visibility\": \"internal\"},",
                "\"docs\": {\"summary\": \"채용 공고 목록\", \"visibility\": \"internal\"},",
                "",
            ),
        };
        let (e1, m1) = run(f, &mutate(f, old, new_summary)).unwrap();
        assert_eq!(e1, e0, "{f:?} summary 변경이 실행 digest를 바꿈");
        assert_ne!(m1, m0, "{f:?} summary 변경이 metadata에 반영 안 됨");
        let (e2, m2) = run(f, &mutate(f, old, removed)).unwrap();
        assert_eq!(e2, e0, "{f:?} docs 삭제가 실행 digest를 바꿈");
        assert_ne!(m2, m0);
    }
}

#[test]
fn negative_controls_reject_with_expected_code() {
    let cases: &[(Form, &str, &str, &str)] = &[
        // 잘못된 기호·타입
        (Form::A, "select id, title,", "select id, titel,", "UNKNOWN_FIELD"),
        (Form::A, "or club.school = actor.school)", "or club.school = actor.schol)", "UNKNOWN_FIELD"),
        (Form::A, "limit atMostOnePublished on Recruitment = atMost 1 where status = PUBLISHED", "", "UNKNOWN_SYMBOL"),
        (Form::A, "club.school = null or club.school", "club.school = 3 or club.school", "TYPE_MISMATCH"),
        (Form::A, "from status = PUBLISHED", "from title = null", "NULL_COMPARE_NON_NULLABLE"),
        (Form::A, "filter periodEnd.gte", "filter title.gte", "OP_TYPE_MISMATCH"),
        (Form::A, "periodEnd.lte", "periodEnd.like", "UNKNOWN_OPERATOR"),
        (Form::A, "traverse club { select id, name, logo }", "traverse club { select id, name, school }", "TRAVERSE_NOT_EXPOSED"),
        (Form::A, "allow managerOf(actor, club)", "allow managerof(actor, club)", "UNKNOWN_SYMBOL"),
        (
            Form::A,
            "field internalNote read when managerOf(actor, club)",
            "field internalNote read when managerOf(actor, club.school)",
            "TYPE_MISMATCH",
        ),
        (
            Form::A,
            "groupKey recruitment\n    callerFilter none\n    rowOutput none",
            "groupKey recruitment\n    callerFilter none\n    rowOutput rows",
            "UNSUPPORTED",
        ),
        (Form::A, "sourceAccess fixedTotalOfVisibleRecruitment\n", "", "MISSING_ITEM"),
        (Form::A, "groupKey recruitment", "groupKey member", "GROUP_KEY_MISMATCH"),
        (Form::A, "access Apply.approvedCount", "access Apply.approvedTotal", "UNKNOWN_SYMBOL"),
        (Form::A, "input { clubId: Club.Id }\n    output", "input { club: Club.Id }\n    output", "EXT_INPUT_UNBOUND"),
        (Form::A, "effect none", "effect mail", "EFFECT_MISMATCH"),
        (Form::A, "depth 2;", "depth 1;", "BUDGET_DEPTH_TOO_SMALL"),
        (Form::A, "budget { rows 50; depth 2; deadline 2s; cost 1000 }", "budget { rows 50; depth 2; cost 1000 }", "MISSING_ITEM"),
        (Form::A, "deadline 2s;", "deadline 2h;", "LEX_BAD_DURATION"),
        (Form::A, "invariant atMostOnePublished per club", "invariant atMostOnePublished per title", "UNKNOWN_FIELD"),
        (
            Form::A,
            "  expose aggregate approvedCount\n",
            "  expose aggregate approvedCount\n  expose read { select id, approvedCount; budget { rows 1; depth 1; deadline 1s; cost 1 } }\n",
            "AGG_NOT_SELECTABLE",
        ),
        (
            Form::A,
            "predicate managerOf(m: Member, c: Club) = exists ClubMember where club = c and member = m",
            "predicate managerOf(member: Member, c: Club) = exists ClubMember where club = c and member = member",
            "AMBIGUOUS_NAME",
        ),
        (Form::A, "status = APPROVE", "status = APPROVED", "UNKNOWN_SYMBOL"),
        (Form::A, "docs { summary", "doc { summary", "PARSE_UNKNOWN_KEY"),
        (Form::A, "logo: Url?", "logo: Uri?", "UNKNOWN_TYPE"),
        (Form::A, "enum ClubRole", "enum ApplyStatus", "DUPLICATE"),
        // E: 정적 리터럴만
        (Form::ETs, "summary \"모집 정보\"", "summary \"${x}\"", "DYNAMIC_INTERPOLATION"),
        (Form::ETs, "export default aip`", "export default aip(`", "NON_LITERAL"),
        (Form::ETs, "expose aggregate approvedCount\n  }\n`", "expose aggregate approvedCount\n  }\n` + extra", "NON_LITERAL"),
        (Form::ETs, "import { aip } from \"@aip/define\"\n", "import { aip } from \"@aip/define\"\nconst a = aip\n", "NON_LITERAL"),
        (Form::ETs, "summary \"모집 정보\"", "summary \"모집\\n정보\"", "LEX_BAD_STRING"),
        (
            Form::ETs,
            "import { aip } from \"@aip/define\"\n",
            "import { aip } from \"@aip/define\"\nexport const x = aip`enum E { A }`\n",
            "MULTIPLE_BLOCKS",
        ),
        (Form::EPy, "SPEC = aip(\"\"\"", "SPEC = aip(f\"\"\"", "DYNAMIC_INTERPOLATION"),
        (Form::EPy, "\n\"\"\")", "\n\"\"\" + EXTRA)", "NON_LITERAL"),
        (Form::EPy, "\n\"\"\")", "\n\"\"\".format(x))", "NON_LITERAL"),
        (Form::EPy, "from aip.define import aip\n", "from aip.define import aip\nA = aip\n", "NON_LITERAL"),
        (Form::EPy, "status = APPROVE", "status = APPROVED", "UNKNOWN_SYMBOL"),
        // H: 리터럴 부분집합 + 같은 의미 검사
        (Form::HTs, "\"recruitment.stats\"", "IMPL", "NON_LITERAL"),
        (Form::HTs, "  resources: {\n", "  resources: {\n    ...base,\n", "NON_LITERAL"),
        (Form::HTs, "summary: \"모집 정보\"", "summary: `모집 정보`", "NON_LITERAL"),
        (
            Form::HTs,
            "fields: { recruitment: \"Recruitment\", member: \"Member\" },\n      rows: \"member = actor\",",
            "fields: { recruitment: \"Recruitment\", member: \"Member\" },\n      rows: (a) => a,",
            "NON_LITERAL",
        ),
        (Form::HTs, "fieldRead:", "fieldReads:", "UNKNOWN_KEY"),
        (Form::HTs, "\"managerOf(actor, recruitment.club)\"", "\"managerOf(actor, recruitment.clubb)\"", "UNKNOWN_FIELD"),
        (Form::HTs, "  actor: \"Member\",\n", "  actor: \"Member\",\n  [k]: 1,\n", "NON_LITERAL"),
        (Form::HTs, "atMost: 1,", "atMost: 1.5,", "NON_LITERAL"),
        (Form::HPy, "\"summary\": \"모집 정보\"", "\"summary\": f\"모집 {x}\"", "DYNAMIC_INTERPOLATION"),
        (Form::HPy, "\"recruitment.stats\"", "IMPL", "NON_LITERAL"),
        (Form::HPy, "\"fieldRead\":", "\"field_read\":", "UNKNOWN_KEY"),
        (Form::HPy, "\"atMost\": 1,", "\"atMost\": True,", "H_SHAPE"),
    ];
    let mut failures = vec![];
    for (f, old, new, code) in cases {
        match run(*f, &mutate(*f, old, new)) {
            Ok(_) => failures.push(format!("{f:?} `{old}`→`{new}`: 통과해버림, 기대 {code}")),
            Err(ds) if !ds.iter().any(|d| d.contains(code)) => failures.push(format!("{f:?} `{old}`→`{new}`: 기대 {code}, 실제 {ds:?}")),
            Err(_) => {}
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    eprintln!("negative controls: {} cases", cases.len());
}

#[test]
fn e_diagnostics_point_into_host_file() {
    let s = mutate(Form::ETs, "select id, title,", "select id, titel,");
    let err = run(Form::ETs, &s).unwrap_err();
    let host_line = s.lines().position(|l| l.contains("titel")).unwrap() + 1;
    assert!(err[0].starts_with(&format!("{host_line}:")), "진단 줄 {err:?} vs 호스트 줄 {host_line}");
}
