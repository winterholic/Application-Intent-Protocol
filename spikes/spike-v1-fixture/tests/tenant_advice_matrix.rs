//! A3 (stabilization Track A): the tenant advisories against counterexample definitions, one row per situation of the
//! request: (1) `rows read` has the tenant and `create allow` does not, (2) a write is wider than the read, (3) the tenant
//! is lost through a relation, an aggregate or an extension, (4) the same tenant condition written another way,
//! (5) the tenant is missing on every path. Each row names the advisory codes the checker reports **today**; the
//! `Verdict` column says whether that is right, a false positive (FP), a false negative (FN) or a deliberate silence.
//! The table is in plan-docs/stabilization-20261122/track-a.md. AIP has no automatic tenant isolation (SUPPORT 13), so a
//! definition without tenant evidence (no policy ties a same-named reference of resource and actor) is never judged.

use spike_v1_fixture::{load_str, Form};

const UNSCOPED: &str = "DEF.ADVISORY_TENANT_UNSCOPED_READ";
const WIDER: &str = "DEF.ADVISORY_WRITE_WIDER_THAN_READ";
const PUBLIC_FIELD: &str = "DEF.ADVISORY_PUBLIC_READ_TENANT_FIELD";
const COUNT: &str = "DEF.ADVISORY_TENANT_UNSCOPED_COUNT";
const PUBLIC_COUNT: &str = "DEF.ADVISORY_PUBLIC_COUNT";

const BASE: &str = r#"actor Member
resource Org {
  fields { id: Id; name: Text }
  rows read when id = actor.org
}
resource Member {
  fields { id: Id; org: Org; name: Text }
  rows read when org = actor.org
}
predicate sameOrg(n: Note, a: Member) = n.org = a.org
predicate mineOrg(n: Note) = n.org = actor.org
predicate hasMember(n: Note, a: Member) = exists Member where org = n.org and id = a.id
access anyone(a: Member) = true
"#;

fn note(body: &str) -> String {
    format!("{BASE}resource Note {{\n  fields {{ id: Id; org: Org; body: Text; owner: Member; open: Bool }}\n{body}\n}}\n")
}

/// The tenant resource readable by everyone: a created `org` reference is then not pinned to the caller's own tenant.
fn open_org(src: String) -> String {
    src.replacen("rows read when id = actor.org", "rows read when true", 1)
}

const CREATE_ALL: &str = "expose create { allow ALLOW; fields org, body, owner, open }";
fn create(allow: &str) -> String {
    CREATE_ALL.replace("ALLOW", allow)
}

fn agg_app(aggregate_line: &str, expose: bool) -> String {
    let expose = if expose { "  expose aggregate openNotes\n" } else { "" };
    format!(
        "actor Member\nresource Org {{ fields {{ id: Id; name: Text }} rows read when id = actor.org }}\n\
         resource Member {{ fields {{ id: Id; org: Org; name: Text }} rows read when org = actor.org\n  {aggregate_line}\n{expose}}}\n\
         access anyone(a: Member) = true\n\
         resource Note {{\n  fields {{ id: Id; org: Org; body: Text; qty: Int; open: Bool; owner: Member }}\n  rows read when org = actor.org\n}}\n"
    )
}

fn count_line(where_: &str, release: &str) -> String {
    format!("aggregate openNotes: Int {{ source Note; sourceAccess anyone(actor); {where_}callerFilter none; rowOutput none; release {release} }}")
}

/// (code, name inside the first backticks of the message). The name is the resource or the aggregate the advisory is about.
fn found(src: &str) -> Vec<(String, String)> {
    let checked = load_str(src, Form::A).unwrap_or_else(|e| panic!("{:?}\n{src}", e.iter().map(|d| (d.code, d.msg.clone())).collect::<Vec<_>>()));
    checked
        .advisories
        .iter()
        .filter(|d| [UNSCOPED, WIDER, PUBLIC_FIELD, COUNT, PUBLIC_COUNT].contains(&d.code))
        .map(|d| (d.code.to_string(), d.msg.split('`').nth(1).unwrap_or("").to_string()))
        .collect()
}

fn expect(id: &str, src: &str, want: &[(&str, &str)]) {
    let want: Vec<(String, String)> = want.iter().map(|(c, n)| (c.to_string(), n.to_string())).collect();
    assert_eq!(found(src), want, "case {id}\n{src}");
}

#[test]
fn situation_1_read_has_the_tenant_and_create_allow_does_not() {
    let read = "  rows read when org = actor.org\n";
    // The tenant resource admits only the caller's own org, and a created reference must be readable: `org = actor.org`
    // is guaranteed without being written (a correct silence, not a miss).
    expect("1a", &note(&format!("{read}  {}", create("body != \"\""))), &[]);
    // The same definition with the tenant resource readable by everyone: the caller picks `org` and creates a row it cannot read.
    expect("1a'", &open_org(note(&format!("{read}  {}", create("body != \"\"")))), &[(WIDER, "Note")]);
    expect("1b'", &open_org(note(&format!("{read}  {}", create("org = actor.org")))), &[]);
    expect("1d'", &open_org(note(&format!("{read}  {}", create("sameOrg(this, actor)")))), &[]);
    // `owner.org = actor.org` does not fix the row's own `org` field.
    expect("1e'", &open_org(note(&format!("{read}  {}", create("owner.org = actor.org")))), &[(WIDER, "Note")]);
    // The caller does not choose `org` (nullable, server fills it): nothing to report.
    expect("1c", &note(&format!("{read}  expose create {{ allow body != \"\"; fields body, owner, open }}")).replace("org: Org;", "org: Org?;"), &[]);
    // A second term of `rows read` (`open`) that the caller sets and `allow` omits.
    expect("1f'", &open_org(note(&format!("  rows read when org = actor.org and open\n  {}", create("org = actor.org")))), &[(WIDER, "Note")]);
}

#[test]
fn situation_2_write_wider_than_read() {
    let read = "  rows read when org = actor.org\n";
    let patch_org = "  transition move { allow true; from true; patch org }\n  expose apply move { target id; bulk maxRows 1 }";
    expect("2a", &note(&format!("{read}{patch_org}")), &[]);
    expect("2b", &open_org(note(&format!("{read}{patch_org}"))), &[(WIDER, "Note")]);
    // An internal transition is not callable by the public API.
    let internal = "  transition move { allow true; from true; patch org }\n  expose apply move { target id; bulk maxRows 1; internal }";
    expect("2c", &open_org(note(&format!("{read}{internal}"))), &[]);
    // A transition acts on rows the caller can read (`rows read` filters the target), so `allow true` is not wider.
    let close = "  transition close { allow true; from true; to open = false }\n  expose apply close { target id; bulk maxRows 50 }";
    expect("2d", &note(&format!("{read}{close}")), &[]);
    expect("2e", &note(&format!("  rows read when org = actor.org and open\n  {}", create("org = actor.org"))), &[(WIDER, "Note")]);
}

#[test]
fn situation_3_the_tenant_is_lost_through_a_relation_an_aggregate_or_an_extension() {
    // Relation: the traversed child is judged as a resource of its own.
    let comment = "resource Comment {\n  fields { id: Id; org: Org; note: Note; text: Text }\n  rows read when true\n  expose read { select id, text; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }\n}\n";
    let traversed = format!(
        "{}{comment}",
        note("  rows read when org = actor.org\n  expose read { select id, body, org; traverse comments via Comment.note { select id, text; sort id; limit 10 }; sort id; budget { rows 20; depth 2; deadline 2s; cost 1000 } }")
    );
    expect("3a", &traversed, &[(PUBLIC_FIELD, "Comment")]);
    // Aggregate: a count that can be called on its own counts other tenants' rows unless `where` ties the tenant.
    expect("3b", &agg_app(&count_line("where open = true; ", "count"), true), &[(COUNT, "openNotes")]);
    expect("3c", &agg_app(&count_line("where open = true and org = actor.org; ", "count"), true), &[]);
    expect("3d", &agg_app(&count_line("where open = true; ", "sum(qty)"), true), &[]);
    // `owner = actor` ties the row to the caller itself, like in `rows read`.
    expect("3h", &agg_app(&count_line("where owner = actor; ", "count"), true), &[]);
    // Not exposed with `expose aggregate`: only a per-row value, no standalone total.
    expect("3i", &agg_app(&count_line("where open = true; ", "count"), false), &[]);
    // A count without `where` keeps its own advisory (DEF.ADVISORY_PUBLIC_COUNT) and is not reported twice.
    expect("3g", &agg_app(&count_line("", "count"), true), &[(PUBLIC_COUNT, "openNotes")]);
    // Extension: a write extension that creates a Note shares the create allow, which is judged on the resource.
    let ext = "  extension write mk { input { b: Text(1..9) } output { id: Text(0..20) } access create.Note effect db deadline 3s implementation \"x.mk\" }";
    let allow = create("body != \"\"");
    expect("3e", &note(&format!("  rows read when org = actor.org\n  {allow}\n{ext}")), &[]);
    expect("3e'", &open_org(note(&format!("  rows read when org = actor.org\n  {allow}\n{ext}"))), &[(WIDER, "Note")]);
    // refIds publishes the raw id of the target on purpose (opt-in); it is not a tenant advisory.
    expect(
        "3f",
        &note("  rows read when org = actor.org\n  expose read { select id, org, owner; refIds owner; sort id; budget { rows 20; depth 1; deadline 2s; cost 1000 } }"),
        &[],
    );
}

#[test]
fn situation_4_the_same_tenant_condition_written_another_way() {
    for (id, policy) in [
        ("4a", "sameOrg(this, actor)"),
        ("4b", "mineOrg(this)"),
        ("4c", "actor.org = org"),
        // Used to be a false positive: the negated comparison was not read as the tenant comparison.
        ("4d", "not (org != actor.org)"),
        ("4d2", "not not (org = actor.org)"),
        ("4e", "org = actor.org and true"),
        ("4f", "(org = actor.org and open) or (org = actor.org and owner = actor)"),
        ("4g", "(org = actor.org and open) or owner = actor"),
        ("4i", "mineOrg(this) and open"),
        ("4j", "org.id = actor.org"),
    ] {
        expect(id, &note(&format!("  rows read when {policy}")), &[]);
    }
    // A branch that keeps neither the tenant nor the actor leaves the other tenants readable.
    expect("4h", &note("  rows read when (org = actor.org and open) or open"), &[(UNSCOPED, "Note")]);
    // Not an equality at all.
    expect("4h2", &note("  rows read when not (org = actor.org)"), &[(UNSCOPED, "Note")]);
    // Verdicts that stay as they are (documented limits):
    // FN, relation path: the tenant of the row's owner is not the row's own tenant field, yet it is accepted as the tie.
    expect("4k", &note("  rows read when owner.org = actor.org"), &[]);
    // Deliberate silence: `exists` policies are not judged (membership tables express tenancy in ways a structural comparison cannot follow).
    expect("4l", &note("  rows read when hasMember(this, actor)"), &[]);
}

#[test]
fn situation_5_the_tenant_is_missing_on_every_path() {
    // No policy anywhere ties a tenant reference to the actor: there is no evidence of tenancy, so no advisory (never a guess).
    let no_evidence = "actor Member\nresource Org { fields { id: Id; name: Text } rows read when true }\nresource Member { fields { id: Id; org: Org; name: Text } rows read when true }\nresource Note {\n  fields { id: Id; org: Org; body: Text }\n  rows read when true\n  expose create { allow true; fields org, body }\n}\n";
    expect("5a", no_evidence, &[]);
    // The Member policy is the evidence (org = actor.org); Note forgets it on read and create.
    let evidence = "actor Member\nresource Org { fields { id: Id; name: Text } rows read when id = actor.org }\nresource Member { fields { id: Id; org: Org; name: Text } rows read when org = actor.org }\nresource Note {\n  fields { id: Id; org: Org; body: Text }\n  rows read when true\n  expose create { allow true; fields org, body }\n}\n";
    expect("5b", evidence, &[(PUBLIC_FIELD, "Note")]);
    // The same, with a policy that is not `true` but still has no tenant term.
    let unscoped = evidence.replace("rows read when true\n  expose create", "rows read when body != \"\"\n  expose create");
    expect("5b2", &unscoped, &[(UNSCOPED, "Note"), (WIDER, "Note")]);
    // `rows read` omitted is default deny: no row is readable, which is not a tenant exposure.
    expect("5c", &evidence.replace("  rows read when true\n", ""), &[]);
    // The row is tied to the caller itself, not to a tenant, and the create allow forgets that term.
    expect("5d", &open_org(note(&format!("  rows read when owner = actor\n  {}", create("body != \"\"")))), &[(WIDER, "Note")]);
}
