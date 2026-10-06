//! Codex r2(plan-docs/reviews/codex-v1v2-r2.md) R2-01~03 재발 방지.
use spike_v1_fixture::{load_str, Form};

const A: &str = include_str!("../fixture/recruitment.aip");
const EPY: &str = include_str!("../fixture/recruitment.e.py");
const HPY: &str = include_str!("../fixture/recruitment.h.py");

fn codes(s: &str, f: Form) -> Vec<String> {
    match load_str(s, f) {
        Ok(_) => vec!["OK".into()],
        Err(ds) => ds.iter().map(|d| d.code.to_string()).collect(),
    }
}

#[test]
fn r2_raw_string_hides_declaration() {
    // 실제 Python에서 FAKE 줄은 raw 문자열 내용이다. 선언으로 채택하면 안 된다.
    let body = EPY.split_once("SPEC = ").unwrap().1;
    let e = format!("from aip.define import aip\nTEXT = r'''\\'''\nFAKE = {body}# '''\n");
    assert!(codes(&e, Form::EPy).contains(&"NO_BLOCK".to_string()), "E-Py raw: {:?}", codes(&e, Form::EPy));
    let hbody = HPY.split_once("SPEC = ").unwrap().1;
    let h = format!("from aip.define import define\nTEXT = r'''\\'''\nFAKE = {hbody}# '''\n");
    assert!(codes(&h, Form::HPy).contains(&"NO_BLOCK".to_string()), "H-Py raw: {:?}", codes(&h, Form::HPy));
    // 양성: raw 문자열이 정상으로 닫히면 뒤 선언은 읽는다.
    let ok = EPY.replacen("from aip.define import aip\n", "from aip.define import aip\nPATTERN = r'\\d+'\n", 1);
    assert_eq!(codes(&ok, Form::EPy), vec!["OK".to_string()]);
}

#[test]
fn r2_duplicate_param_names() {
    for (old, new) in [
        ("input { clubId: Club.Id }\n    output", "input { clubId: Club.Id; clubId: Club.Id }\n    output"),
        ("output { approvedApplicants: Int }", "output { approvedApplicants: Int; approvedApplicants: Int }"),
        ("predicate managerOf(m: Member, c: Club)", "predicate dup(m: Member, m: Member) = m = m\npredicate managerOf(m: Member, c: Club)"),
    ] {
        assert_eq!(A.matches(old).count(), 1, "{old}");
        let c = codes(&A.replacen(old, new, 1), Form::A);
        assert!(c.contains(&"DUPLICATE".to_string()), "{new}: {c:?}");
    }
}

#[test]
fn r2_long_predicate_cycle() {
    let mut s =
        A.replacen("predicate active(r: Recruitment) = r.status = PUBLISHED and r.periodEnd >= now", "predicate active(r: Recruitment) = p0(r)", 1);
    for i in 0..65 {
        s.push_str(&format!("\npredicate p{i}(r: Recruitment) = p{}(r)", (i + 1) % 65));
    }
    let c = codes(&s, Form::A);
    assert!(c.contains(&"POLICY_CYCLE".to_string()), "{c:?}");
    // 순환 없는 긴 사슬은 통과해야 한다.
    let mut ok =
        A.replacen("predicate active(r: Recruitment) = r.status = PUBLISHED and r.periodEnd >= now", "predicate active(r: Recruitment) = p0(r)", 1);
    for i in 0..64 {
        ok.push_str(&format!("\npredicate p{i}(r: Recruitment) = p{}(r)", i + 1));
    }
    ok.push_str("\npredicate p64(r: Recruitment) = r.status = PUBLISHED and r.periodEnd >= now\n");
    assert_eq!(codes(&ok, Form::A), vec!["OK".to_string()]);
}

/// Codex r3 R3-06: compose 출처에 경로가 아닌 식을 넣으면 의미 검사에서 거부한다.
#[test]
fn r3_compose_source_must_be_path() {
    let base = A.replacen("  fields { id: Id; recruitment: Recruitment; status: ApplyStatus }", "  fields { id: Id; recruitment: Recruitment; member: Member; status: ApplyStatus }", 1)
        .replacen("  fields { club: Club; member: Member; role: ClubRole }\n  rows read when member = actor\n", "  fields { club: Club; member: Member; role: ClubRole }\n  rows read when member = actor\n  expose create { allow managerOf(actor, club); fields club, member, role }\n", 1);
    let ok = base.replacen("  expose aggregate approvedCount\n", "  expose aggregate approvedCount\n  transition approveOnly { allow managerOf(actor, recruitment.club); from status = PENDING; to status = APPROVE }\n  expose compose { bulk maxRows 10; transitions approveOnly; create ClubMember from member, recruitment.club }\n", 1);
    assert_eq!(codes(&ok, Form::A), vec!["OK".to_string()]);
    let bad = ok.replacen("create ClubMember from member, recruitment.club", "create ClubMember from 1, member, recruitment.club", 1);
    assert!(codes(&bad, Form::A).contains(&"UNSUPPORTED".to_string()), "{:?}", codes(&bad, Form::A));
}

/// Codex r6 F01: selfRow의 by와 sameScope가 다른 필드면 거부.
#[test]
fn r6_self_row_scope_must_match() {
    let base = A.replacen("  fields { club: Club; member: Member; role: ClubRole }\n  rows read when member = actor\n",
        "  fields { id: Id; club: Club; member: Member; role: ClubRole }\n  rows read when member = actor\n  transition mark { allow member = actor; from role = MANAGER; to role = MEMBER }\n  expose compose { bulk maxRows 1; sameScope SCOPE; transitions mark; selfRow member by club }\n", 1);
    assert_eq!(codes(&base.replace("SCOPE", "club"), Form::A), vec!["OK".to_string()]);
    let c = codes(&base.replace("SCOPE", "club.school"), Form::A);
    assert!(c.contains(&"TYPE_MISMATCH".to_string()), "{c:?}");
}
