//! End-to-end: the AriAri definition compiled and executed against a real
//! PostgreSQL database. Each section reproduces a rule the original Spring
//! service enforced (or failed to enforce) by hand.
//!
//! Requires a local PostgreSQL; set AIP_TEST_ADMIN_URL to override
//! `postgres://localhost/postgres`.

mod common;

use aip_runtime::engine::{Call, Engine};
use common::{call, drain, fails, ok, sql_one};
use serde_json::{Value, json};
use std::sync::Arc;

async fn setup() -> Arc<Engine> {
    setup_db("aip_e2e").await
}

async fn setup_db(db: &str) -> Arc<Engine> {
    common::setup_app("ariari", db).await
}

async fn member(e: &Engine, nick: &str, super_admin: bool) -> String {
    let role = if super_admin { "SUPER_ADMIN" } else { "USER" };
    // email is encrypted: the row id is chosen here because the ciphertext is bound to it
    let id = uuid::Uuid::new_v4().to_string();
    let email = e.keys.as_ref().expect("the ariari program has encrypted fields").encrypt("Member.email", &id, &format!("{nick}@x.com"));
    let v = sql_one(
        e,
        &format!("INSERT INTO member (id, email, nickname, kakao_id, role) VALUES ('{id}', '{email}', '{nick}', 'k-{nick}', '{role}') RETURNING id"),
    )
    .await;
    v.as_str().expect("id").to_string()
}

fn club_input(name: &str) -> Value {
    json!({"input": {"name": name, "intro": "", "affiliation": "UNION", "field": "ACADEMIC", "region": "CAPITAL", "target": "UNDERGRADUATE"}, "adminName": "Admin"})
}

fn period(days_from: i64, days_to: i64) -> Value {
    let now = chrono::Utc::now();
    json!({"start": (now + chrono::Duration::days(days_from)).to_rfc3339(), "end": (now + chrono::Duration::days(days_to)).to_rfc3339()})
}

fn recruitment_input(p: Value) -> Value {
    json!({"title": "2026 fall", "body": "<p>join <script>x</script></p>", "period": p, "field": "ACADEMIC", "region": "CAPITAL", "target": "UNDERGRADUATE"})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ariari_end_to_end() {
    let e = setup().await;
    let alice = member(&e, "alice", false).await;
    let bob = member(&e, "bob", false).await;
    let carol = member(&e, "carol", false).await;
    let root = member(&e, "root", true).await;

    // --- club creation: the creator becomes the single ADMIN; idempotent
    let club = ok(&e, "CreateClub", Some(&alice), club_input("Rust Club"), Some("k-club")).await;
    let club_id = club["id"].as_str().expect("club id").to_string();
    let again = ok(&e, "CreateClub", Some(&alice), club_input("Rust Club"), Some("k-club")).await;
    assert_eq!(again["id"], club["id"], "idempotent replay returns the stored result");
    assert_eq!(sql_one(&e, "SELECT count(*) FROM club").await, json!(1));
    let reused = fails(&e, "CreateClub", Some(&alice), club_input("Other"), Some("k-club")).await;
    assert_eq!(reused.code, "AIP.IDEMPOTENCY.KEY_REUSED");
    let no_key = fails(&e, "CreateClub", Some(&alice), club_input("X"), None).await;
    assert_eq!(no_key.code, "AIP.INPUT.IDEMPOTENCY_KEY_REQUIRED");
    let anon = fails(&e, "CreateClub", None, club_input("X"), Some("anon")).await;
    assert_eq!(anon.code, "AIP.AUTH.UNAUTHENTICATED");
    let internal = fails(&e, "CreateClub", Some(&bob), json!({"input": {"name": "Campus", "intro": "", "affiliation": "INTERNAL", "field": "ACADEMIC", "region": "CAPITAL", "target": "UNDERGRADUATE"}, "adminName": "B"}), Some("k2")).await;
    assert_eq!(internal.reason.as_deref(), Some("SCHOOL_AUTH_REQUIRED"));
    let bad = fails(&e, "CreateClub", Some(&alice), json!({"input": {"name": "", "intro": "", "affiliation": "UNION", "field": "ACADEMIC", "region": "CAPITAL", "target": "UNDERGRADUATE"}, "adminName": "A"}), Some("k3")).await;
    assert_eq!((bad.code.as_str(), bad.path.as_deref()), ("AIP.INPUT.INVALID", Some("input.name")));

    // --- public detail with a per-client view counter
    let detail = ok(&e, "ClubDetail", Some(&bob), json!({"club": club_id}), None).await;
    assert_eq!(detail["myMembership"], Value::Null);
    assert_eq!(detail["isBookmarked"], json!(false));
    ok(&e, "ClubDetail", Some(&bob), json!({"club": club_id}), None).await;
    assert_eq!(sql_one(&e, &format!("SELECT views FROM club WHERE id = '{club_id}'")).await, json!(1), "same client within a day counts once");
    let mine = ok(&e, "ClubDetail", Some(&alice), json!({"club": club_id}), None).await;
    assert_eq!(mine["myMembership"]["role"], json!("ADMIN"));

    // --- toggle bookmark
    ok(&e, "ToggleClubBookmark", Some(&bob), json!({"club": club_id}), None).await;
    assert_eq!(ok(&e, "ClubDetail", Some(&bob), json!({"club": club_id}), None).await["isBookmarked"], json!(true));
    ok(&e, "ToggleClubBookmark", Some(&bob), json!({"club": club_id}), None).await;
    assert_eq!(ok(&e, "ClubDetail", Some(&bob), json!({"club": club_id}), None).await["isBookmarked"], json!(false));
    ok(&e, "ToggleClubBookmark", Some(&bob), json!({"club": club_id}), None).await;

    // --- recruitment: approval by a super admin (audited), overlapping periods rejected by the database
    let r = ok(&e, "CreateRecruitment", Some(&alice), json!({"club": club_id, "input": recruitment_input(period(-1, 10))}), Some("r1")).await;
    let r_id = r["id"].as_str().expect("recruitment").to_string();
    assert_eq!(sql_one(&e, &format!("SELECT body FROM recruitment WHERE id = '{r_id}'")).await, json!("<p>join x</p>"), "rich text is sanitised");
    let forbidden = fails(&e, "ApproveRecruitment", Some(&bob), json!({"r": r_id}), None).await;
    assert_eq!(forbidden.code, "AIP.AUTH.FORBIDDEN");
    ok(&e, "ApproveRecruitment", Some(&root), json!({"r": r_id}), None).await;
    assert_eq!(sql_one(&e, "SELECT count(*) FROM _aip_audit WHERE intent = 'ApproveRecruitment'").await, json!(1));
    let overlap = fails(&e, "CreateRecruitment", Some(&alice), json!({"club": club_id, "input": recruitment_input(period(5, 20))}), Some("r2")).await;
    assert_eq!(overlap.reason.as_deref(), Some("RECRUITMENT_PERIOD_OVERLAP"));
    let not_manager =
        fails(&e, "CreateRecruitment", Some(&bob), json!({"club": club_id, "input": recruitment_input(period(30, 40))}), Some("r3")).await;
    assert_eq!(not_manager.code, "AIP.AUTH.FORBIDDEN");
    let lifecycle = fails(&e, "ApproveRecruitment", Some(&root), json!({"r": r_id}), None).await;
    assert_eq!(lifecycle.reason.as_deref(), Some("RECRUITMENT_STATUS_INVALID_TRANSITION"), "only declared transitions");

    // --- the RecruitmentOpened event notified the bookmarker after commit
    drain(&e).await;
    assert_eq!(
        sql_one(&e, &format!("SELECT count(*) FROM _aip_notification WHERE recipient = '{bob}' AND template = 'club-recruitment-opened'")).await,
        json!(1)
    );

    // --- applications: unique per member, members cannot apply, concurrent submissions cannot duplicate
    // the club's form has no questions yet, so the only valid answer set is empty
    let answers = json!({});
    let apply = ok(&e, "SubmitApply", Some(&bob), json!({"r": r_id, "answers": answers}), Some("a1")).await;
    let apply_id = apply["id"].as_str().expect("apply").to_string();
    let twice = fails(&e, "SubmitApply", Some(&bob), json!({"r": r_id, "answers": answers}), Some("a2")).await;
    assert_eq!(twice.reason.as_deref(), Some("ALREADY_APPLIED"));
    let member_apply = fails(&e, "SubmitApply", Some(&alice), json!({"r": r_id, "answers": answers}), Some("a3")).await;
    assert_eq!(member_apply.code, "AIP.AUTH.FORBIDDEN");
    let mut handles = Vec::new();
    for i in 0..6 {
        let e2 = e.clone();
        let carol = carol.clone();
        let r_id = r_id.clone();
        handles.push(tokio::spawn(
            async move { call(&e2, "SubmitApply", Some(&carol), json!({"r": r_id, "answers": {}}), Some(&format!("c{i}"))).await },
        ));
    }
    let mut succeeded = 0;
    for h in handles {
        if h.await.expect("join").is_ok() {
            succeeded += 1;
        }
    }
    assert_eq!(succeeded, 1, "exactly one concurrent application wins");
    let carol_apply = sql_one(&e, &format!("SELECT id FROM apply WHERE member_id = '{carol}'")).await.as_str().expect("carol apply").to_string();

    // --- the applicant sees their application; a stranger does not even learn it exists
    ok(&e, "ApplyDetail", Some(&bob), json!({"apply": apply_id}), None).await;
    let hidden = fails(&e, "ApplyDetail", Some(&carol), json!({"apply": apply_id}), None).await;
    assert_eq!(hidden.code, "AIP.NOT_FOUND");

    // --- approving several applications at once: same club, lifecycle, membership created
    let mixed = fails(&e, "ApproveApplies", Some(&bob), json!({"applies": [apply_id]}), None).await;
    assert_eq!(mixed.code, "AIP.AUTH.FORBIDDEN");
    ok(&e, "ApproveApplies", Some(&alice), json!({"applies": [apply_id, carol_apply]}), None).await;
    assert_eq!(sql_one(&e, &format!("SELECT count(*) FROM club_member WHERE club_id = '{club_id}'")).await, json!(3));
    let refuse = fails(&e, "RefuseApplies", Some(&alice), json!({"applies": [apply_id]}), None).await;
    assert_eq!(refuse.reason.as_deref(), Some("APPLY_STATUS_INVALID_TRANSITION"));
    let empty = fails(&e, "ApproveApplies", Some(&alice), json!({"applies": []}), None).await;
    assert_eq!(empty.code, "AIP.INPUT.INVALID", "ariari's applies.get(0) crash becomes an input error");
    let ghost = fails(&e, "ApproveApplies", Some(&alice), json!({"applies": ["00000000-0000-0000-0000-000000000000"]}), None).await;
    assert_eq!(ghost.code, "AIP.NOT_FOUND", "unknown ids are not silently dropped");

    // --- roles: exactly one ADMIN, no self entrust, rank-based status changes
    let bob_cm = sql_one(&e, &format!("SELECT id FROM club_member WHERE member_id = '{bob}'")).await.as_str().expect("bob cm").to_string();
    let alice_cm = sql_one(&e, &format!("SELECT id FROM club_member WHERE member_id = '{alice}'")).await.as_str().expect("alice cm").to_string();
    let self_entrust = fails(&e, "EntrustAdmin", Some(&alice), json!({"target": alice_cm}), None).await;
    assert_eq!(self_entrust.reason.as_deref(), Some("CANNOT_ENTRUST_TO_SELF"));
    let own_role = fails(&e, "ChangeMemberRole", Some(&alice), json!({"target": alice_cm, "role": "MANAGER"}), None).await;
    assert_eq!(own_role.reason.as_deref(), Some("CANNOT_CHANGE_OWN_ROLE"));
    ok(&e, "ChangeMemberRole", Some(&alice), json!({"target": bob_cm, "role": "MANAGER"}), None).await;
    let outranked = fails(&e, "ChangeMemberStatus", Some(&bob), json!({"targets": [alice_cm], "newStatus": "ENDED"}), None).await;
    assert_eq!(outranked.code, "AIP.AUTH.FORBIDDEN", "a MANAGER cannot deactivate the ADMIN (ariari single-target bug)");
    ok(&e, "EntrustAdmin", Some(&alice), json!({"target": bob_cm}), None).await;
    assert_eq!(sql_one(&e, &format!("SELECT count(*) FROM club_member WHERE club_id = '{club_id}' AND role = 'ADMIN'")).await, json!(1));
    let c = e.pool.get().await.expect("conn");
    let two_admins = c.execute(format!("UPDATE club_member SET role = 'ADMIN' WHERE id = '{alice_cm}'").as_str(), &[]).await;
    assert!(two_admins.is_err(), "the database itself refuses a second ADMIN");

    // --- erasing the ADMIN repairs the club: the oldest remaining ACTIVE member becomes ADMIN
    ok(&e, "Unregister", Some(&bob), json!({}), None).await;
    let admins = sql_one(&e, &format!("SELECT jsonb_agg(member_id) FROM club_member WHERE club_id = '{club_id}' AND role = 'ADMIN'")).await;
    assert_eq!(admins, json!([alice]), "admin succession after erase");
    assert_eq!(sql_one(&e, &format!("SELECT count(*) FROM member WHERE id = '{bob}'")).await, json!(0));
    assert_eq!(sql_one(&e, &format!("SELECT count(*) FROM apply WHERE member_id = '{bob}'")).await, json!(0), "on erase cascade");

    // --- capacity: at most 3 fixed notices even under concurrency
    let mut notices = Vec::new();
    for i in 0..5 {
        let n =
            ok(&e, "PostNotice", Some(&alice), json!({"club": club_id, "title": format!("n{i}"), "body": "b", "images": []}), Some(&format!("n{i}")))
                .await;
        let _ = n;
        notices.push(sql_one(&e, &format!("SELECT id FROM club_notice WHERE title = 'n{i}'")).await.as_str().expect("notice").to_string());
    }
    let mut tasks = Vec::new();
    for n in notices.clone() {
        let e2 = e.clone();
        let alice = alice.clone();
        tasks.push(tokio::spawn(async move { call(&e2, "ToggleNoticeFixed", Some(&alice), json!({"notice": n, "noticeVersion": 1}), None).await }));
    }
    let mut fixed_ok = 0;
    for t in tasks {
        if t.await.expect("join").is_ok() {
            fixed_ok += 1;
        }
    }
    assert_eq!(fixed_ok, 3);
    assert_eq!(sql_one(&e, &format!("SELECT count(*) FROM club_notice WHERE club_id = '{club_id}' AND fixed")).await, json!(3));

    // --- versioned: two editors read the same version; the second save is refused instead of overwriting
    let n0 = notices[0].clone();
    let read = ok(&e, "NoticeDetail", Some(&alice), json!({"notice": n0}), None).await;
    let v = read["version"].as_i64().expect("version");
    let missing = fails(&e, "EditNotice", Some(&alice), json!({"notice": n0, "title": "t", "body": "b"}), None).await;
    assert_eq!((missing.code.as_str(), missing.path.as_deref()), ("AIP.INPUT.INVALID", Some("noticeVersion")));
    ok(&e, "EditNotice", Some(&alice), json!({"notice": n0, "noticeVersion": v, "title": "first", "body": "b"}), None).await;
    let stale = fails(&e, "EditNotice", Some(&alice), json!({"notice": n0, "noticeVersion": v, "title": "second", "body": "b"}), None).await;
    assert_eq!(
        (stale.code.as_str(), stale.reason.as_deref(), stale.status()),
        ("AIP.CONFLICT.STALE_VERSION", Some("CLUB_NOTICE_VERSION_STALE"), 409)
    );
    let reread = ok(&e, "NoticeDetail", Some(&alice), json!({"notice": n0}), None).await;
    assert_eq!((reread["title"].as_str(), reread["version"].as_i64()), (Some("first"), Some(v + 1)));
    ok(&e, "EditNotice", Some(&alice), json!({"notice": n0, "noticeVersion": v + 1, "title": "second", "body": "b"}), None).await;

    // --- comments: only the author edits; replies stay in the same activity; blocking self is invalid
    let act = sql_one(
        &e,
        &format!("INSERT INTO club_activity (club_id, author_id, access, body) VALUES ('{club_id}', '{alice}', 'PUBLIC', 'hello') RETURNING id"),
    )
    .await
    .as_str()
    .expect("activity")
    .to_string();
    ok(&e, "PostComment", Some(&carol), json!({"activity": act, "body": "first"}), Some("cm1")).await;
    let comment = sql_one(&e, "SELECT id FROM activity_comment WHERE body = 'first'").await.as_str().expect("comment").to_string();
    let not_author = fails(&e, "EditComment", Some(&alice), json!({"comment": comment, "body": "hacked"}), None).await;
    assert_eq!(not_author.code, "AIP.AUTH.FORBIDDEN", "ariari let any member edit any comment");
    ok(&e, "EditComment", Some(&carol), json!({"comment": comment, "body": "edited"}), None).await;
    let other_act = sql_one(
        &e,
        &format!("INSERT INTO club_activity (club_id, author_id, access, body) VALUES ('{club_id}', '{alice}', 'PUBLIC', 'other') RETURNING id"),
    )
    .await
    .as_str()
    .expect("activity2")
    .to_string();
    let cross = fails(&e, "PostComment", Some(&carol), json!({"activity": other_act, "parent": comment, "body": "reply"}), Some("cm2")).await;
    assert_eq!(cross.reason.as_deref(), Some("SAME_ACTIVITY"), "a reply must stay in its parent's activity");
    let c = e.pool.get().await.expect("conn");
    let self_block = c.execute(format!("INSERT INTO block (blocker_id, blocked_id) VALUES ('{carol}', '{carol}')").as_str(), &[]).await;
    assert!(self_block.is_err());

    // --- finance: running balance per club
    for (i, amt) in ["5000", "-1500.25", "2000"].iter().enumerate() {
        ok(&e, "AddFinancialRecord", Some(&alice), json!({"club": club_id, "at": (chrono::Utc::now() + chrono::Duration::minutes(i as i64)).to_rfc3339(), "amount": amt, "memo": format!("m{i}")}), None).await;
    }
    let ledger = ok(&e, "FinancialLedger", Some(&carol), json!({"club": club_id}), None).await;
    let balances: Vec<Value> = ledger.as_array().expect("ledger").iter().map(|r| r["balance"].clone()).collect();
    assert_eq!(balances.len(), 3);
    // Money leaves as a decimal string; the ledger is newest first, so the running balance is read bottom-up
    assert_eq!(balances, [json!("5499.75"), json!("3499.75"), json!("5000")]);
    assert_eq!(ok(&e, "ClubBalance", Some(&carol), json!({"club": club_id}), None).await["balance"], json!("5499.75"));
    // inputs that are not exact decimals never reach the database
    for bad in [json!("NaN"), json!("1e400"), json!(1.5), json!("Infinity"), json!(" 7")] {
        let input = json!({"club": club_id, "at": chrono::Utc::now().to_rfc3339(), "amount": bad, "memo": "bad"});
        let err = fails(&e, "AddFinancialRecord", Some(&alice), input, None).await;
        assert_eq!((err.code.as_str(), err.path.as_deref()), ("AIP.INPUT.INVALID", Some("amount")), "{bad}");
    }
    // 17 integer digits do not fit a double: the amount must survive storage and both read paths unchanged
    let big = "12345678901234567.89";
    let at = chrono::Utc::now() + chrono::Duration::minutes(10);
    ok(&e, "AddFinancialRecord", Some(&alice), json!({"club": club_id, "at": at.to_rfc3339(), "amount": big, "memo": "big"}), None).await;
    assert_eq!(sql_one(&e, "SELECT amount::text FROM financial_record WHERE memo = 'big'").await, json!(big));
    let ledger = ok(&e, "FinancialLedger", Some(&carol), json!({"club": club_id}), None).await;
    assert_eq!((ledger[0]["amount"].clone(), ledger[0]["balance"].clone()), (json!(big), json!("12345678901240067.64")));
    assert_eq!(ok(&e, "ClubBalance", Some(&carol), json!({"club": club_id}), None).await["balance"], json!("12345678901240067.64"));
    let big_id = sql_one(&e, "SELECT id FROM financial_record WHERE memo = 'big'").await.as_str().expect("id").to_string();
    ok(&e, "DeleteFinancialRecord", Some(&alice), json!({"record": big_id}), None).await;
    let outsider = member(&e, "dave", false).await;
    let no_finance = fails(&e, "FinancialLedger", Some(&outsider), json!({"club": club_id}), None).await;
    assert_eq!(no_finance.code, "AIP.AUTH.FORBIDDEN");

    // --- keyset pagination over 25 clubs
    for i in 0..24 {
        ok(&e, "CreateClub", Some(&carol), club_input(&format!("club{i:02}")), Some(&format!("p{i}"))).await;
    }
    let filter = json!({"filter": {"fields": [], "regions": [], "targets": [], "affiliations": []}, "sort": "OLDEST"});
    let page1 = call(&e, "ClubList", None, filter.clone(), None).await.expect("page1");
    assert_eq!(page1.data.as_array().expect("list").len(), 20);
    let cursor = page1.page.as_ref().and_then(|p| p["next_cursor"].as_str()).expect("cursor").to_string();
    let mut f2 = filter.clone();
    f2["cursor"] = json!(cursor);
    let page2 = call(&e, "ClubList", None, f2, None).await.expect("page2");
    assert_eq!(page2.data.as_array().expect("list").len(), 5);
    assert_eq!(page2.page.as_ref().expect("page")["has_more"], json!(false));
    let first_ids: Vec<&Value> = page1.data.as_array().expect("l").iter().map(|x| &x["id"]).collect();
    assert!(page2.data.as_array().expect("l").iter().all(|x| !first_ids.contains(&&x["id"])), "pages do not overlap");

    // --- every event and deferred effect is delivered; handlers ran without error
    drain(&e).await;
    let pending = sql_one(&e, "SELECT jsonb_agg(jsonb_build_object('name', name, 'error', last_error)) FROM _aip_outbox WHERE done_at IS NULL").await;
    assert_eq!(pending, Value::Null, "undelivered outbox rows: {pending}");
    // NoticePosted notified every club member (alice, carol) for each of the 5 notices
    assert_eq!(sql_one(&e, "SELECT count(*) FROM _aip_notification WHERE template = 'club-notice'").await, json!(10));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ariari_forms() {
    let e = setup_db("aip_e2e_forms").await;
    let alice = member(&e, "alice", false).await;
    let bob = member(&e, "bob", false).await;
    let eve = member(&e, "eve", false).await;
    let root = member(&e, "root", true).await;
    let _ = root;
    sql_one(&e, "INSERT INTO school (name, email_domain) VALUES ('Uni', 'uni.ac.kr') RETURNING id").await;

    // --- verification: code is mailed after commit, hashed at rest, attempts survive failures
    let bad_domain = fails(&e, "RequestSchoolEmail", Some(&bob), json!({"target": "bob@gmail.com"}), None).await;
    assert_eq!(bad_domain.reason.as_deref(), Some("VERIFICATION_TARGET_INVALID"));
    ok(&e, "RequestSchoolEmail", Some(&bob), json!({"target": "bob@uni.ac.kr"}), None).await;
    let too_soon = fails(&e, "RequestSchoolEmail", Some(&bob), json!({"target": "bob@uni.ac.kr"}), None).await;
    assert_eq!(too_soon.reason.as_deref(), Some("VERIFICATION_RESEND_TOO_SOON"));
    drain(&e).await;
    let code = sql_one(&e, "SELECT data ->> 'code' FROM _aip_mail WHERE data ->> 'to' = 'bob@uni.ac.kr'").await;
    let code = code.as_str().expect("mailed code").to_string();
    assert_eq!(code.len(), 6);
    assert_eq!(sql_one(&e, &format!("SELECT count(*) FROM _aip_verification WHERE code_hash = '{code}'")).await, json!(0), "only the hash is stored");
    let wrong = if code == "000000" { "111111" } else { "000000" };
    let mismatch = fails(&e, "VerifySchoolEmail", Some(&bob), json!({"target": "bob@uni.ac.kr", "code": wrong}), None).await;
    assert_eq!(mismatch.reason.as_deref(), Some("VERIFICATION_CODE_MISMATCH"));
    assert_eq!(sql_one(&e, "SELECT attempts_left FROM _aip_verification").await, json!(4), "the failed attempt was committed");
    let other = fails(&e, "VerifySchoolEmail", Some(&eve), json!({"target": "bob@uni.ac.kr", "code": code}), None).await;
    assert_eq!(other.reason.as_deref(), Some("VERIFICATION_NOT_FOUND"), "a code is bound to the requesting member");
    ok(&e, "VerifySchoolEmail", Some(&bob), json!({"target": "bob@uni.ac.kr", "code": code}), None).await;
    assert_eq!(sql_one(&e, &format!("SELECT s.name FROM member m JOIN school s ON s.id = m.school_id WHERE m.id = '{bob}'")).await, json!("Uni"));

    // --- grant links: one-time tokens, targeted invites, required inputs
    let club = ok(&e, "CreateClub", Some(&alice), club_input("Invite Club"), Some("ic")).await;
    let club_id = club["id"].as_str().expect("club").to_string();
    let outsider_issue = fails(&e, "IssueClubInvite", Some(&bob), json!({"club": club_id}), None).await;
    assert_eq!(outsider_issue.code, "AIP.AUTH.FORBIDDEN");
    let link = ok(&e, "IssueClubInvite", Some(&alice), json!({"club": club_id}), None).await;
    let token = link["token"].as_str().expect("token").to_string();
    let no_name = fails(&e, "RedeemClubInvite", Some(&bob), json!({"token": token}), None).await;
    assert_eq!(no_name.code, "AIP.INPUT.INVALID");
    ok(&e, "RedeemClubInvite", Some(&bob), json!({"token": token, "name": "Bobby"}), None).await;
    assert_eq!(sql_one(&e, &format!("SELECT name FROM club_member WHERE member_id = '{bob}'")).await, json!("Bobby"));
    let twice = fails(&e, "RedeemClubInvite", Some(&bob), json!({"token": token, "name": "Bobby"}), None).await;
    assert_eq!(twice.reason.as_deref(), Some("ALREADY_CLUB_MEMBER"));
    let forged = fails(&e, "RedeemClubInvite", Some(&eve), json!({"token": "deadbeef", "name": "E"}), None).await;
    assert_eq!(forged.reason.as_deref(), Some("INVALID_GRANT_LINK"));
    let direct = ok(&e, "IssueDirectInvite", Some(&alice), json!({"club": club_id, "invitee": eve}), None).await;
    let dt = direct["token"].as_str().expect("token").to_string();
    let wrong_holder = fails(&e, "RedeemDirectInvite", Some(&root), json!({"token": dt, "name": "R"}), None).await;
    assert_eq!(wrong_holder.reason.as_deref(), Some("INVALID_GRANT_LINK"), "a targeted invite only works for its invitee");
    ok(&e, "RedeemDirectInvite", Some(&eve), json!({"token": dt, "name": "Eve"}), None).await;

    // --- expose: generated CRUD with position ordering and admin-only writes
    let f1 = ok(&e, "CreateClubFaq", Some(&alice), json!({"club": club_id, "question": "Q1", "answer": "A1"}), Some("f1")).await;
    ok(&e, "CreateClubFaq", Some(&alice), json!({"club": club_id, "question": "Q2", "answer": "A2"}), Some("f2")).await;
    let not_admin = fails(&e, "CreateClubFaq", Some(&bob), json!({"club": club_id, "question": "Q3", "answer": "A3"}), Some("f3")).await;
    assert_eq!(not_admin.code, "AIP.AUTH.FORBIDDEN");
    let list = ok(&e, "ListClubFaq", None, json!({"club": club_id}), None).await;
    let qs: Vec<&str> = list.as_array().expect("faqs").iter().filter_map(|x| x["question"].as_str()).collect();
    assert_eq!(qs, vec!["Q1", "Q2"], "ordered by generated position");
    let f1_id = f1["id"].as_str().expect("faq").to_string();
    let bob_update = fails(&e, "UpdateClubFaq", Some(&bob), json!({"target": f1_id, "question": "x", "answer": "y"}), None).await;
    assert_eq!(bob_update.code, "AIP.AUTH.FORBIDDEN");
    ok(&e, "UpdateClubFaq", Some(&alice), json!({"target": f1_id, "question": "Q1!", "answer": "A1!"}), None).await;
    ok(&e, "DeleteClubFaq", Some(&alice), json!({"target": f1_id}), None).await;
    assert_eq!(ok(&e, "ListClubFaq", None, json!({"club": club_id}), None).await.as_array().expect("faqs").len(), 1);

    // --- dynamic schema: answers are validated against the form version the recruitment pinned
    let questions = json!([{"key": "why", "label": "Why", "kind": "TEXT", "required": true, "maxLength": 10, "choices": []}]);
    ok(&e, "UpdateApplyForm", Some(&alice), json!({"club": club_id, "questions": questions}), None).await;
    let r = ok(&e, "CreateRecruitment", Some(&alice), json!({"club": club_id, "input": recruitment_input(period(-1, 10))}), Some("fr1")).await;
    let r_id = r["id"].as_str().expect("r").to_string();
    ok(&e, "ApproveRecruitment", Some(&root_id(&e).await), json!({"r": r_id}), None).await;
    let carol = member(&e, "carol", false).await;
    let missing = fails(&e, "SubmitApply", Some(&carol), json!({"r": r_id, "answers": {}}), Some("s1")).await;
    assert_eq!((missing.code.as_str(), missing.path.as_deref()), ("AIP.INPUT.INVALID", Some("answers.why")));
    let long = fails(&e, "SubmitApply", Some(&carol), json!({"r": r_id, "answers": {"why": "this is far too long"}}), Some("s2")).await;
    assert_eq!(long.path.as_deref(), Some("answers.why"));
    // the club changes its form; the open recruitment keeps the version it pinned
    let newer = json!([{"key": "why", "label": "Why", "kind": "TEXT", "required": true, "choices": []}, {"key": "phone", "label": "Phone", "kind": "TEXT", "required": true, "choices": []}]);
    ok(&e, "UpdateApplyForm", Some(&alice), json!({"club": club_id, "questions": newer}), None).await;
    ok(&e, "SubmitApply", Some(&carol), json!({"r": r_id, "answers": {"why": "fun"}}), Some("s3")).await;
    assert_eq!(
        sql_one(&e, &format!("SELECT form_version FROM apply WHERE member_id = '{carol}'")).await,
        json!(2),
        "pinned to the version at recruitment time"
    );

    // --- jobs: export applicants (visibility-scoped CSV, progress, expiry, notification)
    let dan = member(&e, "dan", false).await;
    let fay = member(&e, "fay", false).await;
    ok(&e, "SubmitApply", Some(&dan), json!({"r": r_id, "answers": {"why": "dev, \"ops\""}}), Some("s4")).await;
    ok(&e, "SubmitApply", Some(&fay), json!({"r": r_id, "answers": {"why": "art"}}), Some("s5")).await;
    let arg = json!({"recruitment": r_id});
    let general = fails(&e, "ExportApplicants", Some(&bob), arg.clone(), None).await;
    assert_eq!(general.code, "AIP.AUTH.FORBIDDEN");
    let started = ok(&e, "ExportApplicants", Some(&alice), arg.clone(), None).await;
    assert_eq!(started["status"], "QUEUED");
    let again = ok(&e, "ExportApplicants", Some(&alice), arg.clone(), None).await;
    assert_eq!(again["job"], started["job"], "an identical running job is reused");
    let job = json!({"job": started["job"]});
    let foreign = fails(&e, "ExportApplicantsStatus", Some(&bob), job.clone(), None).await;
    assert_eq!(foreign.code, "AIP.NOT_FOUND", "only the requester sees a job");
    assert!(aip_runtime::jobs::tick(&e).await.expect("job tick"));
    let st = ok(&e, "ExportApplicantsStatus", Some(&alice), job.clone(), None).await;
    assert_eq!((st["status"].as_str(), st["total"].as_i64(), st["done"].as_i64()), (Some("DONE"), Some(3), Some(3)), "{st}");
    let url = st["file"].as_str().expect("file url").to_string();
    let key = url.trim_start_matches("/aip/objects/");
    let csv = std::fs::read_to_string(e.objects.root.join(key)).expect("csv file");
    let lines: Vec<&str> = csv.trim_end().split("\r\n").collect();
    assert!(lines[0].starts_with("\u{feff}id,createdAt,member,recruitment,answers"), "{}", lines[0]);
    assert_eq!(lines.len(), 4, "header + 3 applicants");
    assert!(csv.contains(&dan) && csv.contains(r#"""dev, \""ops\"""""#), "quoted JSON cell: {csv}");
    assert!(lines[1].split(',').nth(1).is_some_and(|t| t.ends_with('Z') && t.contains('T')), "times are ISO 8601 UTC: {}", lines[1]);
    let expiry = sql_one(
        &e,
        &format!("SELECT available_at > now() + interval '6 days' FROM _aip_outbox WHERE name = 's3.delete' AND payload ->> 'key' = '{key}'"),
    )
    .await;
    assert_eq!(expiry, json!(true), "the export deletes itself after 7 days");
    drain(&e).await;
    let notified =
        sql_one(&e, &format!("SELECT data ->> 'file' FROM _aip_notification WHERE recipient = '{alice}' AND template = 'export-ready'")).await;
    assert_eq!(notified, json!(url));
    let fresh = ok(&e, "ExportApplicants", Some(&alice), arg.clone(), None).await;
    assert_ne!(fresh["job"], started["job"], "a finished job does not block a new one");

    // --- jobs with a body: items changed after the snapshot are skipped, not failed
    let apply_of = |who: &str| format!("(SELECT id FROM apply WHERE member_id = '{who}')");
    let dan_apply = sql_one(&e, &format!("SELECT id FROM apply WHERE member_id = '{dan}'")).await;
    let refuse = ok(&e, "RefuseRemaining", Some(&alice), arg.clone(), None).await;
    sql_one(
        &e,
        &format!(
            "UPDATE _aip_job SET items = jsonb_build_array({}, {}, {}), total = 3 WHERE id = '{}' RETURNING id",
            apply_of(&carol),
            apply_of(&dan),
            apply_of(&fay),
            refuse["job"].as_str().expect("job")
        ),
    )
    .await;
    ok(&e, "ApproveApplies", Some(&alice), json!({"applies": [dan_apply]}), None).await;
    while aip_runtime::jobs::tick(&e).await.expect("job tick") {}
    let st = ok(&e, "RefuseRemainingStatus", Some(&alice), json!({"job": refuse["job"]}), None).await;
    assert_eq!(st["status"], "DONE", "{st}");
    let statuses = sql_one(&e, "SELECT jsonb_object_agg(m.nickname, a.status) FROM apply a JOIN member m ON m.id = a.member_id").await;
    assert_eq!(statuses, json!({"carol": "REFUSED", "dan": "APPROVED", "fay": "REFUSED"}));
}

async fn root_id(e: &Engine) -> String {
    sql_one(e, "SELECT id FROM member WHERE role = 'SUPER_ADMIN' LIMIT 1").await.as_str().expect("root").to_string()
}

fn upload(name: &str, bytes: &[u8]) -> aip_runtime::objects::Upload {
    aip_runtime::objects::Upload { filename: name.to_string(), content_type: Some("image/png".into()), bytes: bytes::Bytes::copy_from_slice(bytes) }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ariari_uploads() {
    let e = setup_db("aip_e2e_uploads").await;
    let alice = member(&e, "alice", false).await;
    let objects_before = |e: Arc<Engine>| async move { sql_one(&e, "SELECT count(*) FROM _aip_object").await };

    // create with a profile image: staged in the transaction, active after commit
    let mut uploads = std::collections::HashMap::new();
    uploads.insert("p1".to_string(), upload("logo.png", b"PNG1"));
    let mut input = club_input("Pics");
    input["profile"] = json!("p1");
    let club = e
        .call(Call {
            intent: "CreateClub".into(),
            input,
            actor: Some(alice.clone()),
            idempotency_key: Some("u1".into()),
            uploads,
            ..Default::default()
        })
        .await
        .expect("create with upload")
        .data;
    let club_id = club["id"].as_str().expect("club").to_string();
    let key1 = sql_one(&e, &format!("SELECT profile FROM club WHERE id = '{club_id}'")).await.as_str().expect("key").to_string();
    assert_eq!(sql_one(&e, &format!("SELECT state FROM _aip_object WHERE key = '{key1}'")).await, json!("active"));
    assert!(e.objects.path(&key1).expect("path").exists());

    // wrong type is rejected before anything is stored
    let mut bad = std::collections::HashMap::new();
    bad.insert("p2".to_string(), upload("virus.exe", b"MZ"));
    let mut input = json!({"club": club_id, "input": {"name": "Pics", "intro": "", "affiliation": "UNION", "field": "ACADEMIC", "region": "CAPITAL", "target": "UNDERGRADUATE"}});
    input["profile"] = json!("p2");
    let err = e
        .call(Call { intent: "UpdateClub".into(), input: input.clone(), actor: Some(alice.clone()), uploads: bad, ..Default::default() })
        .await
        .expect_err("exe");
    assert_eq!((err.code.as_str(), err.path.as_deref()), ("AIP.INPUT.INVALID", Some("profile")));
    assert_eq!(objects_before(e.clone()).await, json!(1));

    // a failed transaction leaves no staged object behind
    let mut up = std::collections::HashMap::new();
    up.insert("p3".to_string(), upload("new.png", b"PNG3"));
    let mut failing = input.clone();
    failing["input"]["affiliation"] = json!("INTERNAL");
    failing["profile"] = json!("p3");
    let err = e
        .call(Call { intent: "UpdateClub".into(), input: failing, actor: Some(alice.clone()), uploads: up, ..Default::default() })
        .await
        .expect_err("affiliation");
    assert_eq!(err.reason.as_deref(), Some("AFFILIATION_IMMUTABLE"));
    assert_eq!(objects_before(e.clone()).await, json!(1), "rolled back: nothing staged survives");

    // replacing the image: the old object is released only after commit
    let mut up = std::collections::HashMap::new();
    up.insert("p4".to_string(), upload("v2.png", b"PNG4"));
    let mut replace = input.clone();
    replace["profile"] = json!("p4");
    e.call(Call { intent: "UpdateClub".into(), input: replace, actor: Some(alice.clone()), uploads: up, ..Default::default() })
        .await
        .expect("replace");
    let key2 = sql_one(&e, &format!("SELECT profile FROM club WHERE id = '{club_id}'")).await.as_str().expect("key").to_string();
    assert_ne!(key1, key2);
    assert!(e.objects.path(&key1).expect("path").exists(), "old object still there until the outbox runs");
    drain(&e).await;
    assert!(!e.objects.path(&key1).expect("path").exists(), "old object deleted after commit");
    assert_eq!(objects_before(e.clone()).await, json!(1));

    // several images, each a separate part; per-item results
    let mut up = std::collections::HashMap::new();
    for i in 0..3 {
        up.insert(format!("img{i}"), upload(&format!("a{i}.png"), b"IMG"));
    }
    let r = e
        .call(Call {
            intent: "PostActivity".into(),
            input: json!({"club": club_id, "access": "PUBLIC", "body": "photos", "images": ["img0", "img1", "img2"]}),
            actor: Some(alice.clone()),
            idempotency_key: Some("act1".into()),
            uploads: up,
            ..Default::default()
        })
        .await
        .expect("post activity");
    assert_eq!(r.partial.as_ref().and_then(|p| p.as_array()).map(|a| a.len()), Some(3));
    assert_eq!(sql_one(&e, "SELECT count(*) FROM activity_image").await, json!(3));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ariari_approval() {
    let e = setup_db("aip_e2e_approval").await;
    let alice = member(&e, "alice", false).await;
    let bob = member(&e, "bob", false).await;
    let carol = member(&e, "carol", false).await;
    let dan = member(&e, "dan", false).await;
    let eve = member(&e, "eve", false).await;
    let club = ok(&e, "CreateClub", Some(&alice), club_input("Ledger Club"), Some("k-ledger")).await;
    let club_id = club["id"].as_str().expect("club id").to_string();
    for (who, role) in [(&bob, "MANAGER"), (&carol, "MANAGER"), (&dan, "GENERAL")] {
        sql_one(&e, &format!("INSERT INTO club_member (club_id, member_id, name, role) VALUES ('{club_id}', '{who}', 'm', '{role}') RETURNING id"))
            .await;
    }
    let record = |memo: &'static str| {
        let e = e.clone();
        let alice = alice.clone();
        let club_id = club_id.clone();
        async move {
            ok(
                &e,
                "AddFinancialRecord",
                Some(&alice),
                json!({"club": club_id, "at": chrono::Utc::now().to_rfc3339(), "amount": 10000, "memo": memo}),
                None,
            )
            .await;
            let id = sql_one(&e, &format!("SELECT id FROM financial_record WHERE memo = '{memo}'")).await;
            id.as_str().expect("record id").to_string()
        }
    };
    let settlement = |id: String| {
        let e = e.clone();
        async move { sql_one(&e, &format!("SELECT settlement FROM financial_record WHERE id = '{id}'")).await }
    };

    // --- who may request: managers only; outsiders cannot even see the record
    let r1 = record("snacks").await;
    let arg = json!({"financialRecord": r1});
    let general = fails(&e, "RequestFinancialSettlement", Some(&dan), arg.clone(), None).await;
    assert_eq!(general.code, "AIP.AUTH.FORBIDDEN");
    let outsider = fails(&e, "RequestFinancialSettlement", Some(&eve), arg.clone(), None).await;
    assert_eq!(outsider.code, "AIP.NOT_FOUND");
    let none = ok(&e, "FinancialSettlementStatus", Some(&bob), arg.clone(), None).await;
    assert_eq!(none["status"], "NONE");
    ok(&e, "RequestFinancialSettlement", Some(&alice), arg.clone(), None).await;
    let twice = fails(&e, "RequestFinancialSettlement", Some(&bob), arg.clone(), None).await;
    assert_eq!(twice.reason.as_deref(), Some("APPROVAL_ALREADY_PENDING"));

    // --- who may vote: approvers other than the requester, once each
    let own = fails(&e, "ApproveFinancialSettlement", Some(&alice), arg.clone(), None).await;
    assert_eq!(own.reason.as_deref(), Some("SELF_APPROVAL_FORBIDDEN"));
    let not_approver = fails(&e, "ApproveFinancialSettlement", Some(&dan), arg.clone(), None).await;
    assert_eq!(not_approver.reason.as_deref(), Some("NOT_AN_APPROVER"));
    let st = ok(&e, "FinancialSettlementStatus", Some(&bob), arg.clone(), None).await;
    assert_eq!((st["status"].as_str(), st["canVote"].as_bool(), st["approvals"].as_i64()), (Some("PENDING"), Some(true), Some(0)));
    let st = ok(&e, "FinancialSettlementStatus", Some(&alice), arg.clone(), None).await;
    assert_eq!(st["canVote"], json!(false), "the requester cannot vote");
    ok(&e, "ApproveFinancialSettlement", Some(&bob), json!({"financialRecord": r1, "comment": "영수증 확인"}), None).await;
    assert_eq!(settlement(r1.clone()).await, json!("OPEN"), "one approval is not enough");
    let again = fails(&e, "ApproveFinancialSettlement", Some(&bob), arg.clone(), None).await;
    assert_eq!(again.reason.as_deref(), Some("ALREADY_VOTED"));
    ok(&e, "ApproveFinancialSettlement", Some(&carol), arg.clone(), None).await;
    assert_eq!(settlement(r1.clone()).await, json!("SETTLED"));
    let st = ok(&e, "FinancialSettlementStatus", Some(&carol), arg.clone(), None).await;
    assert_eq!((st["status"].as_str(), st["approvals"].as_i64()), (Some("APPROVED"), Some(2)));
    assert_eq!(st["votes"][0]["comment"], "영수증 확인");
    let closed = fails(&e, "ApproveFinancialSettlement", Some(&carol), arg.clone(), None).await;
    assert_eq!(closed.reason.as_deref(), Some("APPROVAL_NOT_PENDING"));

    // --- a single rejection decides
    let r2 = record("taxi").await;
    ok(&e, "RequestFinancialSettlement", Some(&bob), json!({"financialRecord": r2}), None).await;
    ok(&e, "RejectFinancialSettlement", Some(&alice), json!({"financialRecord": r2, "reason": "영수증 없음"}), None).await;
    assert_eq!(settlement(r2.clone()).await, json!("DISPUTED"));

    // --- only the requester withdraws; a new request is possible afterwards
    let r3 = record("venue").await;
    let arg3 = json!({"financialRecord": r3});
    ok(&e, "RequestFinancialSettlement", Some(&alice), arg3.clone(), None).await;
    let foreign = fails(&e, "CancelFinancialSettlement", Some(&bob), arg3.clone(), None).await;
    assert_eq!(foreign.reason.as_deref(), Some("ONLY_REQUESTER_CAN_CANCEL"));
    ok(&e, "CancelFinancialSettlement", Some(&alice), arg3.clone(), None).await;
    ok(&e, "RequestFinancialSettlement", Some(&alice), arg3.clone(), None).await;

    // --- expiry: the vote fails, and the expiry itself is committed
    sql_one(
        &e,
        &format!("UPDATE _aip_approval SET expires_at = now() - interval '1 minute' WHERE subject = '{r3}' AND status = 'PENDING' RETURNING id"),
    )
    .await;
    let expired = fails(&e, "ApproveFinancialSettlement", Some(&bob), arg3.clone(), None).await;
    assert_eq!(expired.reason.as_deref(), Some("APPROVAL_EXPIRED"));
    assert_eq!(sql_one(&e, &format!("SELECT count(*) FROM _aip_approval WHERE subject = '{r3}' AND status = 'EXPIRED'")).await, json!(1));
    ok(&e, "RequestFinancialSettlement", Some(&alice), arg3.clone(), None).await;

    // --- concurrent final votes: both land, the decision runs once
    let r4 = record("books").await;
    let arg4 = json!({"financialRecord": r4});
    ok(&e, "RequestFinancialSettlement", Some(&alice), arg4.clone(), None).await;
    let (x, y) = tokio::join!(
        call(&e, "ApproveFinancialSettlement", Some(&bob), arg4.clone(), None),
        call(&e, "ApproveFinancialSettlement", Some(&carol), arg4.clone(), None)
    );
    assert!(x.is_ok() && y.is_ok(), "{:?} {:?}", x.err(), y.err());
    assert_eq!(settlement(r4.clone()).await, json!("SETTLED"));
    assert_eq!(sql_one(&e, &format!("SELECT count(*) FROM _aip_approval WHERE subject = '{r4}' AND status = 'APPROVED'")).await, json!(1));
}
