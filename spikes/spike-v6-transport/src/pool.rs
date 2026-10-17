use spike_v2_read::OwnedConnection;
use std::{
    collections::VecDeque,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    time::Instant,
};
use tokio_postgres::Client;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PoolOptions {
    pub max_size: usize,
    pub acquire_timeout: Duration,
    pub idle_timeout: Duration,
}

impl Default for PoolOptions {
    fn default() -> Self {
        Self { max_size: 16, acquire_timeout: Duration::from_secs(2), idle_timeout: Duration::from_secs(60) }
    }
}

impl PoolOptions {
    pub fn validate(self) -> Result<Self, &'static str> {
        if self.max_size == 0 || self.max_size > Semaphore::MAX_PERMITS || self.acquire_timeout.is_zero() || self.idle_timeout.is_zero() {
            return Err("database pool size and timeouts must be positive and bounded");
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoolError {
    Timeout,
    Closed,
    InvalidOptions,
    Connect,
    Reset,
}

struct IdleConnection {
    since: Instant,
    connection: OwnedConnection,
}

struct PoolInner {
    url: Arc<str>,
    options: PoolOptions,
    checkouts: Arc<Semaphore>,
    idle: Mutex<VecDeque<IdleConnection>>,
    closed: AtomicBool,
}

#[derive(Clone)]
pub struct ConnectionPool(Arc<PoolInner>);

impl ConnectionPool {
    pub fn new(url: impl Into<Arc<str>>, options: PoolOptions) -> Result<Self, PoolError> {
        let options = options.validate().map_err(|_| PoolError::InvalidOptions)?;
        Ok(Self(Arc::new(PoolInner {
            url: url.into(),
            options,
            checkouts: Arc::new(Semaphore::new(options.max_size)),
            idle: Mutex::new(VecDeque::new()),
            closed: AtomicBool::new(false),
        })))
    }

    pub async fn acquire(&self) -> Result<PooledConnection, PoolError> {
        self.acquire_until(Instant::now() + self.0.options.acquire_timeout).await
    }

    pub async fn acquire_until(&self, deadline: Instant) -> Result<PooledConnection, PoolError> {
        if self.0.closed.load(Ordering::Acquire) {
            return Err(PoolError::Closed);
        }
        let deadline = deadline.min(Instant::now() + self.0.options.acquire_timeout).min(Instant::now() + crate::server::DB_CONNECT_TIMEOUT);
        if Instant::now() >= deadline {
            return Err(PoolError::Timeout);
        }
        let permit = tokio::time::timeout_at(deadline, self.0.checkouts.clone().acquire_owned())
            .await
            .map_err(|_| PoolError::Timeout)?
            .map_err(|_| PoolError::Closed)?;
        if Instant::now() >= deadline {
            drop(permit);
            return Err(PoolError::Timeout);
        }
        if self.0.closed.load(Ordering::Acquire) {
            return Err(PoolError::Closed);
        }

        loop {
            let idle = {
                let mut idle = self.0.idle.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                idle.pop_front()
            };
            let Some(idle) = idle else { break };
            if Instant::now().duration_since(idle.since) < self.0.options.idle_timeout {
                return Ok(PooledConnection { connection: Some(idle.connection), pool: self.0.clone(), permit: Some(permit) });
            }
            drop(idle);
        }

        if Instant::now() >= deadline {
            return Err(PoolError::Timeout);
        }
        let connection = tokio::time::timeout_at(deadline, spike_v2_read::connect_owned_with_url(&self.0.url))
            .await
            .map_err(|_| PoolError::Timeout)?
            .map_err(|_| PoolError::Connect)?;
        if self.0.closed.load(Ordering::Acquire) {
            drop(connection);
            return Err(PoolError::Closed);
        }
        Ok(PooledConnection { connection: Some(connection), pool: self.0.clone(), permit: Some(permit) })
    }

    pub fn close(&self) {
        if self.0.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.0.checkouts.close();
        self.0.idle.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    }
}

pub struct PooledConnection {
    connection: Option<OwnedConnection>,
    pool: Arc<PoolInner>,
    permit: Option<OwnedSemaphorePermit>,
}

impl PooledConnection {
    pub async fn release(mut self) -> Result<(), PoolError> {
        let Some(connection) = self.connection.take() else { return Err(PoolError::Closed) };
        if self.pool.closed.load(Ordering::Acquire) {
            drop(connection);
            return Err(PoolError::Closed);
        }

        let reset = tokio::time::timeout(self.pool.options.acquire_timeout, async {
            connection.batch_execute("ROLLBACK").await?;
            connection.batch_execute("DISCARD ALL").await
        })
        .await;
        match reset {
            Ok(Ok(())) => {}
            _ => {
                drop(connection);
                return Err(PoolError::Reset);
            }
        }
        connection.clear_type_cache();

        let mut idle = self.pool.idle.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.pool.closed.load(Ordering::Acquire) {
            drop(idle);
            drop(connection);
            return Err(PoolError::Closed);
        }
        idle.push_back(IdleConnection { since: Instant::now(), connection });
        drop(idle);
        self.permit.take();
        Ok(())
    }
}

impl Deref for PooledConnection {
    type Target = Client;
    fn deref(&self) -> &Self::Target {
        self.connection.as_ref().expect("released connection lease")
    }
}

impl DerefMut for PooledConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection.as_mut().expect("released connection lease")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn pool_options_have_bounded_defaults() {
        let options = PoolOptions::default();
        assert_eq!(options.max_size, 16);
        assert_eq!(options.acquire_timeout, Duration::from_secs(2));
        assert_eq!(options.idle_timeout, Duration::from_secs(60));
        assert!(PoolOptions { max_size: 0, ..options }.validate().is_err());
    }

    #[tokio::test]
    async fn closed_pool_and_expired_deadline_reject_checkout() {
        let pool = ConnectionPool::new(aip_test_support::test_database_url!(), PoolOptions::default()).unwrap();
        assert!(matches!(pool.acquire_until(Instant::now()).await, Err(PoolError::Timeout)));
        pool.close();
        assert!(matches!(pool.acquire().await, Err(PoolError::Closed)));
    }

    #[tokio::test]
    async fn release_reuses_connection_and_clears_session_state() {
        let pool = ConnectionPool::new(aip_test_support::test_database_url!(), PoolOptions::default()).unwrap();
        let first = pool.acquire().await.unwrap();
        let pid: i32 = first.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
        let baseline_app_name: String = first.query_one("SELECT current_setting('application_name')", &[]).await.unwrap().get(0);
        first.batch_execute("SET application_name = 'aip-pool-test'; SELECT pg_advisory_lock(84721, 192); BEGIN").await.unwrap();
        first.release().await.unwrap();

        let second = pool.acquire().await.unwrap();
        let row =
            second.query_one("SELECT pg_backend_pid(), current_setting('application_name'), pg_try_advisory_lock(84721, 192)", &[]).await.unwrap();
        assert_eq!(row.get::<_, i32>(0), pid, "healthy connection should be reused");
        assert_eq!(row.get::<_, String>(1), baseline_app_name, "session settings must not leak between leases");
        assert!(row.get::<_, bool>(2), "session advisory locks must not leak between leases");
        second.query_one("SELECT pg_advisory_unlock(84721, 192)", &[]).await.unwrap();
        second.release().await.unwrap();
    }

    #[tokio::test]
    async fn checkout_is_bounded_by_pool_capacity_and_acquire_deadline() {
        let pool = ConnectionPool::new(
            aip_test_support::test_database_url!(),
            PoolOptions { max_size: 1, acquire_timeout: Duration::from_millis(30), ..PoolOptions::default() },
        )
        .unwrap();
        // 첫 checkout이 새 연결을 만들면 연결 시간이 30ms 기한 안에 들어가는지에 시험이 기대게 된다. 부하가 있으면 첫 호출이 먼저 Timeout이 난다.
        // 연결을 idle에 미리 넣어 첫 checkout이 연결을 만들지 않게 하고, 두 번째 호출만 30ms 기한을 받게 한다.
        warm(&pool).await;
        let first = pool.acquire().await.unwrap();
        let started = Instant::now();
        let result = pool.acquire().await;
        assert!(matches!(result, Err(PoolError::Timeout)), "the only slot is held, so the second checkout cannot succeed");
        assert!(started.elapsed() >= Duration::from_millis(30), "the second checkout must wait for its acquire deadline, not fail early");
        drop(first);
    }

    /// 새 연결을 만들어 idle 목록에 넣는다. 기한이 짧은 pool 시험의 첫 checkout이 연결 시간에 좌우되지 않게 한다.
    async fn warm(pool: &ConnectionPool) {
        let connection = spike_v2_read::connect_owned_with_url(&pool.0.url).await.unwrap();
        pool.0.idle.lock().unwrap().push_back(IdleConnection { since: Instant::now(), connection });
    }

    #[tokio::test]
    async fn concurrent_checkouts_use_distinct_bounded_connections() {
        let pool = ConnectionPool::new(aip_test_support::test_database_url!(), PoolOptions { max_size: 2, ..PoolOptions::default() }).unwrap();
        let (first, second) = tokio::join!(pool.acquire(), pool.acquire());
        let first = first.unwrap();
        let second = second.unwrap();
        let first_pid: i32 = first.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
        let second_pid: i32 = second.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
        assert_ne!(first_pid, second_pid);
        assert!(matches!(pool.acquire().await, Err(PoolError::Timeout)));
        first.release().await.unwrap();
        second.release().await.unwrap();
    }

    #[tokio::test]
    async fn dropping_lease_discards_connection_instead_of_reusing_it() {
        let pool = ConnectionPool::new(aip_test_support::test_database_url!(), PoolOptions { max_size: 1, ..PoolOptions::default() }).unwrap();
        let first = pool.acquire().await.unwrap();
        let first_pid: i32 = first.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
        drop(first);

        let second = pool.acquire().await.unwrap();
        let second_pid: i32 = second.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
        assert_ne!(second_pid, first_pid, "unreleased lease must be discarded");
        second.release().await.unwrap();
    }

    #[tokio::test]
    async fn idle_timeout_evicts_old_connection_on_checkout() {
        let pool = ConnectionPool::new(
            aip_test_support::test_database_url!(),
            PoolOptions { max_size: 1, idle_timeout: Duration::from_millis(20), ..PoolOptions::default() },
        )
        .unwrap();
        let first = pool.acquire().await.unwrap();
        let first_pid: i32 = first.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
        first.release().await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;

        let second = pool.acquire().await.unwrap();
        let second_pid: i32 = second.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
        assert_ne!(second_pid, first_pid, "expired idle connection must be evicted");
        second.release().await.unwrap();
    }

    #[tokio::test]
    async fn failed_connect_does_not_consume_pool_capacity() {
        let pool = ConnectionPool::new(
            "host=127.0.0.1 port=1 user=postgres dbname=postgres",
            PoolOptions { max_size: 1, acquire_timeout: Duration::from_millis(250), ..PoolOptions::default() },
        )
        .unwrap();
        assert!(matches!(pool.acquire().await, Err(PoolError::Connect | PoolError::Timeout)));
        assert!(matches!(pool.acquire().await, Err(PoolError::Connect | PoolError::Timeout)));
    }

    #[tokio::test]
    async fn canceled_query_lease_is_discarded() {
        let pool = ConnectionPool::new(aip_test_support::test_database_url!(), PoolOptions { max_size: 1, ..PoolOptions::default() }).unwrap();
        let first = pool.acquire().await.unwrap();
        let first_pid: i32 = first.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
        let query = first.query_one("SELECT pg_sleep(10)", &[]);
        assert!(tokio::time::timeout(Duration::from_millis(20), query).await.is_err());
        drop(first);

        let second = pool.acquire().await.unwrap();
        let second_pid: i32 = second.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
        assert_ne!(second_pid, first_pid, "lease dropped during a query must not be reused");
        second.release().await.unwrap();
    }
}
