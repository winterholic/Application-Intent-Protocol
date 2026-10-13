use aip_execution::rate_limit::{Decision, Policy, RateLimiter};
use std::time::Duration;

#[tokio::test]
async fn separate_connections_share_an_atomic_limit_and_expired_windows_reset() {
    let (mut db, driver) = tokio_postgres::connect("host=localhost dbname=postgres", tokio_postgres::NoTls).await.unwrap();
    let task = tokio::spawn(driver);
    let schema = format!("aip_rate_{}", std::process::id());
    db.batch_execute(&format!("CREATE SCHEMA {schema}")).await.unwrap();
    let limiter = RateLimiter::new(&schema).unwrap();
    limiter.install(&db).await.unwrap();
    let policy = Policy { limit: 3, window: Duration::from_secs(60) };
    for _ in 0..3 {
        assert_eq!(limiter.check(&mut db, "principal-a", "read", policy).await.unwrap(), Decision::Allowed);
    }
    let (mut other, other_driver) = tokio_postgres::connect("host=localhost dbname=postgres", tokio_postgres::NoTls).await.unwrap();
    let other_task = tokio::spawn(other_driver);
    assert!(matches!(limiter.check(&mut other, "principal-a", "read", policy).await.unwrap(), Decision::Limited { retry_after_ms: 1..=60000 }));
    assert_eq!(limiter.check(&mut other, "principal-b", "read", policy).await.unwrap(), Decision::Allowed);
    assert_eq!(limiter.check(&mut other, "principal-a", "write", policy).await.unwrap(), Decision::Allowed);
    db.batch_execute(&format!("UPDATE {schema}.aip_rate_limits SET reset_at=clock_timestamp()-interval '1 second'")).await.unwrap();
    assert_eq!(limiter.check(&mut db, "principal-a", "read", policy).await.unwrap(), Decision::Allowed);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    task.abort();
    other_task.abort();
}

#[test]
fn invalid_policies_and_schema_are_rejected() {
    assert!(RateLimiter::new("bad;sql").is_err());
    assert!(Policy { limit: 0, window: Duration::from_secs(1) }.validate().is_err());
    assert!(Policy { limit: 1, window: Duration::ZERO }.validate().is_err());
}

async fn full_limiter(label: &str, reset_at: &str) -> (tokio_postgres::Client, tokio::task::JoinHandle<()>, RateLimiter, String) {
    let (db, driver) = tokio_postgres::connect("host=localhost dbname=postgres", tokio_postgres::NoTls).await.unwrap();
    let task = tokio::spawn(async move {
        let _ = driver.await;
    });
    let schema = format!("aip_rate_{label}_{}", std::process::id());
    db.batch_execute(&format!("CREATE SCHEMA {schema}")).await.unwrap();
    let limiter = RateLimiter::new(&schema).unwrap();
    limiter.install(&db).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.aip_rate_limits(scope,used,reset_at) SELECT 'filler-' || g, 1, {reset_at} FROM generate_series(1,100000) g"
    ))
    .await
    .unwrap();
    (db, task, limiter, schema)
}

/// Windows that already ended carry no state, so they must not count toward the scope quota; a limiter with
/// only minute-long windows would otherwise reject every new principal once 100000 scopes passed through in a day.
#[tokio::test]
async fn expired_windows_do_not_keep_new_principals_out_at_the_scope_limit() {
    let (mut db, task, limiter, schema) = full_limiter("expired", "clock_timestamp() - interval '2 minutes'").await;
    let policy = Policy { limit: 3, window: Duration::from_secs(60) };
    assert_eq!(limiter.check(&mut db, "new-principal", "read", policy).await.unwrap(), Decision::Allowed);
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    task.abort();
}

/// When every scope is still active the oldest ones are evicted instead of refusing a new principal with a 503.
#[tokio::test]
async fn the_oldest_scopes_are_evicted_when_every_scope_is_active() {
    let (mut db, task, limiter, schema) = full_limiter("active", "clock_timestamp() + interval '1 minute' + g * interval '1 millisecond'").await;
    let policy = Policy { limit: 3, window: Duration::from_secs(60) };
    assert_eq!(limiter.check(&mut db, "new-principal", "read", policy).await.unwrap(), Decision::Allowed);
    let rows: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.aip_rate_limits"), &[]).await.unwrap().get(0);
    assert!(rows <= 100_000, "the table stays within the scope quota, has {rows} rows");
    let evicted: i64 = db.query_one(&format!("SELECT count(*) FROM {schema}.aip_rate_limits WHERE scope='filler-1'"), &[]).await.unwrap().get(0);
    assert_eq!(evicted, 0, "the scope that resets soonest is the one removed");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
    task.abort();
}
