//! P1 sema: the `DEF.ACCESS_MISMATCH` wording and the three policy-writing advisories
//! (plan-docs/expressiveness-1.0/p1-sema-notes.md). The advisories never reject a definition.

use spike_v1_fixture::{codes, digest, load_str, Form};

const TENANT: &str = "DEF.ADVISORY_TENANT_UNSCOPED_READ";
const WIDER: &str = "DEF.ADVISORY_WRITE_WIDER_THAN_READ";
const PUBLIC: &str = "DEF.ADVISORY_PUBLIC_READ_TENANT_FIELD";

/// Org is the tenant: `Member.org` and `Note.org` carry the same reference, every policy ties it to `actor.org`,
/// and the tenant resource itself admits only the caller's own row.
const APP: &str = r#"
actor Member
resource Org { fields { id: Id; name: Text(1..40) } rows read when id = actor.org }
resource Member { fields { id: Id; org: Org; name: Text(1..40) } rows read when org = actor.org }
resource Note {
  fields { id: Id; org: Org; owner: Member; body: Text(1..200); published: Bool }
  rows read when org = actor.org
  expose create { allow org = actor.org and owner = actor; fields org, owner, body, published }
  transition reword { allow owner = actor; from true; patch body }
  expose apply reword { target id; bulk maxRows 1 }
}
"#;

fn advisories(src: &str) -> Vec<&'static str> {
    load_str(src, Form::A).unwrap_or_else(|e| panic!("{e:?}")).advisories.iter().map(|d| d.code).collect()
}

fn with(old: &str, new: &str) -> String {
    assert_eq!(APP.matches(old).count(), 1, "`{old}` must occur once in APP");
    APP.replacen(old, new, 1)
}

fn only_policy(src: &str) -> Vec<&'static str> {
    advisories(src).into_iter().filter(|c| [TENANT, WIDER, PUBLIC].contains(c)).collect()
}

#[test]
fn access_mismatch_says_what_the_judgement_is() {
    // Task declares the aggregate, Comment is the source and points back with `task`.
    let def = r#"
actor Member
resource Member { fields { id: Id } rows read when true }
resource Task {
  fields { id: Id; title: Text(1..20) }
  rows read when true
  aggregate commentCount: Int { source Comment; sourceAccess visibleTotal; groupKey task; where true; callerFilter none; rowOutput none; release count }
  expose read { select id, title, commentCount; sort id; budget { rows 10; depth 1; deadline 1s; cost 100 } }
}
resource Comment { fields { id: Id; task: Task } rows read when true }
access visibleTotal = totalOfVisible(Comment)
"#;
    let Err(ds) = load_str(def, Form::A) else { panic!("accepted") };
    let d = ds.iter().find(|d| d.code == "DEF.ACCESS_MISMATCH").expect("ACCESS_MISMATCH");
    assert!(d.msg.starts_with("`visibleTotal`은 `Task`를 참조하는 `Comment`의 groupKey 집계에서만 사용"), "{}", d.msg);
    assert!(d.msg.contains("인자가 `Comment`이지만 집계를 선언한 `Task`여야 함"), "{}", d.msg);
    assert!(!d.msg.contains("groupKey가 없음"), "groupKey is there: {}", d.msg);
    // The correct argument is the declaring resource: accepted.
    assert!(load_str(&def.replace("totalOfVisible(Comment)", "totalOfVisible(Task)"), Form::A).is_ok());
    // Without a groupKey the access is rejected too, and the message names that reason.
    let Err(ds) = load_str(&def.replace("totalOfVisible(Comment)", "totalOfVisible(Task)").replace("groupKey task; ", ""), Form::A) else {
        panic!("accepted")
    };
    let d = ds.iter().find(|d| d.code == "DEF.ACCESS_MISMATCH").unwrap();
    assert!(d.msg.contains("aggregate `commentCount`에 groupKey가 없음"), "{}", d.msg);
}

#[test]
fn the_registry_describes_the_actual_judgement() {
    let info = codes::lookup("DEF.ACCESS_MISMATCH").unwrap();
    assert!(info.explain.contains("`totalOfVisible(Task)`"), "{}", info.explain);
    assert!(!info.explain.contains("names a group key"), "the old wording said the opposite: {}", info.explain);
}

#[test]
fn a_definition_that_ties_every_policy_to_the_tenant_has_no_policy_advice() {
    assert_eq!(only_policy(APP), Vec::<&str>::new());
}

#[test]
fn rows_read_that_forgets_the_tenant_is_advised_not_rejected() {
    let src = with("rows read when org = actor.org\n  expose create", "rows read when published\n  expose create");
    assert!(only_policy(&src).contains(&TENANT), "{:?}", advisories(&src));
    // The same mistake through a predicate is found as well as the correct tie through one is accepted.
    let via_pred = format!(
        "predicate seen(n: Note) = n.published\n{}",
        with("rows read when org = actor.org\n  expose create", "rows read when seen(this)\n  expose create")
    );
    assert!(only_policy(&via_pred).contains(&TENANT), "{:?}", advisories(&via_pred));
    let tied_pred = format!(
        "predicate sameOrg(n: Note) = n.org = actor.org\n{}",
        with("rows read when org = actor.org\n  expose create", "rows read when sameOrg(this)\n  expose create")
    );
    assert!(!only_policy(&tied_pred).contains(&TENANT), "{:?}", advisories(&tied_pred));
}

#[test]
fn every_branch_of_an_or_policy_must_keep_the_tenant() {
    let leak = with("rows read when org = actor.org\n  expose create", "rows read when published or org = actor.org\n  expose create");
    assert!(only_policy(&leak).contains(&TENANT), "{:?}", advisories(&leak));
    let kept =
        with("rows read when org = actor.org\n  expose create", "rows read when org = actor.org and (published or owner = actor)\n  expose create");
    assert!(!only_policy(&kept).contains(&TENANT), "{:?}", advisories(&kept));
}

#[test]
fn a_policy_tied_to_the_caller_or_with_exists_is_not_judged() {
    let own = with("rows read when org = actor.org\n  expose create", "rows read when owner = actor\n  expose create");
    assert!(!only_policy(&own).contains(&TENANT), "ownership is not a tenant leak: {:?}", advisories(&own));
    let by_exists =
        with("rows read when org = actor.org\n  expose create", "rows read when exists Member where org = actor.org and id = actor\n  expose create");
    assert!(only_policy(&by_exists).is_empty(), "{:?}", advisories(&by_exists));
}

#[test]
fn a_field_that_no_policy_ties_to_the_actor_is_not_a_tenant() {
    // `team` repeats the actor's `team: Team`, but nothing compares them: it is an attribute, and a public read of it is fine.
    let def = r#"
actor Member
resource Team { fields { id: Id; name: Text(1..20) } rows read when true }
resource Member { fields { id: Id; team: Team } rows read when true }
resource Note { fields { id: Id; team: Team; body: Text(1..20) } rows read when true }
"#;
    assert_eq!(only_policy(def), Vec::<&str>::new());
}

#[test]
fn a_public_read_of_tenant_rows_is_advised() {
    let src = with("rows read when org = actor.org\n  expose create", "rows read when true\n  expose create");
    let found = only_policy(&src);
    assert_eq!(found, [PUBLIC], "a public read is reported once, not also as an unscoped read: {found:?}");
    let ds = load_str(&src, Form::A).unwrap().advisories;
    assert!(ds[0].msg.contains("`Note`") && ds[0].msg.contains("`org`"), "{}", ds[0].msg);
}

#[test]
fn a_create_allow_that_is_wider_than_rows_read_is_advised() {
    // Org is public here, so a created reference is not held to the caller's tenant by its readability.
    let wide = APP
        .replace("rows read when id = actor.org", "rows read when true")
        .replace("allow org = actor.org and owner = actor;", "allow owner = actor;");
    let found = only_policy(&wide);
    assert_eq!(found, [WIDER], "{found:?}");
    let ds = load_str(&wide, Form::A).unwrap().advisories;
    assert!(ds.iter().any(|d| d.msg.contains("`Note`") && d.msg.contains("org = actor.org")), "{ds:?}");
}

#[test]
fn a_missing_term_is_not_reported_when_the_reference_readability_guarantees_it() {
    // Org admits only `actor.org`, and a created reference must be readable: `org = actor.org` is guaranteed.
    let src = APP.replace("allow org = actor.org and owner = actor;", "allow owner = actor;");
    assert_eq!(only_policy(&src), Vec::<&str>::new());
}

#[test]
fn a_term_about_a_referenced_row_is_covered_by_that_rows_own_policy() {
    let def = r#"
actor Member
resource Member { fields { id: Id } rows read when true }
resource Video { fields { id: Id; deleted: Bool } rows read when deleted = false }
resource Remark {
  fields { id: Id; video: Video; author: Member; body: Text(1..20) }
  rows read when video.deleted = false
  expose create { allow author = actor; fields video, author, body }
}
"#;
    assert_eq!(only_policy(def), Vec::<&str>::new(), "Video's own rows read has `deleted = false`");
    let open = def.replace("rows read when deleted = false", "rows read when true");
    assert_eq!(only_policy(&open), [WIDER]);
}

#[test]
fn terms_the_caller_does_not_choose_are_not_reported() {
    // `closed` is filled by the database, not set in `expose create`; a term about the caller alone is not about the new row.
    let def = r#"
actor Member
resource Member { fields { id: Id; admin: Bool } rows read when true }
resource Ticket {
  fields { id: Id; closed: Bool default database; body: Text(1..20) }
  rows read when closed = false and actor.admin
  expose create { allow true; fields body }
}
"#;
    assert_eq!(only_policy(def), Vec::<&str>::new());
    let chosen = def.replace("fields body", "fields body, closed").replace("closed: Bool default database", "closed: Bool");
    assert_eq!(only_policy(&chosen), [WIDER], "the caller can now create a closed ticket it cannot read");
}

#[test]
fn a_patch_of_the_tenant_field_is_advised_when_the_tenant_rows_are_not_limited() {
    let patch_org = APP.replace("patch body", "patch body, org");
    assert_eq!(only_policy(&patch_org), Vec::<&str>::new(), "a patched reference must be readable, and Org admits only actor.org");
    let open_org = patch_org.replace("rows read when id = actor.org", "rows read when true");
    let ds = load_str(&open_org, Form::A).unwrap().advisories;
    let found: Vec<_> = ds.iter().filter(|d| d.code == WIDER).collect();
    assert_eq!(found.len(), 1, "{ds:?}");
    assert!(found[0].msg.contains("transition `reword`") && found[0].msg.contains("`org`"), "{}", found[0].msg);
    // An internal transition is not reachable by a caller.
    let internal =
        open_org.replace("expose apply reword { target id; bulk maxRows 1 }", "expose apply reword { target id; bulk maxRows 1; internal }");
    assert!(!advisories(&internal).contains(&WIDER), "{:?}", advisories(&internal));
}

#[test]
fn the_advisories_change_neither_acceptance_nor_the_execution_digest() {
    // Same facts whether or not the policy advice runs: the advice only reads them. The two definitions differ only in a
    // condition, so the digest of the unchanged one must equal what the plain analysis gives.
    let a = load_str(APP, Form::A).unwrap();
    assert_eq!(digest(&a.execution), digest(&load_str(APP, Form::A).unwrap().execution));
    let leaky = with("rows read when org = actor.org\n  expose create", "rows read when true\n  expose create");
    let b = load_str(&leaky, Form::A).expect("an advisory never rejects");
    assert_eq!(b.advisories.len(), 1);
}

// ---- Decimal sum ----

const SUM_DEF: &str = r#"
actor Member
resource Member { fields { id: Id } rows read when true }
access visibleTotal = totalOfVisible(Order)
resource Order {
  fields { id: Id; title: Text(1..20) }
  rows read when true
  aggregate total: Decimal(14,2) { source Item; sourceAccess visibleTotal; groupKey order; callerFilter none; rowOutput none; release sum(amount) }
  expose read { select id, total; sort id; budget { rows 10; depth 1; deadline 1s; cost 100 } }
}
resource Item { fields { id: Id; order: Order; amount: Decimal(12,2); qty: Int; note: Text(1..10)? } rows read when true }
"#;

fn sum_def(old: &str, new: &str) -> String {
    assert_eq!(SUM_DEF.matches(old).count(), 1, "`{old}` must occur once");
    SUM_DEF.replacen(old, new, 1)
}

fn first_code(src: &str) -> Option<String> {
    load_str(src, Form::A).err().and_then(|ds| ds.first().map(|d| d.code.to_string()))
}

#[test]
fn a_decimal_sum_is_accepted_with_a_result_type_of_the_same_scale_and_at_least_the_precision() {
    let ok = load_str(SUM_DEF, Form::A).unwrap_or_else(|e| panic!("{e:?}"));
    let agg = &ok.execution["resources"]["Order"]["aggregates"]["total"];
    assert_eq!((agg["ty"].as_str(), agg["release"].as_str(), agg["releaseField"].as_str()), (Some("Decimal<14,2>"), Some("sum"), Some("amount")));
    // The same precision is allowed: p' = p.
    assert!(load_str(&sum_def("total: Decimal(14,2)", "total: Decimal(12,2)"), Form::A).is_ok());
}

#[test]
fn a_decimal_sum_with_a_result_type_that_cannot_hold_the_exact_sum_is_rejected() {
    for (why, old, new) in [
        ("scale differs (rounding would be needed)", "total: Decimal(14,2)", "total: Decimal(14,1)"),
        ("scale grows", "total: Decimal(14,2)", "total: Decimal(14,3)"),
        ("precision shrinks", "total: Decimal(14,2)", "total: Decimal(10,2)"),
        ("Int result", "total: Decimal(14,2)", "total: Int"),
        ("nullable result", "total: Decimal(14,2)", "total: Decimal(14,2)?"),
    ] {
        assert_eq!(first_code(&sum_def(old, new)).as_deref(), Some("DEF.TYPE_MISMATCH"), "{why}");
    }
    let Err(ds) = load_str(&sum_def("total: Decimal(14,2)", "total: Decimal(14,1)"), Form::A) else { panic!() };
    assert!(ds[0].msg.contains("Decimal(p',2)") && ds[0].msg.contains("WIRE.OUTPUT_INVALID"), "{}", ds[0].msg);
}

#[test]
fn only_sum_opens_for_decimal_and_the_other_releases_stay_closed() {
    // avg and min/max on Decimal are not opened; sum over a non-number is not either.
    assert_eq!(first_code(&sum_def("release sum(amount)", "release avg(amount)")).as_deref(), Some("DEF.UNSUPPORTED"));
    assert_eq!(first_code(&sum_def("release sum(amount)", "release min(amount)")).as_deref(), Some("DEF.TYPE_MISMATCH"));
    assert_eq!(first_code(&sum_def("release sum(amount)", "release max(amount)")).as_deref(), Some("DEF.TYPE_MISMATCH"));
    assert_eq!(first_code(&sum_def("release sum(amount)", "release sum(note)")).as_deref(), Some("DEF.TYPE_MISMATCH"));
    // An Int sum keeps its rule: the result is Int, and a Decimal result for an Int field is wrong.
    assert!(load_str(&sum_def("total: Decimal(14,2)", "total: Int").replace("sum(amount)", "sum(qty)"), Form::A).is_ok());
    assert_eq!(
        first_code(&sum_def("total: Decimal(14,2)", "total: Decimal(14,2)").replace("sum(amount)", "sum(qty)")).as_deref(),
        Some("DEF.TYPE_MISMATCH")
    );
}

#[test]
fn a_decimal_sum_over_a_policy_field_is_rejected_like_an_int_sum() {
    let src =
        sum_def("note: Text(1..10)? } rows read when true }", "note: Text(1..10)? } rows read when true field amount read when actor != null }");
    assert_eq!(first_code(&src).as_deref(), Some("DEF.POLICY_FIELD_NOT_AGGREGATABLE"));
}
