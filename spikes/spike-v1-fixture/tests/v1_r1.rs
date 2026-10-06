//! Codex V1 r1 검토(plan-docs/reviews/codex-v1-r1.md) 반례 재발 방지와 독립 기대 facts 대조.
use serde_json::{json, Value};
use spike_v1_fixture::{digest, load_str, Form};

const A: &str = include_str!("../fixture/recruitment.aip");
const ETS: &str = include_str!("../fixture/recruitment.e.ts");
const EPY: &str = include_str!("../fixture/recruitment.e.py");
const HTS: &str = include_str!("../fixture/recruitment.h.ts");
const HPY: &str = include_str!("../fixture/recruitment.h.py");

fn sub(s: &str, old: &str, new: &str) -> String {
    assert_eq!(s.matches(old).count(), 1, "치환 대상 `{old}`가 정확히 한 번 있어야 함");
    s.replacen(old, new, 1)
}
fn exec(s: &str, f: Form) -> Result<String, Vec<String>> {
    load_str(s, f).map(|o| digest(&o.execution)).map_err(|ds| ds.iter().map(|d| d.to_string()).collect())
}
fn base() -> String {
    exec(A, Form::A).unwrap()
}
fn rejects(s: &str, f: Form, code: &str) -> Option<String> {
    match exec(s, f) {
        Ok(_) => Some(format!("{f:?}: 통과해버림, 기대 {code}")),
        Err(ds) if !ds.iter().any(|d| d.contains(code)) => Some(format!("{f:?}: 기대 {code}, 실제 {ds:?}")),
        Err(_) => None,
    }
}

#[test]
fn r1_extraction_boundaries() {
    let b = base();
    let mut fails = vec![];
    let mut want = |name: &str, r: Option<String>| {
        if let Some(e) = r {
            fails.push(format!("{name}: {e}"));
        }
    };
    // F01 주석·문자열 안 블록은 선언이 아님
    want("F01 TS 주석 안", rejects(&format!("/*\naip`{A}`\n*/\n"), Form::ETs, "NO_BLOCK"));
    want("F01 Py 문자열 안", rejects(&format!("TEXT = '''\naip(\"\"\"{A}\"\"\")\n'''\n"), Form::EPy, "NO_BLOCK"));
    want("F07 H-Py 문자열 안", rejects(&format!("X = '''\n{HPY}\n'''\n"), Form::HPy, "NO_BLOCK"));
    // F02 공식 import·선언 위치·두 번째 블록
    want("F02 TS 다른 패키지", rejects(&sub(ETS, "@aip/define", "evil-package"), Form::ETs, "WRONG_BINDING"));
    want("F02 Py 다른 바인딩", rejects(&sub(EPY, "from aip.define import aip", "from evil_package import other as aip"), Form::EPy, "WRONG_BINDING"));
    want("F02 TS 숨은 두 번째", rejects(&format!("{ETS}\nimport \"x\"; const second = aip`enum E {{ A }}`\n"), Form::ETs, "NON_LITERAL"));
    want("F02 TS export 두 번째", rejects(&format!("{ETS}\nexport const second = aip`enum E {{ A }}`\n"), Form::ETs, "MULTIPLE_BLOCKS"));
    want("F02 TS 별칭 import", rejects(&sub(ETS, "import { aip } from", "import { aip as a } from"), Form::ETs, "WRONG_BINDING"));
    want("F02 H 다른 패키지", rejects(&sub(HTS, "@aip/define", "evil-package"), Form::HTs, "WRONG_BINDING"));
    // F03·F07 다음 줄 가공
    let close = "expose aggregate approvedCount\n  }\n`";
    want("F03 TS 다음 줄 + 결합", rejects(&sub(ETS, close, &format!("{close}\n+ extra")), Form::ETs, "NON_LITERAL"));
    want("F03 TS 다음 줄 멤버", rejects(&sub(ETS, close, &format!("{close}\n.trim()")), Form::ETs, "NON_LITERAL"));
    want("F03 Py 줄 연결", rejects(&sub(EPY, "\n\"\"\")", "\n\"\"\") \\\n.strip()"), Form::EPy, "NON_LITERAL"));
    want("F07 H 호출 뒤 호출", rejects(&sub(HTS, "  },\n})", "  },\n})(execute())"), Form::HTs, "NON_LITERAL"));
    want("F07 H 다음 줄 호출", rejects(&sub(HTS, "  },\n})", "  },\n})\n(execute())"), Form::HTs, "NON_LITERAL"));
    // 양성 대조: 정상 호스트 코드의 문자열·주석·멤버 이름에 같은 단어가 있어도 통과
    let ok_ts = sub(
        ETS,
        "import { aip } from \"@aip/define\"\n",
        "import { aip } from \"@aip/define\"\nconst note = \"aip\" // aip 설명\nconst o = { x: 1 }.aip\n",
    );
    if exec(&ok_ts, Form::ETs).as_ref() != Ok(&b) {
        fails.push(format!("양성 TS 문자열/주석: {:?}", exec(&ok_ts, Form::ETs)));
    }
    let ok_py = sub(EPY, "from aip.define import aip\n", "from aip.define import aip\nNOTE = \"aip\"  # aip 설명\n");
    if exec(&ok_py, Form::EPy).as_ref() != Ok(&b) {
        fails.push(format!("양성 Py 문자열/주석: {:?}", exec(&ok_py, Form::EPy)));
    }
    let ok_h =
        sub(HTS, "import { define } from \"@aip/define\"\n", "import { define } from \"@aip/define\"\n/* define 설명 */ const s = \"define\";\n");
    if exec(&ok_h, Form::HTs).as_ref() != Ok(&b) {
        fails.push(format!("양성 H 주석/문자열: {:?}", exec(&ok_h, Form::HTs)));
    }
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}

#[test]
fn r1_contract_loss_and_validation() {
    let b = base();
    let mut fails = vec![];
    // F04 입출력 범위가 facts에 남음
    let r1 = exec(&sub(A, "output { approvedApplicants: Int }", "output { approvedApplicants: Int(0..10) }"), Form::A).unwrap();
    let r2 = exec(&sub(A, "output { approvedApplicants: Int }", "output { approvedApplicants: Int(0..999) }"), Form::A).unwrap();
    if r1 == r2 || r1 == b {
        fails.push("F04 출력 범위가 digest에 반영 안 됨".to_string());
    }
    let cases: &[(&str, &str, &str, &str)] = &[
        // F06 H totalOfVisible에 params
        (
            "F06",
            "fixedTotalOfVisibleRecruitment: { totalOfVisible: \"Recruitment\" }",
            "fixedTotalOfVisibleRecruitment: { totalOfVisible: \"Recruitment\", params: 123 }",
            "H_SHAPE",
        ),
    ];
    for (n, old, new, code) in cases {
        if let Some(e) = rejects(&sub(HTS, old, new), Form::HTs, code) {
            fails.push(format!("{n}: {e}"));
        }
    }
    let a_cases: &[(&str, &str, &str, &str)] = &[
        // F09 중복은 덮어쓰지 않고 거부
        ("F09 필드 정책", "  field internalNote read when", "  field internalNote read when views = 0\n  field internalNote read when", "DUPLICATE"),
        ("F09 from", "from status = PUBLISHED", "from status = CLOSED\n    from status = PUBLISHED", "DUPLICATE"),
        ("F09 budget", "budget { rows 50;", "budget { rows 1; rows 50;", "DUPLICATE"),
        ("F09 aggregate 키", "    groupKey recruitment\n", "    groupKey recruitment\n    groupKey recruitment\n", "DUPLICATE"),
        ("F09 transition 이름", "  invariant atMostOnePublished per club", "  transition close { from status = DRAFT; to status = PUBLISHED; allow managerOf(actor, club) }\n  invariant atMostOnePublished per club", "DUPLICATE"),
        ("F09 집계/필드 이름 충돌", "aggregate bookmarkCount: Int {", "aggregate views: Int { source RecruitmentBookmark; sourceAccess fixedTotalOfVisibleRecruitment; groupKey recruitment; callerFilter none; rowOutput none; release count }\n  aggregate bookmarkCount: Int {", "DUPLICATE"),
        ("F09 select 중복", "select id, title,", "select id, id, title,", "DUPLICATE"),
        ("F09 docs 키", "docs { summary \"모집 정보\";", "docs { summary \"a\"; summary \"모집 정보\";", "DUPLICATE"),
        // F10 타입·범위·budget
        ("F10 nullable 대입", "to status = CLOSED", "to title = internalNote", "TYPE_MISMATCH"),
        ("F10 역전 범위", "Text(1..100)", "Text(100..1)", "BAD_RANGE"),
        ("F10 budget 0", "rows 50;", "rows 0;", "BAD_BUDGET"),
        // F11 정책 순환
        ("F11 자기 재귀", "predicate active(r: Recruitment) = r.status = PUBLISHED and r.periodEnd >= now", "predicate active(r: Recruitment) = active(r)", "POLICY_CYCLE"),
    ];
    for (n, old, new, code) in a_cases {
        if let Some(e) = rejects(&sub(A, old, new), Form::A, code) {
            fails.push(format!("{n}: {e}"));
        }
    }
    // F12 시간 의존 조건은 부분 인덱스로 집행할 수 없고 V3도 집행하지 않으므로 check에서 거부한다(sema_fixes.rs 참고).
    if let Some(e) = rejects(
        &sub(A, "atMost 1 where status = PUBLISHED", "atMost 1 where status = PUBLISHED and periodEnd >= now"),
        Form::A,
        "UNSUPPORTED_INVARIANT",
    ) {
        fails.push(format!("F12 now 조건 불변식: {e}"));
    }
    // F05 같은 의미는 같은 facts
    if exec(&sub(A, "role in (ADMIN, MANAGER)", "role in (MANAGER, ADMIN)"), Form::A).as_ref() != Ok(&b) {
        fails.push("F05 in 순서".into());
    }
    if exec(&sub(A, "club = c and member = m", "club.id = c.id and member.id = m.id"), Form::A).as_ref() != Ok(&b) {
        fails.push("F05 x.id ≡ x".into());
    }
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
}

#[test]
fn r1_source_columns_point_into_host_file() {
    let s = sub(HTS, "\"managerOf(actor, recruitment.club)\"", "\"managerOf(actor, recruitment.clubb)\"");
    let err = exec(&s, Form::HTs).unwrap_err();
    let (ln, line) = s.lines().enumerate().find(|(_, l)| l.contains("recruitment.clubb")).unwrap();
    let col = line[..line.find("recruitment.clubb").unwrap()].chars().count() + 1;
    assert!(err[0].starts_with(&format!("{}:{col} ", ln + 1)), "H 진단 {err:?}, 기대 {}:{col}", ln + 1);

    let s = sub(EPY, "select id, title,", "select id, titel,");
    let err = exec(&s, Form::EPy).unwrap_err();
    let (ln, line) = s.lines().enumerate().find(|(_, l)| l.contains("titel")).unwrap();
    let col = line[..line.find("titel").unwrap()].chars().count() + 1;
    assert!(err[0].starts_with(&format!("{}:{col} ", ln + 1)), "E-Py 진단 {err:?}, 기대 {}:{col}", ln + 1);
}

#[test]
fn r1_cli_rejects_unknown_args() {
    let bin = env!("CARGO_BIN_EXE_spike-v1-fixture");
    let fx = concat!(env!("CARGO_MANIFEST_DIR"), "/fixture/recruitment.aip");
    let st = std::process::Command::new(bin).args(["check", fx, "--optional-checks=off"]).output().unwrap();
    assert_eq!(st.status.code(), Some(2));
    let st = std::process::Command::new(bin).args(["chek", fx]).output().unwrap();
    assert_eq!(st.status.code(), Some(2));
}

/// sema 출력을 복사하지 않고 C B.1 문장에서 손으로 옮긴 기대 facts. 공유 sema끼리의 일치만으로 못 잡는 누락을 본다(F16-5).
#[test]
fn r1_independent_expected_facts() {
    let o = load_str(A, Form::A).unwrap().execution;
    let rr = &o["resources"]["Recruitment"];
    let path = |root: &str, segs: &[&str], ty: &str| json!({ "path": { "root": root, "segs": segs }, "ty": ty });
    let status_published =
        json!({ "cmp": "=", "l": path("this", &["status"], "Enum<RecruitmentStatus>"), "r": { "enum": "RecruitmentStatus.PUBLISHED" } });
    let expected: Vec<(&str, &Value, Value)> = vec![
        (
            "Recruitment.fields",
            &rr["fields"],
            json!({
                "id": { "ty": "Id<Recruitment>", "range": null }, "title": { "ty": "Text", "range": [1, 100] },
                "periodEnd": { "ty": "Time", "range": null }, "status": { "ty": "Enum<RecruitmentStatus>", "range": null },
                "views": { "ty": "Int", "range": null }, "club": { "ty": "Ref<Club>", "range": null },
                "internalNote": { "ty": "Text?", "range": null }
            }),
        ),
        (
            "Recruitment.rowRead",
            &rr["rowRead"],
            json!({ "and": [
            { "call": "active", "args": [path("this", &[], "Ref<Recruitment>")] },
            { "or": [
                { "cmp": "=", "l": path("this", &["club", "school"], "Ref<School>?"), "r": { "lit": null } },
                { "cmp": "=", "l": path("this", &["club", "school"], "Ref<School>?"), "r": path("actor", &["school"], "Ref<School>?") }
            ] }
        ] }),
        ),
        (
            "Recruitment.exposeRead",
            &rr["exposeRead"],
            json!({
                "select": { "id": "field", "title": "field", "periodEnd": "field", "views": "field", "bookmarkCount": "aggregate", "internalNote": "fieldWithPolicy" },
                "filter": ["periodEnd.gte", "periodEnd.lte"], "sort": ["id", "periodEnd", "views"],
                "traverse": { "club": { "target": "Club", "select": ["id", "logo", "name"], "reapply": ["rowRead", "fieldRead"] } },
                "budget": { "rows": 50, "depth": 2, "deadlineMs": 2000, "cost": 1000 }, "rootQueryable": true
            }),
        ),
        (
            "Recruitment.aggregates",
            &rr["aggregates"],
            json!({ "bookmarkCount": {
            "ty": "Int", "tyRange": null, "input": [], "source": "RecruitmentBookmark",
            "sourceAccess": { "ref": "fixedTotalOfVisibleRecruitment" }, "groupKey": "recruitment", "where": null,
            "callerFilter": "none", "rowOutput": "none", "release": "count"
        } }),
        ),
        (
            "Recruitment.transitions",
            &rr["transitions"],
            json!({ "close": {
            "from": status_published, "to": { "status": { "enum": "RecruitmentStatus.CLOSED" } },
            "allow": { "call": "managerOf", "args": [path("actor", &[], "Ref<Member>"), path("this", &["club"], "Ref<Club>")] },
            "repeat": "reject", "effects": []
        } }),
        ),
        (
            "Recruitment.invariants",
            &rr["invariants"],
            json!({ "atMostOnePublished": {
            "per": "club", "enforcement": { "kind": "partialUniqueIndex", "columns": ["club"], "where": status_published, "deferred": false }
        } }),
        ),
        (
            "Recruitment.extensions",
            &rr["extensions"],
            json!({ "stats": {
            "kind": "read", "input": [["clubId", "Id<Club>", null]], "output": [["approvedApplicants", "Int", null]],
            "access": { "Apply.approvedCount": { "clubId": "input.clubId" } }, "effect": "none",
            "deadlineMs": 2000, "implementation": "recruitment.stats"
        } }),
        ),
        (
            "Apply.aggregates",
            &o["resources"]["Apply"]["aggregates"],
            json!({ "approvedCount": {
            "ty": "Int", "tyRange": null, "input": [["clubId", "Id<Club>", null]], "source": "Apply",
            "sourceAccess": { "ref": "clubManagerOnly", "args": [path("actor", &[], "Ref<Member>"), path("input.clubId", &[], "Id<Club>")] },
            "groupKey": null,
            "where": { "and": [
                { "cmp": "=", "l": path("this", &["recruitment", "club"], "Ref<Club>"), "r": path("input.clubId", &[], "Id<Club>") },
                { "cmp": "=", "l": path("this", &["status"], "Enum<ApplyStatus>"), "r": { "enum": "ApplyStatus.APPROVE" } }
            ] },
            "callerFilter": "none", "rowOutput": "none", "release": "count"
        } }),
        ),
        (
            "predicates.managerOf",
            &o["predicates"]["managerOf"],
            json!({
                "params": [["m", "Ref<Member>", null], ["c", "Ref<Club>", null]],
                "body": { "exists": "ClubMember", "evaluatedAs": "serverPolicy", "where": { "and": [
                    { "cmp": "=", "l": path("this", &["club"], "Ref<Club>"), "r": path("var.c", &[], "Ref<Club>") },
                    { "cmp": "=", "l": path("this", &["member"], "Ref<Member>"), "r": path("var.m", &[], "Ref<Member>") },
                    { "in": path("this", &["role"], "Enum<ClubRole>"), "items": [{ "enum": "ClubRole.ADMIN" }, { "enum": "ClubRole.MANAGER" }] }
                ] } }
            }),
        ),
        (
            "Club.exposeRead",
            &o["resources"]["Club"]["exposeRead"],
            json!({
                "select": { "id": "field", "name": "field", "logo": "field" }, "filter": [], "sort": [], "traverse": {},
                "budget": null, "rootQueryable": false
            }),
        ),
        ("School.rowRead", &o["resources"]["School"]["rowRead"], json!({ "default": "denyAll" })),
        (
            "RecruitmentBookmark.rowRead",
            &o["resources"]["RecruitmentBookmark"]["rowRead"],
            json!({ "cmp": "=", "l": path("this", &["member"], "Ref<Member>"), "r": path("actor", &[], "Ref<Member>") }),
        ),
    ];
    let fails: Vec<String> =
        expected.iter().filter(|(_, got, want)| *got != want).map(|(n, got, want)| format!("{n}\n  기대 {want}\n  실제 {got}")).collect();
    assert!(fails.is_empty(), "\n{}", fails.join("\n"));
    for f in [Form::ETs, Form::EPy, Form::HTs, Form::HPy] {
        let src = match f {
            Form::ETs => ETS,
            Form::EPy => EPY,
            Form::HTs => HTS,
            _ => HPY,
        };
        assert_eq!(load_str(src, f).unwrap().execution, o, "{f:?} 전체 facts");
    }
}
