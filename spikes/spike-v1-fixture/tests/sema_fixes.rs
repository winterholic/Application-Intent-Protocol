//! 감사 결함 회귀: 필드 정책 필드의 filter/sort 누설, exists 안 this 재바인딩, invariant 집행 범위.
use spike_v1_fixture::{load_str, Form};

const A: &str = include_str!("../fixture/recruitment.aip");

fn codes(s: &str) -> Vec<String> {
    match load_str(s, Form::A) {
        Ok(_) => vec!["OK".into()],
        Err(ds) => ds.iter().map(|d| d.code.to_string()).collect(),
    }
}

fn sub(old: &str, new: &str) -> String {
    assert_eq!(A.matches(old).count(), 1, "{old}");
    A.replacen(old, new, 1)
}

#[test]
fn policy_field_cannot_be_filtered_or_sorted() {
    for (old, new) in [
        ("filter periodEnd.gte, periodEnd.lte", "filter periodEnd.gte, internalNote.eq"),
        ("filter periodEnd.gte, periodEnd.lte", "filter periodEnd.gte, internalNote.prefix"),
        ("sort periodEnd, views, id", "sort periodEnd, internalNote, id"),
    ] {
        let c = codes(&sub(old, new));
        assert!(c.contains(&"POLICY_FIELD_NOT_FILTERABLE".to_string()), "{new}: {c:?}");
    }
    // 양성: 정책 없는 필드 filter/sort는 그대로 통과한다.
    assert_eq!(codes(A), vec!["OK".to_string()]);
}

#[test]
fn this_inside_exists_is_rejected() {
    for body in ["exists ClubMember where club = this.club and member = m", "exists ClubMember where club = c and member = this"] {
        let c = codes(&sub("exists ClubMember where club = c and member = m and role in (ADMIN, MANAGER)", body));
        assert!(c.contains(&"THIS_IN_EXISTS".to_string()), "{body}: {c:?}");
    }
}

#[test]
fn count_invariants_use_locked_checks_but_time_dependent_conditions_are_rejected() {
    let source = sub("atMost 1 where status = PUBLISHED", "atMost 2 where status = PUBLISHED");
    let output = load_str(&source, Form::A).unwrap_or_else(|diagnostics| panic!("{diagnostics:?}"));
    let where_clause = &output.execution["limits"]["atMostOnePublished"]["where"];
    assert_eq!(where_clause["cmp"], "=");
    assert_eq!(where_clause["l"]["path"]["root"], "this");
    assert_eq!(where_clause["l"]["path"]["segs"], serde_json::json!(["status"]));
    assert_eq!(where_clause["r"], serde_json::json!({"enum":"RecruitmentStatus.PUBLISHED"}));
    assert_eq!(
        output.execution["resources"]["Recruitment"]["invariants"]["atMostOnePublished"]["enforcement"],
        serde_json::json!({
            "kind":"lockedCountCheck",
            "columns":["club"],
            "where":where_clause,
            "max":2,
            "deferred":false
        })
    );

    let c = codes(&sub("atMost 1 where status = PUBLISHED", "atMost 1 where status = PUBLISHED and periodEnd >= now"));
    assert!(c.contains(&"UNSUPPORTED_INVARIANT".to_string()), "{c:?}");
}
