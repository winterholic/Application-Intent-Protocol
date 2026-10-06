//! End-to-end for `encrypted` fields against PostgreSQL: what the database holds is ciphertext bound to its row, the
//! API answers plaintext, a ciphertext moved to another row or field is refused, keys rotate, and the copies of a row
//! (published version, history versions, job export) follow the same rules.
//!
//! Requires a local PostgreSQL; set AIP_TEST_ADMIN_URL to override `postgres://localhost/postgres`.
//! Every key here is made from random bytes inside the tests; none is written down.

mod common;

use aip_runtime::crypto::{self, Keys};
use aip_runtime::engine::Engine;
use common::{drain, fails, ok, random_key, sql_one};
use serde_json::{Value, json};
use std::process::Command;
use std::sync::Arc;

const APP: &str = include_str!("encrypt_app.aip");
const APP_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/encrypt_app.aip");

fn compile(src: &str) -> (aip_ir::Program, aip_plan::Program) {
    let checked = aip_sema::pipeline::check_source(src);
    assert!(!checked.has_errors(), "{:?}", checked.diagnostics);
    let (core, map) = checked.into_core().expect("no errors");
    let compiled = aip_pg::compile(&core, &map);
    assert!(!compiled.diagnostics.iter().any(|d| d.is_error()), "{:?}", compiled.diagnostics);
    (core, compiled.program)
}

async fn setup(db: &str) -> Arc<Engine> {
    common::setup_program(db, compile(APP).1).await
}

/// The same database and program, served by a process that has these keys.
fn with_keys(e: &Engine, spec: &str) -> Engine {
    Engine {
        program: e.program.clone(),
        pool: e.pool.clone(),
        objects: aip_runtime::objects::ObjectStore::new(std::env::temp_dir().join("aip-e2e-objects")),
        secret: e.secret.clone(),
        outbound: e.outbound.clone(),
        keys: Some(Arc::new(Keys::parse(spec).expect("keys"))),
    }
}

fn key_spec(entries: &[(&str, &str)]) -> String {
    entries.iter().map(|(id, k)| format!("{id}:{k}")).collect::<Vec<_>>().join(",")
}

async fn register(e: &Engine, nick: &str, email: Option<&str>, phone: Option<&str>) -> String {
    let v = ok(e, "Register", None, json!({"nickname": nick, "email": email, "phone": phone}), Some(&format!("reg-{nick}"))).await;
    v["id"].as_str().expect("member id").to_string()
}

async fn raw(e: &Engine, q: &str) -> String {
    sql_one(e, q).await.as_str().map(String::from).unwrap_or_default()
}

/// Every table's text the application keeps, as one string.
async fn dump(e: &Engine) -> String {
    let c = e.pool.get().await.expect("conn");
    let tables: Vec<String> =
        c.query("SELECT tablename FROM pg_tables WHERE schemaname = 'public'", &[]).await.expect("tables").iter().map(|r| r.get(0)).collect();
    let mut out = String::new();
    for t in tables {
        let rows = c.query(&format!("SELECT x::text FROM \"{t}\" x"), &[]).await.expect("rows");
        for r in rows {
            out.push_str(&r.get::<_, String>(0));
            out.push('\n');
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_database_holds_ciphertext_and_the_api_answers_plaintext() {
    let e = setup("aip_enc_basic").await;
    let alice = register(&e, "alice", Some("alice@example.com"), Some("01012345678")).await;
    let bob = register(&e, "bob", Some("bob@example.com"), None).await;

    // the stored value is the versioned envelope, never the plaintext
    let stored = raw(&e, &format!("SELECT email FROM member WHERE id = '{alice}'")).await;
    assert!(stored.starts_with("v1:t1:"), "{stored}");
    assert!(!stored.contains("alice") && !stored.contains("example.com"), "{stored}");
    let all = dump(&e).await;
    for plain in ["alice@example.com", "bob@example.com", "01012345678"] {
        assert!(!all.contains(plain), "{plain} is readable somewhere in the database:\n{all}");
    }
    assert_eq!(sql_one(&e, &format!("SELECT phone IS NULL FROM member WHERE id = '{bob}'")).await, json!(true), "an absent value stays NULL");

    // the owner reads her own values; others get the mask (email) or nothing (phone is visible to self)
    let me = ok(&e, "Me", Some(&alice), json!({"m": alice}), None).await;
    assert_eq!((me["email"].as_str(), me["phone"].as_str()), (Some("alice@example.com"), Some("01012345678")), "{me}");
    let list = ok(&e, "Members", Some(&alice), json!({}), None).await;
    let by_nick = |n: &str| list.as_array().expect("list").iter().find(|m| m["nickname"] == n).cloned().expect("member");
    assert_eq!(by_nick("alice")["email"], "alice@example.com");
    assert_eq!(by_nick("bob")["email"], "b***@example.com", "the mask is applied after decrypting");
    assert_eq!(by_nick("bob")["phone"], Value::Null, "visible to self still applies");
    assert_eq!(by_nick("bob")["id"], bob);

    // the command that inserted the row answers like a query would, and a replay answers the same
    let first = ok(&e, "Register", None, json!({"nickname": "carol", "email": "carol@example.com", "phone": null}), Some("reg-carol")).await;
    assert_eq!(first["email"], "c***@example.com");
    let replay = ok(&e, "Register", None, json!({"nickname": "carol", "email": "carol@example.com", "phone": null}), Some("reg-carol")).await;
    assert_eq!(replay, first, "an idempotent replay decrypts the kept (encrypted) response again");
    let kept = raw(&e, "SELECT response::text FROM _aip_idempotency WHERE key = 'reg-carol'").await;
    assert!(!kept.contains("carol@example.com") && kept.contains("v1:t1:"), "the kept response stays encrypted: {kept}");
    let another = fails(&e, "Register", None, json!({"nickname": "carol", "email": "other@example.com", "phone": null}), Some("reg-carol")).await;
    assert_eq!(another.code, "AIP.IDEMPOTENCY.KEY_REUSED", "a different email under the same key is still told apart");

    // an update encrypts for the row it changes, with a fresh nonce
    let before = raw(&e, &format!("SELECT email FROM member WHERE id = '{alice}'")).await;
    let changed = ok(&e, "ChangeEmail", Some(&alice), json!({"m": alice, "email": "alice@example.org"}), None).await;
    assert_eq!(changed["email"], "alice@example.org");
    let after = raw(&e, &format!("SELECT email FROM member WHERE id = '{alice}'")).await;
    assert_ne!(before, after);
    assert!(!after.contains("example.org"));
    ok(&e, "ChangeContact", Some(&alice), json!({"m": alice, "email": "same@example.org", "phone": "01099998888"}), None).await;
    let me = ok(&e, "Me", Some(&alice), json!({"m": alice}), None).await;
    assert_eq!((me["email"].as_str(), me["phone"].as_str()), (Some("same@example.org"), Some("01099998888")));

    // the audit log of a superuser call does not keep what was bound for an encrypted field
    let root = sql_one(&e, "INSERT INTO member (nickname, role) VALUES ('root', 'SUPER_ADMIN') RETURNING id").await;
    let root = root.as_str().expect("root id").to_string();
    ok(&e, "ChangeEmail", Some(&root), json!({"m": alice, "email": "audited@example.org"}), None).await;
    assert_eq!(sql_one(&e, "SELECT input ->> 'email' FROM _aip_audit WHERE intent = 'ChangeEmail'").await, json!("[encrypted]"));
    let all = dump(&e).await;
    for plain in ["alice@example.org", "same@example.org", "audited@example.org", "01099998888", "carol@example.com"] {
        assert!(!all.contains(plain), "{plain} is readable somewhere in the database");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ciphertext_moved_to_another_row_or_field_is_refused() {
    let e = setup("aip_enc_aad").await;
    let alice = register(&e, "alice", Some("alice@example.com"), Some("01012345678")).await;
    let bob = register(&e, "bob", Some("bob@example.com"), Some("01087654321")).await;
    ok(&e, "Me", Some(&alice), json!({"m": alice}), None).await;

    // bob's ciphertext pasted into alice's row
    sql_one(&e, &format!("UPDATE member SET email = (SELECT email FROM member WHERE id = '{bob}') WHERE id = '{alice}' RETURNING id")).await;
    let err = fails(&e, "Me", Some(&alice), json!({"m": alice}), None).await;
    assert_eq!(err.code, "AIP.ENCRYPTION.DECRYPT_FAILED", "{err}");
    assert_eq!(err.status(), 500);
    assert!(!err.to_string().contains("bob@example.com") && !err.to_string().contains("alice@example.com"), "{err}");
    // the list fails as a whole rather than showing a wrong value
    let err = fails(&e, "Members", Some(&alice), json!({}), None).await;
    assert_eq!(err.code, "AIP.ENCRYPTION.DECRYPT_FAILED");

    // alice's own email moved into her phone column (another field, same row)
    sql_one(&e, &format!("UPDATE member SET email = NULL, phone = (SELECT email FROM member WHERE id = '{bob}') WHERE id = '{alice}' RETURNING id"))
        .await;
    let err = fails(&e, "Me", Some(&alice), json!({"m": alice}), None).await;
    assert_eq!(err.code, "AIP.ENCRYPTION.DECRYPT_FAILED");

    // a value that is not ciphertext at all (written around the runtime) is refused too, not shown
    sql_one(&e, &format!("UPDATE member SET email = 'alice@example.com', phone = NULL WHERE id = '{alice}' RETURNING id")).await;
    let err = fails(&e, "Me", Some(&alice), json!({"m": alice}), None).await;
    assert_eq!(err.code, "AIP.ENCRYPTION.DECRYPT_FAILED");
    assert!(err.message.contains("not in the encrypted format"), "{err}");
}

#[test]
fn a_server_without_keys_does_not_start() {
    let (_, program) = compile(APP);
    for spec in [None, Some(""), Some("   "), Some("k1"), Some("k1:not base64!"), Some("k1:c2hvcnQ=")] {
        let err = crypto::load(&program, spec).expect_err("unusable keys");
        assert_eq!(err.code, "AIP.ENCRYPTION.KEYS_MISSING", "{spec:?}");
    }
    assert!(crypto::load(&program, Some(&key_spec(&[("k1", &random_key())]))).expect("usable").is_some());
    let (_, plain) = compile("entity Note { text: Text }");
    assert!(crypto::load(&plain, None).expect("no encrypted field, no keys needed").is_none());

    // the real binary: no keys in the environment, nothing is served, and the database is not even reached
    let bin = env!("CARGO_BIN_EXE_aip");
    let out = Command::new(bin)
        .args(["run", APP_PATH, "--dev-auth", "--port", "0"])
        .env("DATABASE_URL", "postgres://127.0.0.1:1/none")
        .env_remove(crypto::ENV_KEYS)
        .output()
        .expect("run aip");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && err.contains("AIP.ENCRYPTION.KEYS_MISSING") && err.contains("AIP_ENCRYPTION_KEYS"), "{err}");
    let out = Command::new(bin)
        .args(["run", APP_PATH, "--dev-auth", "--port", "0"])
        .env("DATABASE_URL", "postgres://127.0.0.1:1/none")
        .env(crypto::ENV_KEYS, "k1:AAAA")
        .output()
        .expect("run aip");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && err.contains("AIP.ENCRYPTION.KEYS_MISSING") && err.contains("32"), "a key of the wrong size: {err}");
}

async fn encrypted_columns_are_under(e: &Engine, key_id: &str) {
    let prefix = format!("v1:{key_id}:");
    for q in [
        "SELECT count(*) FROM member WHERE phone IS NOT NULL",
        "SELECT count(*) FROM post",
        "SELECT count(*) FROM post_published",
        "SELECT count(*) FROM diary",
        "SELECT count(*) FROM diary_history",
    ] {
        assert!(sql_one(e, q).await.as_i64().is_some_and(|n| n > 0), "the check below means nothing without rows: {q}");
    }
    for (what, q) in [
        ("member.email", "SELECT count(*) FROM member WHERE email IS NOT NULL AND NOT starts_with(email, '#')"),
        ("member.phone", "SELECT count(*) FROM member WHERE phone IS NOT NULL AND NOT starts_with(phone, '#')"),
        ("post.body", "SELECT count(*) FROM post WHERE NOT starts_with(body, '#')"),
        ("post_published.body", "SELECT count(*) FROM post_published WHERE NOT starts_with(body, '#')"),
        ("diary.body", "SELECT count(*) FROM diary WHERE NOT starts_with(body, '#')"),
        ("diary_history.data", "SELECT count(*) FROM diary_history WHERE NOT starts_with(data ->> 'body', '#')"),
    ] {
        let stale = sql_one(e, &q.replace('#', &prefix)).await;
        assert_eq!(stale, json!(0), "{what} still holds values that are not under key {key_id}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keys_rotate_and_rekey_reaches_every_copy() {
    let (k1, k2) = (random_key(), random_key());
    let e = setup("aip_enc_rotate").await;
    let old = with_keys(&e, &key_spec(&[("k1", &k1)]));
    let alice = register(&old, "alice", Some("alice@example.com"), Some("01012345678")).await;
    let _bob = register(&old, "bob", Some("bob@example.com"), None).await;
    let post =
        ok(&old, "AddPost", Some(&alice), json!({"title": "t", "body": "first body"}), Some("p1")).await["id"].as_str().expect("post").to_string();
    ok(&old, "PublishPost", Some(&alice), json!({"post": post}), None).await;
    let diary = ok(&old, "AddDiary", Some(&alice), json!({"body": "day one"}), Some("d1")).await["id"].as_str().expect("diary").to_string();
    ok(&old, "EditDiary", Some(&alice), json!({"d": diary, "body": "day two"}), None).await;
    assert_eq!(sql_one(&old, "SELECT count(*) FROM diary_history").await, json!(2), "two versions are kept");
    let (version, touched) = {
        let r = sql_one(&old, &format!("SELECT jsonb_build_array(version, updated_at) FROM diary WHERE id = '{diary}'")).await;
        (r[0].clone(), r[1].clone())
    };

    // new first, old second: the old values still read, new writes use k2
    let both = with_keys(&e, &key_spec(&[("k2", &k2), ("k1", &k1)]));
    let me = ok(&both, "Me", Some(&alice), json!({"m": alice}), None).await;
    assert_eq!(me["email"], "alice@example.com", "a value written under the old key reads while both keys are configured");
    ok(&both, "ChangeEmail", Some(&alice), json!({"m": alice, "email": "alice@example.org"}), None).await;
    assert!(raw(&both, &format!("SELECT email FROM member WHERE id = '{alice}'")).await.starts_with("v1:k2:"), "the first key encrypts");
    assert!(raw(&both, "SELECT phone FROM member WHERE phone IS NOT NULL").await.starts_with("v1:k1:"), "untouched values keep their key");

    // rekey from the command line, keys from the environment like a deployment
    let admin = std::env::var("AIP_TEST_ADMIN_URL").unwrap_or_else(|_| "postgres://localhost/postgres".into());
    let url = admin.rsplit_once('/').map(|(base, _)| format!("{base}/aip_enc_rotate")).expect("url");
    let rekey = || {
        Command::new(env!("CARGO_BIN_EXE_aip"))
            .args(["rekey", APP_PATH])
            .env("DATABASE_URL", &url)
            .env(crypto::ENV_KEYS, key_spec(&[("k2", &k2), ("k1", &k1)]))
            .output()
            .expect("run aip rekey")
    };
    let out = rekey();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "{text}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("current key: k2") && text.contains("Member.phone in member: 1 re-encrypted"), "{text}");
    encrypted_columns_are_under(&e, "k2").await;
    let r = sql_one(&e, &format!("SELECT jsonb_build_array(version, updated_at) FROM diary WHERE id = '{diary}'")).await;
    assert_eq!((&r[0], &r[1]), (&version, &touched), "re-encrypting is not an edit: no new version, no touched timestamp");
    assert_eq!(sql_one(&e, "SELECT count(*) FROM diary_history").await, json!(2), "and no new history version");

    // running it again changes nothing
    let again = String::from_utf8_lossy(&rekey().stdout).to_string();
    assert!(again.contains("Member.phone in member: 0 re-encrypted") && !again.contains("unreadable (key"), "{again}");

    // k1 can be retired: k2 alone reads everything, the published copy and the history included
    let only_new = with_keys(&e, &key_spec(&[("k2", &k2)]));
    let me = ok(&only_new, "Me", Some(&alice), json!({"m": alice}), None).await;
    assert_eq!((me["email"].as_str(), me["phone"].as_str()), (Some("alice@example.org"), Some("01012345678")));
    let posts = ok(&only_new, "Posts", None, json!({}), None).await;
    assert_eq!(posts[0]["body"], "first body", "{posts}");
    let drafts = ok(&only_new, "DraftPosts", Some(&alice), json!({}), None).await;
    assert_eq!(drafts[0]["body"], "first body");
    let keys = Keys::parse(&key_spec(&[("k2", &k2)])).expect("keys");
    let versions = sql_one(&e, &format!("SELECT jsonb_agg(data ->> 'body' ORDER BY version) FROM diary_history WHERE id = '{diary}'")).await;
    let plain: Vec<String> = versions
        .as_array()
        .expect("versions")
        .iter()
        .map(|c| keys.decrypt("Diary.body", &diary, c.as_str().expect("ciphertext")).expect("a history copy decrypts like its row"))
        .collect();
    assert_eq!(plain, ["day one", "day two"]);
    // and the old key alone no longer can
    let err = fails(&old, "Me", Some(&alice), json!({"m": alice}), None).await;
    assert_eq!(err.code, "AIP.ENCRYPTION.DECRYPT_FAILED");
    assert!(err.message.contains("k2"), "the message names the key that is missing: {err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn published_and_history_copies_hold_the_same_ciphertext() {
    let e = setup("aip_enc_copies").await;
    let alice = register(&e, "alice", Some("alice@example.com"), None).await;
    let post =
        ok(&e, "AddPost", Some(&alice), json!({"title": "t", "body": "secret draft"}), Some("p1")).await["id"].as_str().expect("post").to_string();
    assert_eq!(ok(&e, "Posts", None, json!({}), None).await, json!([]), "nothing is published yet");
    ok(&e, "PublishPost", Some(&alice), json!({"post": post}), None).await;
    let (work, copy) = (
        raw(&e, &format!("SELECT body FROM post WHERE id = '{post}'")).await,
        raw(&e, &format!("SELECT body FROM post_published WHERE id = '{post}'")).await,
    );
    assert_eq!(work, copy, "publishing copies the ciphertext as it is");
    assert!(!work.contains("secret draft"));
    let posts = ok(&e, "Posts", None, json!({}), None).await;
    assert_eq!((posts[0]["body"].as_str(), posts[0]["owner"]["nickname"].as_str()), (Some("secret draft"), Some("alice")), "{posts}");
    // the draft moves on; readers keep the published text, the editor sees the draft
    ok(&e, "EditPost", Some(&alice), json!({"post": post, "body": "newer draft"}), None).await;
    assert_eq!(ok(&e, "Posts", None, json!({}), None).await[0]["body"], "secret draft");
    assert_eq!(ok(&e, "DraftPosts", Some(&alice), json!({}), None).await[0]["body"], "newer draft");

    // history versions are JSON documents of the row: the ciphertext in them opens with the row's id
    let diary = ok(&e, "AddDiary", Some(&alice), json!({"body": "monday"}), Some("d1")).await["id"].as_str().expect("diary").to_string();
    ok(&e, "EditDiary", Some(&alice), json!({"d": diary, "body": "tuesday"}), None).await;
    let versions = sql_one(&e, &format!("SELECT jsonb_agg(data ->> 'body' ORDER BY version) FROM diary_history WHERE id = '{diary}'")).await;
    let keys = e.keys.as_ref().expect("keys");
    let plain: Vec<String> = versions
        .as_array()
        .expect("versions")
        .iter()
        .map(|c| keys.decrypt("Diary.body", &diary, c.as_str().expect("text")).expect("opens"))
        .collect();
    assert_eq!(plain, ["monday", "tuesday"]);
    assert!(!dump(&e).await.contains("monday"), "no version is kept in the clear");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_export_decrypts_what_the_requester_may_see() {
    let e = setup("aip_enc_export").await;
    register(&e, "alice", Some("alice@example.com"), Some("01012345678")).await;
    register(&e, "bob", None, None).await;
    let root = sql_one(&e, "INSERT INTO member (nickname, role) VALUES ('root', 'SUPER_ADMIN') RETURNING id").await;
    let root = root.as_str().expect("root").to_string();
    let started = ok(&e, "ExportMembers", Some(&root), json!({}), None).await;
    assert!(aip_runtime::jobs::tick(&e).await.expect("job tick"));
    drain(&e).await;
    let st = ok(&e, "ExportMembersStatus", Some(&root), json!({"job": started["job"]}), None).await;
    assert_eq!(st["status"], "DONE", "{st}");
    let key = st["file"].as_str().expect("file").trim_start_matches("/aip/objects/").to_string();
    let csv = std::fs::read_to_string(e.objects.root.join(key)).expect("csv");
    assert!(csv.contains("alice@example.com"), "the email is decrypted for the file: {csv}");
    assert!(!csv.contains("v1:t1:"), "no ciphertext leaks into the file: {csv}");
    assert!(!csv.contains("01012345678"), "a field that is visible to self only is left out of an export, encrypted or not: {csv}");
}

#[test]
fn the_contract_does_not_tell_clients_about_encryption() {
    let (core, plan) = compile(APP);
    let contract = aip_contract::describe(&core).to_string().to_lowercase();
    let ts = aip_contract::gen_ts(&core).to_lowercase();
    assert!(!contract.contains("encrypt") && !ts.contains("encrypt"), "encryption is the server's business; the shape of a value is unchanged");
    // while the plan, which only the runtime reads, says where to decrypt
    assert!(plan.entities["Member"].columns.iter().any(|c| c.field == "email" && c.encrypted));
}

#[test]
fn turning_encryption_on_or_off_or_renaming_an_encrypted_field_is_refused() {
    let plain = APP.replace("email: Email? personal encrypted masked", "email: Email? personal masked");
    let (old_core, _) = compile(&plain);
    let (new_core, new_plan) = compile(APP);
    let p = aip_pg::evolve::plan(&old_core, &new_core, &new_plan.ddl);
    assert!(p.rejections.iter().any(|r| r.code == "AIP.SCHEMA.ENCRYPTION_CHANGE" && r.message.contains("Member.email became encrypted")), "{p:?}");
    let (_, plain_plan) = compile(&plain);
    let p = aip_pg::evolve::plan(&new_core, &old_core, &plain_plan.ddl);
    assert!(p.rejections.iter().any(|r| r.code == "AIP.SCHEMA.ENCRYPTION_CHANGE" && r.message.contains("stopped being encrypted")), "{p:?}");
    // a rename changes the name the ciphertext is bound to
    let renamed = APP
        .replace("email", "mail")
        .replace("mask.mail", "mask.email")
        .replace("mail: Email? personal encrypted", "mail: Email? was email personal encrypted");
    let (renamed_core, renamed_plan) = compile(&renamed);
    let p = aip_pg::evolve::plan(&new_core, &renamed_core, &renamed_plan.ddl);
    assert!(p.rejections.iter().any(|r| r.code == "AIP.SCHEMA.ENCRYPTION_CHANGE" && r.message.contains("bound to the name")), "{p:?}");
    // unchanged: nothing refused
    assert!(aip_pg::evolve::plan(&new_core, &new_core, &new_plan.ddl).rejections.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_server_that_lost_its_keys_fails_writes_instead_of_storing_plaintext() {
    let e = setup("aip_enc_nokeys").await;
    let keyless = Engine {
        program: e.program.clone(),
        pool: e.pool.clone(),
        objects: aip_runtime::objects::ObjectStore::new(std::env::temp_dir().join("aip-e2e-objects")),
        secret: e.secret.clone(),
        outbound: e.outbound.clone(),
        keys: None,
    };
    let err = fails(&keyless, "Register", None, json!({"nickname": "dan", "email": "dan@example.com", "phone": null}), Some("k")).await;
    assert_eq!(err.code, "AIP.ENCRYPTION.KEYS_MISSING");
    assert!(!dump(&e).await.contains("dan@example.com"));
}
