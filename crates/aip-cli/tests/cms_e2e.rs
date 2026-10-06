//! End-to-end for the CMS example: the three forms that put a person on record.
//!
//! - `publishable`: readers see the published version only, editors change the draft, publishing and
//!   discarding are allowed to whoever `publishable by` names
//! - `consent`: an intent needs the caller's consent to the current version of the terms
//! - `impersonate`: support staff act as a user for a limited time, on record, without lending their own powers
//!   and without doing what only the person may
//!
//! `cms_negative_controls` switches the check behind each rule off in the plan and requires the attack that the
//! rule stops to work, so a passing end-to-end test is not a test that cannot fail.

mod common;

use aip_plan::{CheckKind, Intent, Step};
use aip_runtime::auth;
use aip_runtime::engine::{Call, Engine, Impersonation, Reply};
use aip_runtime::error::AipError;
use aip_runtime::objects::ObjectStore;
use common::{call, fails, ok, sql_one};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

static KEY: AtomicUsize = AtomicUsize::new(0);

fn key() -> String {
    format!("k{}", KEY.fetch_add(1, Ordering::Relaxed))
}

fn source() -> String {
    std::fs::read_to_string(format!("{}/../../examples/cms/app.aip", env!("CARGO_MANIFEST_DIR"))).expect("cms source")
}

fn compile(src: &str) -> aip_plan::Program {
    let (core, map) = aip_sema::pipeline::check_source(src).into_core().expect("cms checks clean");
    let compiled = aip_pg::compile(&core, &map);
    assert!(!compiled.diagnostics.iter().any(|d| d.is_error()), "{:?}", compiled.diagnostics);
    compiled.program
}

/// The same database run by a different program: what restarting with an edited source does.
fn restarted(e: &Engine, src: &str) -> Engine {
    Engine {
        program: Arc::new(compile(src)),
        pool: e.pool.clone(),
        objects: ObjectStore::new(std::env::temp_dir().join("aip-e2e-objects")),
        secret: e.secret.clone(),
        outbound: e.outbound.clone(),
        keys: e.keys.clone(),
    }
}

async fn scalar(e: &Engine, q: &str) -> Value {
    sql_one(e, q).await
}

async fn count(e: &Engine, q: &str) -> i64 {
    scalar(e, &format!("SELECT count(*) AS n FROM {q}")).await.as_i64().expect("count")
}

async fn member(e: &Engine, name: &str, role: &str) -> String {
    let v = scalar(e, &format!("INSERT INTO member (email, name, role) VALUES ('{name}@x.com', '{name}', '{role}') RETURNING id")).await;
    v.as_str().expect("id").to_string()
}

fn id(v: &Value) -> String {
    v["id"].as_str().expect("id").to_string()
}

async fn site(e: &Engine, owner: &str, name: &str) -> String {
    id(&ok(e, "CreateSite", Some(owner), json!({"name": name}), Some(&key())).await)
}

async fn agree(e: &Engine, who: &str) {
    ok(e, "GiveTermsConsent", Some(who), json!({}), None).await;
}

async fn create(e: &Engine, who: &str, site: &str) -> Result<Reply, AipError> {
    call(e, "CreateArticle", Some(who), json!({"site": site, "title": "t", "body": "b"}), Some(&key())).await
}

async fn article(e: &Engine, who: &str, site: &str, title: &str) -> String {
    id(&ok(e, "CreateArticle", Some(who), json!({"site": site, "title": title, "body": "text"}), Some(&key())).await)
}

fn titles(v: &Value) -> Vec<String> {
    v.as_array().expect("list").iter().map(|a| a["title"].as_str().expect("title").to_string()).collect()
}

async fn reader_sees(e: &Engine, site: &str) -> Vec<String> {
    titles(&ok(e, "Articles", None, json!({"site": site}), None).await)
}

async fn editor_sees(e: &Engine, who: &str, site: &str) -> Vec<String> {
    titles(&ok(e, "ArticleDrafts", Some(who), json!({"site": site}), None).await)
}

fn code(r: &Result<impl Sized, AipError>) -> Option<(&str, Option<&str>)> {
    r.as_ref().err().map(|e| (e.code.as_str(), e.reason.as_deref()))
}

fn forbidden(r: &Result<Reply, AipError>, reason: Option<&str>) -> bool {
    code(r) == Some(("AIP.AUTH.FORBIDDEN", reason))
}

/// A call made inside an impersonation session: the actor is the person acted as.
async fn within(e: &Engine, session: &str, intent: &str, actor: &str, input: Value) -> Result<Reply, AipError> {
    e.call(Call {
        intent: intent.into(),
        input,
        actor: Some(actor.into()),
        client: Some("test-client".into()),
        impersonation: Some(Impersonation { session: session.into(), operator: None }),
        ..Default::default()
    })
    .await
}

async fn start(e: &Engine, operator: &str, target: &str) -> Result<Value, AipError> {
    call(e, "StartMemberImpersonation", Some(operator), json!({"target": target, "reason": "ticket 42"}), None).await.map(|r| r.data)
}

// ------------------------------------------------------------------ publishable

/// What the readers and editors see at each step of an article's life, and who may move it between the two.
struct Publishing {
    before_publish: (Vec<String>, Vec<String>),
    outsider_publish: Result<Reply, AipError>,
    published: Vec<String>,
    after_edit: (Vec<String>, Vec<String>),
    republished: Vec<String>,
    after_discard: (Vec<String>, Vec<String>),
    discard_unpublished: Result<Reply, AipError>,
    outsider_discard: Result<Reply, AipError>,
    get_before: Result<Reply, AipError>,
    stamped: bool,
}

async fn publishing(e: &Engine) -> Publishing {
    let (alice, bob) = (member(e, "alice", "USER").await, member(e, "bob", "USER").await);
    let s = site(e, &alice, "S").await;
    agree(e, &alice).await;
    let a = article(e, &alice, &s, "v1").await;
    let before_publish = (reader_sees(e, &s).await, editor_sees(e, &alice, &s).await);
    let get_before = call(e, "GetArticle", None, json!({"article": a}), None).await;
    let outsider_publish = call(e, "PublishArticle", Some(&bob), json!({"article": a}), None).await;
    let first = ok(e, "PublishArticle", Some(&alice), json!({"article": a}), None).await;
    let stamped = first["publishedAt"].is_string()
        && scalar(e, &format!("SELECT published_at IS NOT NULL AS val FROM article_published WHERE id = '{a}'")).await == json!(true);
    let published = reader_sees(e, &s).await;
    ok(e, "EditArticle", Some(&alice), json!({"article": a, "title": "v2", "body": "text"}), None).await;
    let after_edit = (reader_sees(e, &s).await, editor_sees(e, &alice, &s).await);
    ok(e, "PublishArticle", Some(&alice), json!({"article": a}), None).await;
    let republished = reader_sees(e, &s).await;
    ok(e, "EditArticle", Some(&alice), json!({"article": a, "title": "v3", "body": "text"}), None).await;
    let outsider_discard = call(e, "DiscardArticleDraft", Some(&bob), json!({"article": a}), None).await;
    ok(e, "DiscardArticleDraft", Some(&alice), json!({"article": a}), None).await;
    let after_discard = (reader_sees(e, &s).await, editor_sees(e, &alice, &s).await);
    let b = article(e, &alice, &s, "never published").await;
    let discard_unpublished = call(e, "DiscardArticleDraft", Some(&alice), json!({"article": b}), None).await;
    Publishing {
        before_publish,
        outsider_publish,
        published,
        after_edit,
        republished,
        after_discard,
        discard_unpublished,
        outsider_discard,
        get_before,
        stamped,
    }
}

fn check_publishing(p: &Publishing) {
    assert_eq!(p.before_publish, (vec![], vec!["v1".to_string()]), "a draft is in the editor's list and not in the readers'");
    assert_eq!(code(&p.get_before), Some(("AIP.NOT_FOUND", Some("ARTICLE_NOT_FOUND"))), "an unpublished article does not exist for readers");
    assert_eq!(code(&p.outsider_publish), Some(("AIP.AUTH.FORBIDDEN", None)));
    assert_eq!(p.published, vec!["v1".to_string()]);
    assert!(p.stamped, "publishing records when");
    assert_eq!(p.after_edit, (vec!["v1".to_string()], vec!["v2".to_string()]), "an edit changes the draft, not what is published");
    assert_eq!(p.republished, vec!["v2".to_string()]);
    assert_eq!(p.after_discard, (vec!["v2".to_string()], vec!["v2".to_string()]), "discarding brings the draft back to what is published");
    assert_eq!(code(&p.outsider_discard), Some(("AIP.AUTH.FORBIDDEN", None)));
    assert_eq!(code(&p.discard_unpublished), Some(("AIP.PRECONDITION.FAILED", Some("NOT_PUBLISHED"))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publishable_end_to_end() {
    let e = common::setup_app("cms", "aip_e2e_cms_publish").await;
    let p = publishing(&e).await;
    check_publishing(&p);

    // more of the same: deleting unpublishes, sites do not mix, the superuser may publish, a clean slate for another site
    let (alice, bob, root) = (member(&e, "alice2", "USER").await, member(&e, "bob2", "USER").await, member(&e, "root", "ROOT").await);
    let (sa, sb) = (site(&e, &alice, "A").await, site(&e, &bob, "B").await);
    agree(&e, &alice).await;
    agree(&e, &bob).await;
    let (a1, b1) = (article(&e, &alice, &sa, "alpha").await, article(&e, &bob, &sb, "beta").await);
    for (who, art) in [(&alice, &a1), (&bob, &b1)] {
        ok(&e, "PublishArticle", Some(who), json!({"article": art}), None).await;
    }
    assert_eq!(reader_sees(&e, &sa).await, vec!["alpha"], "a site's list holds its own articles");
    assert_eq!(reader_sees(&e, &sb).await, vec!["beta"]);
    let cross = call(&e, "PublishArticle", Some(&alice), json!({"article": b1}), None).await;
    assert_eq!(code(&cross), Some(("AIP.AUTH.FORBIDDEN", None)), "an editor of another site may not publish");
    ok(&e, "EditArticle", Some(&bob), json!({"article": b1, "title": "beta2", "body": "text"}), None).await;
    ok(&e, "PublishArticle", Some(&root), json!({"article": b1}), None).await;
    assert_eq!(reader_sees(&e, &sb).await, vec!["beta2"], "the superuser publishes without being an editor");
    ok(&e, "DeleteArticle", Some(&alice), json!({"article": a1}), None).await;
    assert!(reader_sees(&e, &sa).await.is_empty(), "deleting an article unpublishes it");
    assert_eq!(count(&e, &format!("article_published WHERE id = '{a1}'")).await, 0);
    assert_eq!(count(&e, &format!("article WHERE id = '{a1}' AND deleted_at IS NOT NULL")).await, 1, "a soft-deleted working row stays");
    assert_eq!(count(&e, "article_published").await, count(&e, "article WHERE id IN (SELECT id FROM article_published)").await);
    let err = fails(&e, "ArticleDrafts", Some(&alice), json!({"site": sb}), None).await;
    assert_eq!(err.code, "AIP.AUTH.FORBIDDEN", "the drafts of another site are not for an editor of this one");
    let hit = call(&e, "ArticleDrafts", None, json!({"site": sa}), None).await;
    assert_eq!(code(&hit), Some(("AIP.AUTH.UNAUTHENTICATED", None)), "drafts are for editors, not for the public");
}

/// Without `soft delete` the working row is deleted for real, and the published one goes with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publishable_hard_delete() {
    let src = source().replace("  soft delete\n", "");
    assert!(!src.contains("soft delete\n  tenant"), "the variant has no soft delete");
    let e = common::setup_program("aip_e2e_cms_publish_hard", compile(&src)).await;
    let alice = member(&e, "alice", "USER").await;
    let s = site(&e, &alice, "S").await;
    agree(&e, &alice).await;
    let a = article(&e, &alice, &s, "gone soon").await;
    ok(&e, "PublishArticle", Some(&alice), json!({"article": a}), None).await;
    assert_eq!(reader_sees(&e, &s).await, vec!["gone soon"]);
    ok(&e, "DeleteArticle", Some(&alice), json!({"article": a}), None).await;
    assert!(reader_sees(&e, &s).await.is_empty());
    assert_eq!(count(&e, "article").await + count(&e, "article_published").await, 0);
}

// ------------------------------------------------------------------ consent

struct Consenting {
    refused: Result<Reply, AipError>,
    refused_query: Result<Reply, AipError>,
    anonymous: Result<Reply, AipError>,
    status_before: Value,
    given: Value,
    records_after_two_gives: i64,
    allowed: Result<Reply, AipError>,
    bobs_status: Value,
}

async fn consenting(e: &Engine) -> Consenting {
    let (alice, bob) = (member(e, "alice", "USER").await, member(e, "bob", "USER").await);
    let s = site(e, &alice, "S").await;
    let refused = create(e, &alice, &s).await;
    let refused_query = call(e, "ArticleDrafts", Some(&alice), json!({"site": s}), None).await;
    let anonymous = call(e, "CreateArticle", None, json!({"site": s, "title": "t", "body": "b"}), Some(&key())).await;
    let status_before = ok(e, "TermsConsentStatus", Some(&alice), json!({}), None).await;
    let given = ok(e, "GiveTermsConsent", Some(&alice), json!({}), None).await;
    ok(e, "GiveTermsConsent", Some(&alice), json!({}), None).await;
    let records_after_two_gives = count(e, &format!("_aip_consent WHERE actor = '{alice}' AND withdrawn_at IS NULL")).await;
    let allowed = create(e, &alice, &s).await;
    let bobs_status = ok(e, "TermsConsentStatus", Some(&bob), json!({}), None).await;
    Consenting { refused, refused_query, anonymous, status_before, given, records_after_two_gives, allowed, bobs_status }
}

fn check_consenting(c: &Consenting) {
    let r = c.refused.as_ref().expect_err("no consent yet");
    assert_eq!((r.code.as_str(), r.reason.as_deref(), r.status(), r.retryable), ("AIP.CONSENT.REQUIRED", Some("Terms"), 403, false));
    assert_eq!(code(&c.refused_query), Some(("AIP.CONSENT.REQUIRED", Some("Terms"))), "a listed query needs the consent too");
    assert_eq!(code(&c.anonymous), Some(("AIP.AUTH.UNAUTHENTICATED", None)), "consent is a person's: nobody is not asked");
    assert_eq!(c.status_before["status"], json!("NONE"));
    assert_eq!((c.given["status"].clone(), c.given["version"].clone()), (json!("GIVEN"), json!(1)));
    assert_eq!(c.records_after_two_gives, 1, "giving twice records once");
    assert!(c.allowed.is_ok(), "{:?}", c.allowed);
    assert_eq!(c.bobs_status["status"], json!("NONE"), "consent is per person");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn consent_end_to_end() {
    let e = common::setup_app("cms", "aip_e2e_cms_consent").await;
    let c = consenting(&e).await;
    check_consenting(&c);
    let alice = scalar(&e, "SELECT id FROM member WHERE email = 'alice@x.com'").await.as_str().expect("id").to_string();
    let s = scalar(&e, "SELECT id FROM site LIMIT 1").await.as_str().expect("id").to_string();

    // the terms change: a program that asks for version 2 does not honour the old consent
    let v2_src = source().replace("consent Terms version 1", "consent Terms version 2");
    // restarting runs the same schema check as `aip run`: nothing to apply, nothing drifted
    let report = aip_runtime::migrate(&e.pool, &compile(&v2_src), false).await.expect("migrate");
    assert_eq!((report.applied, report.drift.len()), (0, 0));
    let v2 = restarted(&e, &v2_src);
    let r = create(&v2, &alice, &s).await;
    assert_eq!(code(&r), Some(("AIP.CONSENT.REQUIRED", Some("Terms"))), "consent to version 1 does not cover version 2");
    let st = ok(&v2, "TermsConsentStatus", Some(&alice), json!({}), None).await;
    assert_eq!((st["status"].clone(), st["version"].clone(), st["givenVersion"].clone()), (json!("OUTDATED"), json!(2), json!(1)));
    ok(&v2, "GiveTermsConsent", Some(&alice), json!({}), None).await;
    assert!(create(&v2, &alice, &s).await.is_ok(), "consent to the current version");
    assert_eq!(count(&v2, &format!("_aip_consent WHERE actor = '{alice}'")).await, 2, "the record of version 1 stays");
    // the old program still reads version 1 as current, and sees its record
    assert!(create(&e, &alice, &s).await.is_ok());

    // withdrawing ends every active consent
    let w = ok(&v2, "WithdrawTermsConsent", Some(&alice), json!({}), None).await;
    assert_eq!(w["status"], json!("WITHDRAWN"));
    assert_eq!(code(&create(&v2, &alice, &s).await), Some(("AIP.CONSENT.REQUIRED", Some("Terms"))), "withdrawn");
    assert_eq!(code(&create(&e, &alice, &s).await), Some(("AIP.CONSENT.REQUIRED", Some("Terms"))), "withdrawn for the older version as well");
    assert_eq!(count(&v2, &format!("_aip_consent WHERE actor = '{alice}' AND withdrawn_at IS NULL")).await, 0);
    assert_eq!(count(&v2, &format!("_aip_consent WHERE actor = '{alice}' AND withdrawn_at IS NOT NULL")).await, 2, "the history is kept");
    // and can be given again
    ok(&v2, "GiveTermsConsent", Some(&alice), json!({}), None).await;
    assert!(create(&v2, &alice, &s).await.is_ok());
    assert_eq!(count(&v2, &format!("_aip_consent WHERE actor = '{alice}'")).await, 3);
}

// ------------------------------------------------------------------ impersonation

struct People {
    sam: String,
    sam2: String,
    root: String,
    alice: String,
    bob: String,
    carol: String,
    site: String,
}

async fn people(e: &Engine) -> People {
    let (sam, sam2, root) = (member(e, "sam", "SUPPORT").await, member(e, "sam2", "SUPPORT").await, member(e, "root", "ROOT").await);
    let (alice, bob, carol) = (member(e, "alice", "USER").await, member(e, "bob", "USER").await, member(e, "carol", "USER").await);
    let site = site(e, &alice, "S").await;
    agree(e, &alice).await;
    ok(e, "AddEditor", Some(&alice), json!({"site": site, "member": carol}), None).await;
    People { sam, sam2, root, alice, bob, carol, site }
}

fn session_of(v: &Value) -> String {
    v["session"].as_str().expect("session").to_string()
}

/// What each rule of a session lets through.
struct Impersonating {
    by_user: Result<Value, AipError>,
    by_anonymous: Result<Value, AipError>,
    to_self: Result<Value, AipError>,
    to_support: Result<Value, AipError>,
    to_root: Result<Value, AipError>,
    root_to_support: Result<Value, AipError>,
    renamed: Result<Reply, AipError>,
    name_after: Value,
    nested: Result<Reply, AipError>,
    nested_as_support: Option<Result<Reply, AipError>>,
    export: Result<Reply, AipError>,
    erase: Result<Reply, AipError>,
    alice_still_there: i64,
    consent_for_carol: Result<Reply, AipError>,
    carol_status: Value,
    vote: Result<Reply, AipError>,
    own_export: Result<Reply, AipError>,
    own_vote: Result<Reply, AipError>,
    lent_superuser: Result<Reply, AipError>,
    own_superuser: Result<Reply, AipError>,
    wrong_actor: Result<Reply, AipError>,
    unknown_session: Result<Reply, AipError>,
    no_actor: Result<Reply, AipError>,
}

async fn impersonating(e: &Engine) -> Impersonating {
    let p = people(e).await;
    let by_user = start(e, &p.alice, &p.bob).await;
    let by_anonymous = call(e, "StartMemberImpersonation", None, json!({"target": p.bob, "reason": "x"}), None).await.map(|r| r.data);
    let to_self = start(e, &p.sam, &p.sam).await;
    let to_support = start(e, &p.sam, &p.sam2).await;
    // when the privileged-target rule is off a session for another support person exists; the session must still not nest
    let nested_as_support = match &to_support {
        Ok(v) => Some(within(e, &session_of(v), "StartMemberImpersonation", &p.sam2, json!({"target": p.bob, "reason": "x"})).await),
        Err(_) => None,
    };
    let to_root = start(e, &p.sam, &p.root).await;
    let root_to_support = start(e, &p.root, &p.sam2).await;

    // a session for alice, and what it does
    let session = session_of(&start(e, &p.sam, &p.alice).await.expect("support starts a session"));
    let renamed = within(e, &session, "RenameMe", &p.alice, json!({"name": "Alice B"})).await;
    let name_after = scalar(e, &format!("SELECT name AS val FROM member WHERE id = '{}'", p.alice)).await;
    // inside a session even a support person's own condition does not start another
    let nested = within(e, &session, "StartMemberImpersonation", &p.alice, json!({"target": p.bob, "reason": "x"})).await;
    // what is the person's own
    let export = within(e, &session, "ExportMyData", &p.alice, json!({})).await;
    let own_export = call(e, "ExportMyData", Some(&p.alice), json!({}), None).await;
    // a vote: carol (an editor) may vote on an article of the site, the support person acting as carol may not
    let art = article(e, &p.alice, &p.site, "to review").await;
    ok(e, "RequestArticleReview", Some(&p.alice), json!({"article": art}), None).await;
    let csession = session_of(&start(e, &p.sam, &p.carol).await.expect("session for carol"));
    let consent_for_carol = within(e, &csession, "GiveTermsConsent", &p.carol, json!({})).await;
    let carol_status = ok(e, "TermsConsentStatus", Some(&p.carol), json!({}), None).await;
    let vote = within(e, &csession, "ApproveArticleReview", &p.carol, json!({"article": art})).await;
    let own_vote = call(e, "ApproveArticleReview", Some(&p.carol), json!({"article": art}), None).await;

    // the superuser bypass is the operator's: it does not follow the person acted as, even if they become one
    scalar(e, &format!("UPDATE member SET role = 'ROOT' WHERE id = '{}' RETURNING 1 AS val", p.alice)).await;
    let lent_superuser = within(e, &session, "PurgeDrafts", &p.alice, json!({"site": p.site})).await;
    let own_superuser = call(e, "PurgeDrafts", Some(&p.alice), json!({"site": p.site}), None).await;
    scalar(e, &format!("UPDATE member SET role = 'USER' WHERE id = '{}' RETURNING 1 AS val", p.alice)).await;

    // last of the probes that use alice: with the guard off this removes her
    let erase = within(e, &session, "DeleteMyAccount", &p.alice, json!({})).await;
    let alice_still_there = count(e, &format!("member WHERE id = '{}'", p.alice)).await;

    // a session belongs to one person, and has to exist
    let wrong_actor = within(e, &session, "RenameMe", &p.bob, json!({"name": "Not Bob"})).await;
    let unknown_session = within(e, "11111111-1111-4111-8111-111111111111", "RenameMe", &p.alice, json!({"name": "x"})).await;
    let no_actor = e
        .call(Call {
            intent: "RenameMe".into(),
            input: json!({"name": "x"}),
            impersonation: Some(Impersonation { session: session.clone(), operator: None }),
            ..Default::default()
        })
        .await;
    Impersonating {
        by_user,
        by_anonymous,
        to_self,
        to_support,
        to_root,
        root_to_support,
        renamed,
        name_after,
        nested,
        nested_as_support,
        export,
        erase,
        alice_still_there,
        consent_for_carol,
        carol_status,
        vote,
        own_export,
        own_vote,
        lent_superuser,
        own_superuser,
        wrong_actor,
        unknown_session,
        no_actor,
    }
}

fn denied(r: &Result<Value, AipError>, reason: Option<&str>) -> bool {
    code(r) == Some(("AIP.AUTH.FORBIDDEN", reason))
}

fn check_impersonating(i: &Impersonating) {
    assert!(denied(&i.by_user, None), "only support staff start a session: {:?}", code(&i.by_user));
    assert_eq!(code(&i.by_anonymous), Some(("AIP.AUTH.UNAUTHENTICATED", None)));
    assert!(denied(&i.to_self, Some("IMPERSONATION_SELF")), "{:?}", code(&i.to_self));
    assert!(denied(&i.to_support, Some("IMPERSONATION_TARGET_PRIVILEGED")), "a session never reaches someone the operator's condition also passes");
    assert!(denied(&i.to_root, Some("IMPERSONATION_TARGET_PRIVILEGED")), "nor a superuser");
    assert!(
        denied(&i.root_to_support, Some("IMPERSONATION_TARGET_PRIVILEGED")),
        "the superuser passes the condition and is still held to the same rule"
    );
    assert!(i.renamed.is_ok(), "{:?}", i.renamed);
    assert_eq!(i.name_after, json!("Alice B"), "the command ran as the person");
    assert!(forbidden(&i.nested, Some("IMPERSONATION_NESTED")), "{:?}", code(&i.nested));
    assert!(i.nested_as_support.is_none());
    assert!(forbidden(&i.export, Some("IMPERSONATION_FORBIDDEN")), "{:?}", code(&i.export));
    assert!(forbidden(&i.erase, Some("IMPERSONATION_FORBIDDEN")), "{:?}", code(&i.erase));
    assert_eq!(i.alice_still_there, 1);
    assert!(i.own_export.is_ok(), "the person may export their data themselves: {:?}", code(&i.own_export));
    assert!(forbidden(&i.consent_for_carol, Some("IMPERSONATION_FORBIDDEN")), "consent is the person's own");
    assert_eq!(i.carol_status["status"], json!("NONE"));
    assert!(forbidden(&i.vote, Some("IMPERSONATION_FORBIDDEN")), "a vote is the voter's own");
    assert!(i.own_vote.is_ok(), "{:?}", code(&i.own_vote));
    assert!(forbidden(&i.lent_superuser, None), "a session does not lend the superuser bypass: {:?}", code(&i.lent_superuser));
    assert!(i.own_superuser.is_ok(), "the same call as the superuser themselves works: {:?}", code(&i.own_superuser));
    assert_eq!(code(&i.wrong_actor), Some(("AIP.AUTH.UNAUTHENTICATED", Some("IMPERSONATION_ENDED"))), "a session is for its own target");
    assert_eq!(code(&i.unknown_session), Some(("AIP.AUTH.UNAUTHENTICATED", Some("IMPERSONATION_ENDED"))));
    assert_eq!(code(&i.no_actor), Some(("AIP.AUTH.UNAUTHENTICATED", Some("IMPERSONATION_ENDED"))));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn impersonation_end_to_end() {
    let e = common::setup_app("cms", "aip_e2e_cms_impersonate").await;
    let i = impersonating(&e).await;
    check_impersonating(&i);

    // the audit holds both people, and the session holds why
    let rows = scalar(&e, "SELECT jsonb_agg(jsonb_build_object('intent', intent, 'actor', actor, 'by', impersonated_by, 'session', impersonation, 'superuser', superuser) ORDER BY id) AS val FROM _aip_audit WHERE impersonated_by IS NOT NULL").await;
    let rows = rows.as_array().expect("audit rows");
    let sam = scalar(&e, "SELECT id FROM member WHERE email = 'sam@x.com'").await;
    let alice = scalar(&e, "SELECT id FROM member WHERE email = 'alice@x.com'").await;
    let rename = rows.iter().find(|r| r["intent"] == "RenameMe").expect("RenameMe is audited");
    assert_eq!((rename["actor"].clone(), rename["by"].clone(), rename["superuser"].clone()), (alice.clone(), sam.clone(), json!(false)));
    assert!(rename["session"].is_string());
    let reason =
        scalar(&e, &format!("SELECT reason AS val FROM _aip_impersonation WHERE id = '{}'", rename["session"].as_str().expect("session"))).await;
    assert_eq!(reason, json!("ticket 42"));
    // starting is audited too, with what was asked
    let started = scalar(&e, "SELECT jsonb_agg(input ORDER BY id) AS val FROM _aip_audit WHERE intent = 'StartMemberImpersonation' AND actor IS NOT NULL AND impersonated_by IS NULL").await;
    assert!(started.as_array().is_some_and(|a| a.iter().any(|i| i["reason"] == json!("ticket 42"))), "{started}");
    // refused calls change and record nothing
    assert!(rows.iter().all(|r| {
        !["ExportMyData", "DeleteMyAccount", "GiveTermsConsent", "ApproveArticleReview"].contains(&r["intent"].as_str().unwrap_or_default())
    }));
    // a command that does not need audit on its own is audited in a session; the same command outside one is not
    let before = count(&e, "_aip_audit WHERE intent = 'RenameMe'").await;
    let bob = scalar(&e, "SELECT id FROM member WHERE email = 'bob@x.com'").await;
    ok(&e, "RenameMe", bob.as_str(), json!({"name": "Bobby"}), None).await;
    assert_eq!(count(&e, "_aip_audit WHERE intent = 'RenameMe'").await, before, "outside a session RenameMe is not audited");

    // ending: a session ends once, then nothing can use it
    let (sam, alice) = (sam.as_str().expect("id").to_string(), alice.as_str().expect("id").to_string());
    let s1 = session_of(&start(&e, &sam, &alice).await.expect("start"));
    assert!(within(&e, &s1, "RenameMe", &alice, json!({"name": "A"})).await.is_ok());
    let stopped = within(&e, &s1, "StopMemberImpersonation", &alice, json!({})).await.expect("stop inside the session");
    assert_eq!(stopped.data["ended"], json!(1));
    assert_eq!(
        code(&within(&e, &s1, "RenameMe", &alice, json!({"name": "B"})).await),
        Some(("AIP.AUTH.UNAUTHENTICATED", Some("IMPERSONATION_ENDED")))
    );
    // the operator ends every session of theirs with their own token
    let (s2, s3) = (session_of(&start(&e, &sam, &alice).await.expect("start")), session_of(&start(&e, &sam, &alice).await.expect("start")));
    let open = count(&e, &format!("_aip_impersonation WHERE operator = '{sam}' AND ended_at IS NULL AND expires_at > now()")).await;
    assert!(open >= 2);
    let all = ok(&e, "StopMemberImpersonation", Some(&sam), json!({}), None).await;
    assert_eq!(all["ended"], json!(open), "every open session of the operator, and no one else's");
    assert_eq!(count(&e, "_aip_impersonation WHERE ended_at IS NULL AND expires_at > now()").await, 0);
    for s in [&s2, &s3] {
        assert!(within(&e, s, "RenameMe", &alice, json!({"name": "C"})).await.is_err());
    }
    // someone else's stop does not touch them
    let s4 = session_of(&start(&e, &sam, &alice).await.expect("start"));
    let none = ok(&e, "StopMemberImpersonation", Some(&alice), json!({}), None).await;
    assert_eq!(none["ended"], json!(0), "a person cannot end the operator's session from outside");
    assert!(within(&e, &s4, "RenameMe", &alice, json!({"name": "D"})).await.is_ok());

    // expiry: the session row says when, and the engine believes the row
    scalar(&e, &format!("UPDATE _aip_impersonation SET expires_at = now() - interval '1 second' WHERE id = '{s4}' RETURNING 1 AS val")).await;
    assert_eq!(
        code(&within(&e, &s4, "RenameMe", &alice, json!({"name": "E"})).await),
        Some(("AIP.AUTH.UNAUTHENTICATED", Some("IMPERSONATION_ENDED")))
    );
    assert_eq!(
        code(&within(&e, &s4, "TermsConsentStatus", &alice, json!({})).await),
        Some(("AIP.AUTH.UNAUTHENTICATED", Some("IMPERSONATION_ENDED"))),
        "queries as well"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn impersonation_ttl_runs_out() {
    let e = common::setup_program("aip_e2e_cms_ttl", compile(&source().replace("ttl 30m", "ttl 1s"))).await;
    let p = people(&e).await;
    let started = start(&e, &p.sam, &p.alice).await.expect("start");
    let session = session_of(&started);
    let left =
        scalar(&e, &format!("SELECT extract(epoch FROM expires_at - started_at)::int AS val FROM _aip_impersonation WHERE id = '{session}'")).await;
    assert_eq!(left, json!(1), "the program's ttl is what the session lasts");
    assert!(within(&e, &session, "RenameMe", &p.alice, json!({"name": "in time"})).await.is_ok());
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
    let late = within(&e, &session, "RenameMe", &p.alice, json!({"name": "too late"})).await;
    assert_eq!(code(&late), Some(("AIP.AUTH.UNAUTHENTICATED", Some("IMPERSONATION_ENDED"))), "after the ttl nothing runs");
    assert_eq!(scalar(&e, &format!("SELECT name AS val FROM member WHERE id = '{}'", p.alice)).await, json!("in time"));
}

// ------------------------------------------------------------------ the HTTP layer: tokens

/// One request over a plain socket: `(status, JSON body)`.
async fn http(addr: std::net::SocketAddr, intent: &str, token: Option<&str>, body: &Value) -> (u16, Value) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let payload = body.to_string();
    let auth = token.map(|t| format!("authorization: Bearer {t}\r\n")).unwrap_or_default();
    let req = format!(
        "POST /aip/{intent} HTTP/1.1\r\nhost: localhost\r\ncontent-type: application/json\r\n{auth}content-length: {}\r\nconnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(req.as_bytes()).await.expect("send");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.expect("read");
    let status: u16 = raw.split_whitespace().nth(1).and_then(|s| s.parse().ok()).expect("status line");
    let json_body = raw.split("\r\n\r\n").nth(1).and_then(|b| serde_json::from_str(b.trim()).ok()).unwrap_or(Value::Null);
    (status, json_body)
}

async fn serve(e: Arc<Engine>, secret: &[u8]) -> std::net::SocketAddr {
    let port = std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port();
    let state =
        aip_runtime::http::AppState::new(e, Arc::new(secret.to_vec()), false, false, Arc::new(json!({})), "postgres://localhost/postgres").await;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    tokio::spawn(async move {
        let _ = aip_runtime::http::serve(state, addr).await;
    });
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return addr;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("server did not start");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn impersonation_tokens_over_http() {
    let e = common::setup_app("cms", "aip_e2e_cms_http").await;
    let p = people(&e).await;
    let secret: &[u8] = b"cms-test-secret";
    let addr = serve(e.clone(), secret).await;
    let sam_token = auth::issue(secret, &p.sam, 600);

    // the operator asks for a session and gets a token for the person
    let (status, body) = http(addr, "StartMemberImpersonation", Some(&sam_token), &json!({"target": p.alice, "reason": "ticket 7"})).await;
    assert_eq!(status, 200, "{body}");
    let token = body["data"]["token"].as_str().expect("token").to_string();
    let session = body["data"]["session"].as_str().expect("session").to_string();
    let identity = auth::verify(secret, &token).expect("a valid token");
    assert_eq!((identity.actor.as_str(), identity.session.as_deref()), (p.alice.as_str(), Some(session.as_str())));

    // the token acts as the person and the audit has both
    let (status, body) = http(addr, "RenameMe", Some(&token), &json!({"name": "Alice via HTTP"})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(scalar(&e, &format!("SELECT name AS val FROM member WHERE id = '{}'", p.alice)).await, json!("Alice via HTTP"));
    assert_eq!(count(&e, &format!("_aip_audit WHERE intent = 'RenameMe' AND actor = '{}' AND impersonated_by = '{}'", p.alice, p.sam)).await, 1);
    // what only the person may do is refused, and nesting too
    let (status, body) = http(addr, "ExportMyData", Some(&token), &json!({})).await;
    assert_eq!((status, body["error"]["reason"].clone()), (403, json!("IMPERSONATION_FORBIDDEN")), "{body}");
    let (status, body) = http(addr, "StartMemberImpersonation", Some(&token), &json!({"target": p.bob, "reason": "again"})).await;
    assert_eq!((status, body["error"]["reason"].clone()), (403, json!("IMPERSONATION_NESTED")), "{body}");

    // forged tokens
    let sig = token.rsplit('.').next().expect("signature").to_string();
    let b64 = |s: &str| {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s)
    };
    let exp = chrono::Utc::now().timestamp() + 600;
    // a character in the middle of the signature, where every bit counts
    let at = token.len() - 10;
    let mut tampered = token.clone();
    tampered.replace_range(at..=at, if &token[at..=at] == "A" { "B" } else { "A" });
    let forged = [
        ("a flipped signature byte", tampered),
        ("signed with another secret", auth::issue_impersonation(b"not-the-secret", &p.alice, &session, exp)),
        ("the session of a token spliced onto another actor", format!("aip1.{}.{sig}", b64(&format!("{}.{exp}.{session}", p.bob)))),
        (
            "a normal token that grew a session",
            format!("aip1.{}.{}", b64(&format!("{}.{exp}.{session}", p.sam)), sam_token.rsplit('.').next().expect("signature")),
        ),
        ("no signature", format!("aip1.{}", b64(&format!("{}.{exp}.{session}", p.alice)))),
    ];
    for (what, t) in &forged {
        let (status, body) = http(addr, "RenameMe", Some(t), &json!({"name": "forged"})).await;
        assert_eq!((status, body["error"]["code"].clone()), (401, json!("AIP.AUTH.UNAUTHENTICATED")), "{what}: {body}");
    }
    assert_eq!(
        scalar(&e, &format!("SELECT name AS val FROM member WHERE id = '{}'", p.alice)).await,
        json!("Alice via HTTP"),
        "no forged call changed anything"
    );
    // a token signed with the right secret for a session that does not exist (only a holder of the secret could make one) is refused too
    let ghost = auth::issue_impersonation(secret, &p.alice, "11111111-1111-4111-8111-111111111111", exp);
    let (status, body) = http(addr, "RenameMe", Some(&ghost), &json!({"name": "ghost"})).await;
    assert_eq!((status, body["error"]["reason"].clone()), (401, json!("IMPERSONATION_ENDED")), "{body}");

    // stopping: the token stops working at once, long before its own expiry
    let (status, body) = http(addr, "StopMemberImpersonation", Some(&token), &json!({})).await;
    assert_eq!((status, body["data"]["ended"].clone()), (200, json!(1)), "{body}");
    let (status, body) = http(addr, "RenameMe", Some(&token), &json!({"name": "after stop"})).await;
    assert_eq!((status, body["error"]["reason"].clone()), (401, json!("IMPERSONATION_ENDED")), "{body}");
    // a plain token of the person is unaffected
    let alice_token = auth::issue(secret, &p.alice, 600);
    let (status, _) = http(addr, "RenameMe", Some(&alice_token), &json!({"name": "Alice again"})).await;
    assert_eq!(status, 200);
    // and a plain token is no impersonation token: the person cannot start one
    let (status, body) = http(addr, "StartMemberImpersonation", Some(&alice_token), &json!({"target": p.bob, "reason": "x"})).await;
    assert_eq!(status, 403, "{body}");
}

// ------------------------------------------------------------------ negative controls

/// Removes the checks of `intent` that `drop` selects.
fn strip_checks(program: &mut aip_plan::Program, intent: &str, drop: impl Fn(&CheckKind, &Option<String>, &str) -> bool) -> usize {
    let Some(Intent::Command(c)) = program.intents.get_mut(intent) else { panic!("no command {intent}") };
    let before = c.steps.len();
    c.steps.retain(|s| !matches!(s, Step::Check { kind, code, sql } if drop(kind, code, &sql.text)));
    before - c.steps.len()
}

/// The attacks, with the check that stops each one removed from the plan, must work.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cms_negative_controls() {
    // publish and discard no longer check who calls
    let mut program = compile(&source());
    for intent in ["PublishArticle", "DiscardArticleDraft"] {
        assert_eq!(strip_checks(&mut program, intent, |k, _, _| *k == CheckKind::Allow), 1);
    }
    let e = common::setup_program("aip_e2e_cms_no_publish_allow", program).await;
    let p = publishing(&e).await;
    assert!(p.outsider_publish.is_ok(), "without the allow check anybody publishes: {:?}", code(&p.outsider_publish));
    assert!(p.outsider_discard.is_ok(), "and discards");

    // readers' queries read the working table: drafts and edits are public
    let mut program = compile(&source());
    for name in ["Articles", "GetArticle"] {
        let Some(Intent::Query(q)) = program.intents.get_mut(name) else { panic!("no query {name}") };
        let mut texts: Vec<&mut String> = q.variants.iter_mut().map(|v| &mut v.main.text).collect();
        for st in &mut q.prelude {
            if let Step::Load { sql, .. } = st {
                texts.push(&mut sql.text);
            }
        }
        for t in texts {
            *t = t.replace("\"article_published\"", "\"article\"");
        }
    }
    let e = common::setup_program("aip_e2e_cms_no_published_view", program).await;
    let p = publishing(&e).await;
    assert_eq!(p.before_publish.0, vec!["v1".to_string()], "without the published view a draft is public");
    assert!(p.get_before.is_ok());
    assert_eq!(p.after_edit.0, vec!["v2".to_string()], "and an edit shows at once");

    // the consent check removed from the intents it guards
    let mut program = compile(&source());
    assert_eq!(strip_checks(&mut program, "CreateArticle", |k, _, _| *k == CheckKind::Consent), 1);
    let Some(Intent::Query(q)) = program.intents.get_mut("ArticleDrafts") else { panic!("no query") };
    let n = q.prelude.len();
    q.prelude.retain(|s| !matches!(s, Step::Check { kind: CheckKind::Consent, .. }));
    assert_eq!(n - q.prelude.len(), 1);
    let e = common::setup_program("aip_e2e_cms_no_consent_check", program).await;
    let c = consenting(&e).await;
    assert!(c.refused.is_ok() && c.refused_query.is_ok(), "without the check nobody is asked: {:?}", code(&c.refused));

    // the session rules, one at a time
    type Mutation = fn(&mut aip_plan::Program);
    type Attack = fn(&Impersonating) -> bool;
    let cases: [(&str, Mutation, Attack); 4] = [
        // the two rules overlap: a target never passes the operator's condition, so a session cannot nest while the first holds.
        // Without the privileged-target rule alone the nesting rule is the one that still refuses
        ("aip_e2e_cms_no_privileged_check", no_privileged_check, |i| {
            i.to_support.is_ok() && i.to_root.is_ok() && i.nested_as_support.as_ref().is_some_and(|r| forbidden(r, Some("IMPERSONATION_NESTED")))
        }),
        ("aip_e2e_cms_no_nested_check", no_nested_and_privileged_check, |i| i.nested_as_support.as_ref().is_some_and(|r| r.is_ok())),
        ("aip_e2e_cms_no_personal_guard", no_personal_guard, |i| {
            i.export.is_ok() && i.erase.is_ok() && i.consent_for_carol.is_ok() && i.vote.is_ok()
        }),
        ("aip_e2e_cms_su_lent", su_lent, |i| i.lent_superuser.is_ok()),
    ];
    for (db, mutate, attack_works) in cases {
        let mut program = compile(&source());
        mutate(&mut program);
        let e = common::setup_program(db, program).await;
        let i = impersonating(&e).await;
        assert!(attack_works(&i), "{db}: the attack should work with its check removed");
    }
}

fn no_nested_and_privileged_check(p: &mut aip_plan::Program) {
    assert_eq!(strip_checks(p, "StartMemberImpersonation", |_, c, _| c.as_deref() == Some("IMPERSONATION_NESTED")), 1);
    no_privileged_check(p);
}

fn no_privileged_check(p: &mut aip_plan::Program) {
    assert_eq!(strip_checks(p, "StartMemberImpersonation", |_, c, _| c.as_deref() == Some("IMPERSONATION_TARGET_PRIVILEGED")), 1);
}

fn no_personal_guard(p: &mut aip_plan::Program) {
    for intent in ["ExportMyData", "DeleteMyAccount", "GiveTermsConsent", "ApproveArticleReview"] {
        assert_eq!(strip_checks(p, intent, |_, c, _| c.as_deref() == Some("IMPERSONATION_FORBIDDEN")), 1, "{intent}");
    }
}

/// The superuser bypass without the clause that switches it off inside a session.
fn su_lent(p: &mut aip_plan::Program) {
    let mut changed = 0;
    for i in p.intents.values_mut() {
        if let Intent::Command(c) = i {
            for s in &mut c.steps {
                if let Step::Check { sql, .. } = s {
                    for k in 1..=12 {
                        let before = sql.text.clone();
                        sql.text = sql.text.replace(&format!(" AND (${k}::text) IS NULL)"), &format!(" AND ((${k}::text) IS NULL OR true))"));
                        changed += usize::from(before != sql.text);
                    }
                }
            }
        }
    }
    assert!(changed > 0, "the guard is in the plans");
}

// ------------------------------------------------------------------ the operator is asked again on every call

/// A session opened by support staff, then the operator's role is taken away in the database: the next call as
/// the target fails like an ended session and the session row says when it ended.
async fn operator_loses_power(e: &Engine) -> (Result<Reply, AipError>, Result<Reply, AipError>, Result<Reply, AipError>, i64, String) {
    let p = people(e).await;
    let session = session_of(&start(e, &p.sam, &p.alice).await.expect("start"));
    let before = within(e, &session, "RenameMe", &p.alice, json!({"name": "A1"})).await;
    scalar(e, &format!("UPDATE member SET role = 'USER' WHERE id = '{}' RETURNING 1 AS val", p.sam)).await;
    let after = within(e, &session, "RenameMe", &p.alice, json!({"name": "A2"})).await;
    let query_after = within(e, &session, "TermsConsentStatus", &p.alice, json!({})).await;
    let ended = count(e, &format!("_aip_impersonation WHERE id = '{session}' AND ended_at IS NOT NULL AND ended_at <= now()")).await;
    let name = scalar(e, &format!("SELECT name AS val FROM member WHERE id = '{}'", p.alice)).await.as_str().unwrap_or_default().to_string();
    (before, after, query_after, ended, name)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn impersonation_rechecks_the_operator() {
    let e = common::setup_app("cms", "aip_e2e_cms_imp_recheck").await;
    let (before, after, query_after, ended, name) = operator_loses_power(&e).await;
    assert!(before.is_ok(), "while the operator holds the role the session works");
    let gone = Some(("AIP.AUTH.UNAUTHENTICATED", Some("IMPERSONATION_ENDED")));
    assert_eq!(code(&after), gone, "the same response as for an ended session");
    assert_eq!(code(&query_after), gone);
    assert_eq!(ended, 1, "and the session is closed on record");
    assert_eq!(name, "A1", "the refused call changed nothing");

    // negative control: with the operator no longer asked, the revoked operator keeps acting until the ttl
    let mut program = compile(&source());
    let spec = program.impersonation.as_mut().expect("impersonation");
    assert!(spec.operator_check.text.contains("coalesce"), "the plan carries the re-check");
    spec.operator_check = aip_plan::Sql { text: "SELECT true".into(), params: vec![] };
    let e = common::setup_program("aip_e2e_cms_imp_no_recheck", program).await;
    let (_, after, _, ended, name) = operator_loses_power(&e).await;
    assert!(after.is_ok(), "without the re-check the session outlives the operator's role: {:?}", code(&after));
    assert_eq!((ended, name.as_str()), (0, "A2"));
}
