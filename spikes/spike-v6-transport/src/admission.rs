use crate::{auth::Session, pool::ConnectionPool};
use aip_execution::rate_limit::{Decision, Policy, RateLimiter};
use sha2::{Digest, Sha256};
use std::fmt;
use tokio::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Route {
    Read,
    Write,
    Session,
    Extension,
    Operation,
    Managed,
    Objects,
    Messages,
    Rag,
    Realtime,
    Static,
    ManagedFiles,
    Other,
}

impl Route {
    pub(crate) fn from_request(path: &str, _method: &str) -> Self {
        match path {
            "/read" => Self::Read,
            "/apply" | "/status" => Self::Write,
            "/session" | "/capabilities" => Self::Session,
            "/extension" => Self::Extension,
            "/operation" => Self::Operation,
            "/events/read" => Self::Managed,
            "/events/subscribe" | "/events/socket" => Self::Realtime,
            path if path.starts_with("/objects/") => Self::Objects,
            path if path.starts_with("/messages/") => Self::Messages,
            "/rag" => Self::Rag,
            path if path.starts_with("/rag/") => Self::Rag,
            path if path.starts_with("/jobs/") || path.starts_with("/files/") => Self::Managed,
            _ => Self::Other,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Session => "session",
            Self::Extension => "extension",
            Self::Operation => "operation",
            Self::Managed => "managed",
            Self::Objects => "objects",
            Self::Messages => "messages",
            Self::Rag => "rag",
            Self::Realtime => "realtime",
            Self::Static => "static",
            Self::ManagedFiles => "managed-files",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Outcome {
    Allowed,
    Limited { retry_after_ms: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AdmissionError {
    Unavailable,
}

impl fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("rate limiter unavailable"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Admission {
    limiter: RateLimiter,
    policy: Policy,
}

impl Admission {
    pub(crate) fn new(limiter: RateLimiter, policy: Policy) -> Result<Self, AdmissionError> {
        Ok(Self { limiter, policy: policy.validate().map_err(|_| AdmissionError::Unavailable)? })
    }

    pub(crate) async fn check(&self, pool: &ConnectionPool, principal: &str, route: Route, deadline: Instant) -> Result<Outcome, AdmissionError> {
        tokio::time::timeout_at(deadline, async {
            let mut db = pool.acquire_until(deadline).await.map_err(|_| AdmissionError::Unavailable)?;
            let result = self.check_on(&mut db, principal, route).await?;
            db.release().await.map_err(|_| AdmissionError::Unavailable)?;
            Ok(result)
        })
        .await
        .map_err(|_| AdmissionError::Unavailable)?
    }

    async fn check_on(&self, db: &mut crate::pool::PooledConnection, principal: &str, route: Route) -> Result<Outcome, AdmissionError> {
        let decision = self.limiter.check(db, principal, route.key(), self.policy).await.map_err(|_| AdmissionError::Unavailable)?;
        Ok(match decision {
            Decision::Allowed => Outcome::Allowed,
            Decision::Limited { retry_after_ms } => Outcome::Limited { retry_after_ms },
        })
    }
}

pub(crate) fn principal(session: Option<&Session>) -> String {
    let Some(session) = session else { return "anonymous".to_owned() };
    if let Some(grant) = &session.durable {
        let value = format!("{}\0{}\0{}", grant.issuer, grant.subject, grant.actor_id);
        return format!("durable:{:x}", Sha256::digest(value.as_bytes()));
    }
    format!("actor:{}", session.actor_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use serde_json::json;
    use std::{
        future::Future,
        pin::Pin,
        sync::atomic::{AtomicU64, Ordering},
        sync::Arc,
        time::Duration,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::Instant;
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

    static NEXT_SCHEMA: AtomicU64 = AtomicU64::new(0);

    async fn setup() -> (crate::pool::ConnectionPool, Admission, String) {
        let schema = format!("aip_admission_{}_{}", std::process::id(), NEXT_SCHEMA.fetch_add(1, Ordering::Relaxed));
        let pool = crate::pool::ConnectionPool::new(spike_v2_read::DB_URL, crate::pool::PoolOptions { max_size: 12, ..Default::default() }).unwrap();
        let db = pool.acquire().await.unwrap();
        db.batch_execute(&format!("CREATE SCHEMA {schema}")).await.unwrap();
        db.release().await.unwrap();
        let limiter = aip_execution::rate_limit::RateLimiter::new(&schema).unwrap();
        let db = pool.acquire().await.unwrap();
        limiter.install(&*db).await.unwrap();
        db.release().await.unwrap();
        let admission = Admission::new(limiter, aip_execution::rate_limit::Policy { limit: 3, window: Duration::from_secs(60) }).unwrap();
        (pool, admission, schema)
    }

    #[tokio::test]
    async fn rate_storage_work_obeys_the_request_deadline() {
        let (pool, admission, schema) = setup().await;
        let mut blocker = pool.acquire().await.unwrap();
        let tx = blocker.transaction().await.unwrap();
        tx.batch_execute(&format!("LOCK TABLE {schema}.aip_rate_limits IN ACCESS EXCLUSIVE MODE")).await.unwrap();
        let started = Instant::now();
        let result = admission.check(&pool, "deadline", Route::Read, started + Duration::from_millis(20)).await;
        let elapsed = started.elapsed();
        tx.rollback().await.unwrap();
        blocker.batch_execute(&format!("DROP SCHEMA {schema} CASCADE")).await.unwrap();
        blocker.release().await.unwrap();
        pool.close();
        assert_eq!(result, Err(AdmissionError::Unavailable));
        assert!(elapsed < Duration::from_millis(80), "admission exceeded absolute request deadline: {elapsed:?}");
    }

    #[tokio::test]
    async fn simultaneous_connections_share_one_principal_route_quota() {
        let (pool, admission, schema) = setup().await;
        let leases = futures_util::future::join_all((0..12).map(|_| pool.acquire())).await.into_iter().map(Result::unwrap).collect::<Vec<_>>();
        let pids = futures_util::future::join_all(
            leases.iter().map(|db| async move { db.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get::<_, i32>(0) }),
        )
        .await;
        assert_eq!(pids.iter().copied().collect::<std::collections::HashSet<_>>().len(), 12, "all requests must hold distinct live DB connections");
        let barrier = Arc::new(tokio::sync::Barrier::new(12));
        let mut tasks = Vec::new();
        for mut db in leases {
            let (admission, barrier) = (Arc::new(admission.clone()), barrier.clone());
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                let result = admission.check_on(&mut db, "principal-generation-1", Route::Read).await;
                let release = db.release().await;
                (result, release)
            }));
        }
        let mut allowed = 0;
        let mut limited = 0;
        for task in tasks {
            let (result, release) = task.await.unwrap();
            assert!(release.is_ok());
            match result.unwrap() {
                Outcome::Allowed => allowed += 1,
                Outcome::Limited { retry_after_ms } => {
                    assert!((1..=60_000).contains(&retry_after_ms));
                    limited += 1;
                }
            }
        }
        assert_eq!((allowed, limited), (3, 9));
        let db = pool.acquire().await.unwrap();
        db.batch_execute(&format!(
            "DROP SCHEMA {schema} CASCADE; DROP SCHEMA IF EXISTS {} CASCADE",
            aip_execution::infrastructure_schema(&schema).unwrap()
        ))
        .await
        .unwrap();
        db.release().await.unwrap();
        pool.close();
    }

    #[tokio::test]
    async fn backend_errors_fail_closed_and_route_keys_are_bounded() {
        let (pool, admission, schema) = setup().await;
        assert_eq!(Route::from_request("/objects/0123456789abcdef", "PUT"), Route::Objects);
        assert_eq!(Route::from_request("/anything", "GET"), Route::Other);
        let db = pool.acquire().await.unwrap();
        db.batch_execute(&format!(
            "DROP SCHEMA {schema} CASCADE; DROP SCHEMA IF EXISTS {} CASCADE",
            aip_execution::infrastructure_schema(&schema).unwrap()
        ))
        .await
        .unwrap();
        db.release().await.unwrap();
        assert_eq!(
            admission.check(&pool, "principal-generation-1", Route::Read, Instant::now() + Duration::from_secs(2)).await,
            Err(AdmissionError::Unavailable)
        );
        pool.close();
    }

    struct DurableAuth;
    impl crate::auth::Authenticator for DurableAuth {
        fn verify_session<'a>(
            &'a self,
            token: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<crate::auth::Session, crate::auth::AuthError>> + Send + 'a>> {
            Box::pin(async move {
                if token != "admission-test" {
                    return Err(crate::auth::AuthError::BadSignature);
                }
                crate::auth::Session::new_durable(
                    crate::auth::DurablePrincipal {
                        issuer: "https://issuer.example".into(),
                        subject: "subject-1".into(),
                        actor_id: 42,
                        issued_at: 1,
                        min_iat: 1,
                    },
                    i64::MAX,
                )
            })
        }
    }

    async fn session_request(address: std::net::SocketAddr) -> String {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(b"POST /session HTTP/1.1\r\nauthorization: Bearer admission-test\r\ncontent-length: 2\r\n\r\n{}").await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    }

    async fn browser_socket(
        address: std::net::SocketAddr,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
        let mut request = format!("ws://{address}/events/socket").into_client_request().unwrap();
        request.headers_mut().insert("origin", format!("http://{address}").parse().unwrap());
        tokio_tungstenite::connect_async(request).await.unwrap().0
    }

    #[tokio::test]
    async fn enabled_limiter_returns_http_429_with_retry_after() {
        let (pool, _admission, schema) = setup().await;
        let infrastructure =
            crate::infrastructure::Infrastructure { rate_limit: Some(Policy { limit: 1, window: Duration::from_secs(60) }), ..Default::default() };
        let runtime = crate::server::Runtime {
            infrastructure,
            db_pool: None,
            admission: None,
            realtime_slots: tokio::sync::Semaphore::new(8),
            realtime_preauth: tokio::sync::Semaphore::new(8),
            object_slots: tokio::sync::Semaphore::new(4),
            db_url: spike_v2_read::DB_URL.into(),
            broker: None,
            enable_file_uploads: false,
            fault_injection: false,
            extensions: None,
            write_extensions: false,
            extension_slots: tokio::sync::Semaphore::new(4),
            principal_slots: crate::slots::PrincipalSlotSet::unlimited(),
            options: crate::server::ServerOptions::default(),
            authenticator: Some(Arc::new(DurableAuth)),
            deployment: Some(crate::server::DeploymentFence::new(schema.clone(), "a".repeat(64)).unwrap()),
        };
        let facts = json!({"resources":{}});
        let fingerprint = spike_v5_sdk::contract_fingerprint_with_wire(&facts, spike_v2_read::id_wire::IdWire::Legacy).into();
        let server = crate::server::listen_with_fingerprint(
            facts,
            spike_v2_read::id_wire::IdWire::Legacy,
            "127.0.0.1:0".parse().unwrap(),
            runtime,
            Some(fingerprint),
        )
        .await
        .unwrap();
        let first = session_request(server.address).await;
        let second = session_request(server.address).await;
        assert!(first.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(second.starts_with("HTTP/1.1 429 Too Many Requests\r\n"));
        assert!(second.contains("Retry-After: 60\r\n"));
        assert!(second.contains("\"code\":\"WIRE.RATE_LIMITED\""));
        server.shutdown().await.unwrap();
        let db = pool.acquire().await.unwrap();
        db.batch_execute(&format!(
            "DROP SCHEMA {schema} CASCADE; DROP SCHEMA IF EXISTS {} CASCADE",
            aip_execution::infrastructure_schema(&schema).unwrap()
        ))
        .await
        .unwrap();
        db.release().await.unwrap();
        pool.close();
    }

    #[tokio::test]
    async fn browser_websocket_is_limited_after_its_auth_frame() {
        let (pool, _admission, schema) = setup().await;
        let infrastructure =
            crate::infrastructure::Infrastructure { rate_limit: Some(Policy { limit: 1, window: Duration::from_secs(60) }), ..Default::default() };
        let runtime = crate::server::Runtime {
            infrastructure,
            db_pool: None,
            admission: None,
            realtime_slots: tokio::sync::Semaphore::new(8),
            realtime_preauth: tokio::sync::Semaphore::new(8),
            object_slots: tokio::sync::Semaphore::new(4),
            db_url: spike_v2_read::DB_URL.into(),
            broker: None,
            enable_file_uploads: false,
            fault_injection: false,
            extensions: None,
            write_extensions: false,
            extension_slots: tokio::sync::Semaphore::new(4),
            principal_slots: crate::slots::PrincipalSlotSet::unlimited(),
            options: crate::server::ServerOptions::default(),
            authenticator: Some(Arc::new(DurableAuth)),
            deployment: Some(crate::server::DeploymentFence::new(schema.clone(), "a".repeat(64)).unwrap()),
        };
        let facts = json!({"resources":{}});
        let fingerprint = spike_v5_sdk::contract_fingerprint_with_wire(&facts, spike_v2_read::id_wire::IdWire::Legacy).into();
        let server = crate::server::listen_with_fingerprint(
            facts,
            spike_v2_read::id_wire::IdWire::Legacy,
            "127.0.0.1:0".parse().unwrap(),
            runtime,
            Some(fingerprint),
        )
        .await
        .unwrap();
        let mut first = browser_socket(server.address).await;
        first.send(Message::Text(json!({"type":"auth","token":"admission-test"}).to_string().into())).await.unwrap();
        let first_reply = tokio::time::timeout(Duration::from_secs(2), first.next()).await.unwrap().unwrap().unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&first_reply.into_text().unwrap()).unwrap(), json!({"type":"authenticated"}));
        drop(first);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut second = browser_socket(server.address).await;
        second.send(Message::Text(json!({"type":"auth","token":"admission-test"}).to_string().into())).await.unwrap();
        let second_reply = tokio::time::timeout(Duration::from_secs(2), second.next()).await.unwrap().unwrap().unwrap();
        let second_reply: serde_json::Value = serde_json::from_str(&second_reply.into_text().unwrap()).unwrap();
        assert_eq!(second_reply["type"], "error");
        assert_eq!(second_reply["code"], "WIRE.RATE_LIMITED");
        drop(second);
        server.shutdown().await.unwrap();
        let db = pool.acquire().await.unwrap();
        db.batch_execute(&format!(
            "DROP SCHEMA {schema} CASCADE; DROP SCHEMA IF EXISTS {} CASCADE",
            aip_execution::infrastructure_schema(&schema).unwrap()
        ))
        .await
        .unwrap();
        db.release().await.unwrap();
        pool.close();
    }

    #[tokio::test]
    async fn rate_limit_rejects_prototype_without_durable_auth_at_startup() {
        let infrastructure =
            crate::infrastructure::Infrastructure { rate_limit: Some(Policy { limit: 1, window: Duration::from_secs(60) }), ..Default::default() };
        let runtime = crate::server::Runtime {
            infrastructure,
            db_pool: None,
            admission: None,
            realtime_slots: tokio::sync::Semaphore::new(8),
            realtime_preauth: tokio::sync::Semaphore::new(8),
            object_slots: tokio::sync::Semaphore::new(4),
            db_url: spike_v2_read::DB_URL.into(),
            broker: None,
            enable_file_uploads: false,
            fault_injection: false,
            extensions: None,
            write_extensions: false,
            extension_slots: tokio::sync::Semaphore::new(4),
            principal_slots: crate::slots::PrincipalSlotSet::unlimited(),
            options: crate::server::ServerOptions::default(),
            authenticator: None,
            deployment: None,
        };
        let result = crate::server::listen_with_fingerprint(
            json!({"schema":"aip_prototype","resources":{}}),
            spike_v2_read::id_wire::IdWire::Legacy,
            "127.0.0.1:0".parse().unwrap(),
            runtime,
            None,
        )
        .await;
        match result {
            Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput),
            Ok(server) => panic!("unexpected server startup: {}", server.address),
        }
    }

    #[test]
    fn principal_quota_stays_stable_across_session_generation_changes() {
        let make_session = |min_iat, actor_id| {
            crate::auth::Session::new_durable(
                crate::auth::DurablePrincipal {
                    issuer: "https://issuer.example".into(),
                    subject: "subject-1".into(),
                    actor_id,
                    issued_at: min_iat,
                    min_iat,
                },
                i64::MAX,
            )
            .unwrap()
        };
        let current = make_session(1, 42);
        let renewed = make_session(10, 42);
        let other_account = make_session(1, 43);
        assert_eq!(principal(Some(&current)), principal(Some(&renewed)));
        assert_ne!(principal(Some(&current)), principal(Some(&other_account)));
    }
}
