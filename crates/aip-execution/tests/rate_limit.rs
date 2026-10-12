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
