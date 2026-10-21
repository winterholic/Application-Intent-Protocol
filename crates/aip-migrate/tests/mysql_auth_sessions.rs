#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! WBS 7.2.2: login sessions with single-use refresh tokens on a real MySQL 8.4 / MariaDB 11.4: rotation, replay revoking the
//! whole session, logout, account suspension, expiry and a race between concurrent refreshes of one token.
//! Run with `-- --ignored` against a run-owned container (`scripts/mysql-live-fixture.sh`).
use aip_migrate::mysql_auth_sessions::{Consume, Error, MysqlAuthSessions, SessionPolicy};
use aip_migrate::mysql_driver::MysqlDriver;
use aip_migrate::mysql_driver::mysql_async::{TxOpts, prelude::Queryable};
use std::sync::Arc;

const META: &str = "aip_t_auth_meta";
const SOURCE: &str = "aip_t_auth_src";
const FACTS: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// The tests share one schema pair, so they must not run at the same time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Harness {
    driver: Arc<MysqlDriver>,
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

impl Harness {
    async fn new() -> Self {
        let serial = SERIAL.lock().await;
        let var = if std::env::var("AIP_TEST_JOBS_URL").is_ok() { "AIP_TEST_JOBS_URL" } else { "AIP_TEST_MYSQL_URL" };
        let url = std::env::var(var).unwrap_or_else(|_| panic!("set {var} to a run-owned MySQL 8.4 or MariaDB 11.4 container"));
        aip_test_support::validate_mysql_admin_url(&url).unwrap_or_else(|reason| panic!("{var} is unsafe: {reason}"));
        let driver = MysqlDriver::connect(&url).await.expect("admin connection");
        let mut setup = driver.pool().get_conn().await.unwrap();
        for schema in [META, SOURCE] {
            setup.query_drop(format!("DROP DATABASE IF EXISTS `{schema}`")).await.unwrap();
        }
        setup.query_drop(format!("CREATE DATABASE `{SOURCE}`")).await.unwrap();
        setup.query_drop(format!("CREATE TABLE `{SOURCE}`.`t` (id INT PRIMARY KEY) ENGINE=InnoDB")).await.unwrap();
        drop(setup);
        driver.ensure_meta_schema_bootstrap(META, SOURCE, FACTS).await.expect("meta bootstrap creates the session tables");
        Self { driver: Arc::new(driver), _serial: serial }
    }

    async fn conn(&self) -> aip_migrate::mysql_driver::mysql_async::Conn {
        self.driver.pool().get_conn().await.unwrap()
    }

    async fn cell(&self, sql: String) -> Option<String> {
        self.conn().await.query_first::<Option<String>, _>(sql).await.unwrap().flatten()
    }
}

fn store(policy: SessionPolicy) -> MysqlAuthSessions {
    MysqlAuthSessions::new(META, policy).unwrap()
}

/// One refresh as the service runs it: consume, then the next token for a fresh one, committed either way.
async fn refresh(driver: &MysqlDriver, store: &MysqlAuthSessions, token: &str) -> (Consume, Option<String>) {
    let mut conn = driver.pool().get_conn().await.unwrap();
    let mut tx = conn.start_transaction(TxOpts::default()).await.unwrap();
    let outcome = store.consume(&mut tx, token).await.unwrap();
    let next = match &outcome {
        Consume::Fresh(consumed) => Some(store.issue_next(&mut tx, consumed).await.unwrap().refresh_token),
        _ => None,
    };
    tx.commit().await.unwrap();
    (outcome, next)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn refresh_tokens_rotate_and_a_replay_revokes_the_whole_session() {
    let h = Harness::new().await;
    let s = store(SessionPolicy::default());
    let mut c = h.conn().await;
    let first = s.open(&mut c, "user-1").await.unwrap();
    assert!(s.is_active(&mut c, &first.family, "user-1").await.unwrap());
    assert!(!s.is_active(&mut c, &first.family, "user-2").await.unwrap(), "a session belongs to its own subject only");

    // Only the digest is stored: the table holds 32 bytes, never the token.
    let stored = h.cell(format!("SELECT LENGTH(token_hash) FROM `{META}`.aip_auth_refresh_tokens")).await;
    assert_eq!(stored.as_deref(), Some("32"));
    assert_eq!(
        h.cell(format!("SELECT COUNT(*) FROM `{META}`.aip_auth_refresh_tokens WHERE HEX(token_hash) = UPPER('{}')", first.refresh_token))
            .await
            .as_deref(),
        Some("0")
    );

    let (outcome, next) = refresh(&h.driver, &s, &first.refresh_token).await;
    let second = next.expect("a fresh token yields the next one");
    assert!(matches!(&outcome, Consume::Fresh(c) if c.subject == "user-1" && c.family == first.family));
    assert_ne!(second, first.refresh_token);

    // The next token keeps working, and chains.
    let (outcome, third) = refresh(&h.driver, &s, &second).await;
    assert!(matches!(outcome, Consume::Fresh(_)));
    let third = third.unwrap();

    // Replaying the first token, which the original never rejected, ends the session. The newest token dies with it.
    let (replay, none) = refresh(&h.driver, &s, &first.refresh_token).await;
    assert!(matches!(&replay, Consume::Replayed { family, subject } if *family == first.family && subject == "user-1"), "{replay:?}");
    assert!(none.is_none());
    let (after, none) = refresh(&h.driver, &s, &third).await;
    assert_eq!(after, Consume::Revoked);
    assert!(none.is_none());
    assert!(!s.is_active(&mut c, &first.family, "user-1").await.unwrap());
    assert_eq!(h.cell(format!("SELECT revoked_reason FROM `{META}`.aip_auth_sessions")).await.as_deref(), Some("replay"));

    // A token nobody issued, and values that cannot be tokens, are unknown and change nothing.
    for bogus in ["", "x", &"y".repeat(257), "00000000000000000000000000000000"] {
        assert_eq!(refresh(&h.driver, &s, bogus).await.0, Consume::Unknown, "{bogus:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn concurrent_refreshes_of_one_token_let_exactly_one_through() {
    let h = Harness::new().await;
    let s = Arc::new(store(SessionPolicy::default()));
    let mut c = h.conn().await;
    let issued = s.open(&mut c, "user-1").await.unwrap();
    drop(c);
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let (driver, s, token) = (h.driver.clone(), s.clone(), issued.refresh_token.clone());
        tasks.push(tokio::spawn(async move { refresh(&driver, &s, &token).await }));
    }
    let mut fresh = 0;
    let mut replayed = 0;
    let mut revoked = 0;
    for task in tasks {
        match task.await.unwrap().0 {
            Consume::Fresh(_) => fresh += 1,
            Consume::Replayed { .. } => replayed += 1,
            Consume::Revoked => revoked += 1,
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(fresh, 1, "a refresh token is single use even when eight requests race");
    assert_eq!(fresh + replayed + revoked, 8);
    assert!(replayed >= 1, "the losers saw a used token");
    // The loser's replay revoked the session, so the winner's new token no longer works either.
    let mut c = h.conn().await;
    assert!(!s.is_active(&mut c, &issued.family, "user-1").await.unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn logout_and_account_suspension_end_sessions_and_nothing_else() {
    let h = Harness::new().await;
    let s = store(SessionPolicy::default());
    let mut c = h.conn().await;
    let a1 = s.open(&mut c, "alice").await.unwrap();
    let a2 = s.open(&mut c, "alice").await.unwrap();
    let b1 = s.open(&mut c, "bob").await.unwrap();

    // Logout ends one session even when presented with an already used token.
    let (_, next) = refresh(&h.driver, &s, &a1.refresh_token).await;
    let next = next.unwrap();
    assert!(s.revoke_by_token(&mut c, &a1.refresh_token, "logout").await.unwrap(), "a used token still names its session");
    assert!(!s.revoke_by_token(&mut c, &a1.refresh_token, "logout").await.unwrap(), "logging out twice changes nothing");
    assert!(!s.revoke_by_token(&mut c, "unknown", "logout").await.unwrap());
    assert_eq!(refresh(&h.driver, &s, &next).await.0, Consume::Revoked, "the token the cookie held before logout is dead");
    assert!(s.is_active(&mut c, &a2.family, "alice").await.unwrap(), "her other device stays logged in");

    // Suspension ends every session of the subject, not a neighbour's.
    assert_eq!(s.revoke_subject(&mut c, "alice", "suspended").await.unwrap(), 1, "a1 was already revoked");
    assert_eq!(refresh(&h.driver, &s, &a2.refresh_token).await.0, Consume::Revoked);
    assert!(s.is_active(&mut c, &b1.family, "bob").await.unwrap());
    assert!(matches!(refresh(&h.driver, &s, &b1.refresh_token).await.0, Consume::Fresh(_)));

    // A session revoked between consume and issue_next cannot mint a token.
    let mut tx = c.start_transaction(TxOpts::default()).await.unwrap();
    let c2 = s.open(&mut tx, "carol").await.unwrap();
    let Consume::Fresh(consumed) = s.consume(&mut tx, &c2.refresh_token).await.unwrap() else { panic!("fresh") };
    s.revoke_family(&mut tx, &consumed.family, "suspended").await.unwrap();
    assert_eq!(s.issue_next(&mut tx, &consumed).await.unwrap_err(), Error::Revoked);
    tx.commit().await.unwrap();
    assert_eq!(s.open(&mut c, "",).await.unwrap_err(), Error::InvalidSubject);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "live: needs a dedicated MySQL 8.4 container (scripts/mysql-live-fixture.sh); run with -- --ignored"]
async fn tokens_expire_idle_and_sessions_expire_absolutely_and_purge_clears_them() {
    let h = Harness::new().await;
    let s = store(SessionPolicy { idle_secs: 2, absolute_secs: 4 });
    let mut c = h.conn().await;
    let idle = s.open(&mut c, "idle").await.unwrap();
    let busy = s.open(&mut c, "busy").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2300)).await;
    assert_eq!(refresh(&h.driver, &s, &idle.refresh_token).await.0, Consume::Expired, "unused for longer than the idle window");

    // Use the second session just in time; its new token cannot outlive the 4 second session.
    let tight = store(SessionPolicy { idle_secs: 3, absolute_secs: 4 });
    let other = tight.open(&mut c, "capped").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let (outcome, next) = refresh(&h.driver, &tight, &other.refresh_token).await;
    assert!(matches!(outcome, Consume::Fresh(_)));
    let next = next.unwrap();
    let remaining: i64 = h
        .cell(format!("SELECT CAST(TIMESTAMPDIFF(SECOND, UTC_TIMESTAMP(6), expires_at) AS SIGNED) FROM `{META}`.aip_auth_refresh_tokens WHERE state = 'ACTIVE' AND family = '{}'", other.family))
        .await
        .unwrap()
        .parse()
        .unwrap();
    assert!(remaining <= 2, "capped by the absolute end, not idle + now: {remaining}");
    tokio::time::sleep(std::time::Duration::from_millis(3000)).await;
    assert_eq!(refresh(&h.driver, &tight, &next).await.0, Consume::Expired);
    assert!(!tight.is_active(&mut c, &other.family, "capped").await.unwrap());
    let _ = busy;

    // Purge removes ended sessions after the grace period together with their tokens, a bounded number per call.
    assert_eq!(s.purge(&mut c, 3600, 10).await.unwrap(), 0, "nothing has been over for an hour");
    assert_eq!(s.purge(&mut c, 0, 1).await.unwrap(), 1, "the limit bounds one call");
    assert_eq!(s.purge(&mut c, 0, 10).await.unwrap(), 2);
    assert_eq!(h.cell(format!("SELECT COUNT(*) FROM `{META}`.aip_auth_refresh_tokens")).await.as_deref(), Some("0"));
    assert_eq!(h.cell(format!("SELECT COUNT(*) FROM `{META}`.aip_auth_sessions")).await.as_deref(), Some("0"));
}

#[test]
fn policy_and_schema_are_validated() {
    assert_eq!(MysqlAuthSessions::new("meta", SessionPolicy::default()).unwrap_err(), Error::InvalidSchema);
    assert_eq!(MysqlAuthSessions::new(META, SessionPolicy { idle_secs: 0, absolute_secs: 10 }).unwrap_err(), Error::InvalidPolicy);
    assert_eq!(MysqlAuthSessions::new(META, SessionPolicy { idle_secs: 10, absolute_secs: 5 }).unwrap_err(), Error::InvalidPolicy);
}
