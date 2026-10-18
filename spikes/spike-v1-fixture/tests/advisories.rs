use spike_v1_fixture::{load_str, Form};

const UNSAFE_AGGREGATE: &str = r#"
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

#[test]
fn caller_filter_read_without_where_warns_when_source_has_row_policy() {
    let checked = load_str(UNSAFE_AGGREGATE, Form::A).expect("accepted definitions may carry an advisory");
    assert!(checked.advisories.iter().any(|d| d.code == "DEF.ADVISORY_CALLER_FILTER_POLICY"));
}

#[test]
fn caller_filter_read_with_explicit_where_and_facereview_have_no_advisory() {
    let safe = UNSAFE_AGGREGATE.replace("    callerFilter read", "    where owner = actor\n    callerFilter read");
    let checked = load_str(&safe, Form::A).expect("valid explicit policy");
    assert!(checked.advisories.is_empty());

    let no_policy = UNSAFE_AGGREGATE.replace("  rows read when owner = actor\n", "");
    let checked = load_str(&no_policy, Form::A).expect("source without a row policy");
    assert!(checked.advisories.is_empty());

    let facereview = include_str!("../../../product/tests/fixtures/facereview-mysql/facereview.aip");
    let checked = load_str(facereview, Form::A).expect("FaceReview definition");
    assert!(checked.advisories.is_empty());
}
