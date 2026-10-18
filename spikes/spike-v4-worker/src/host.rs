//! 관계형 DB 및 capability를 worker와 연결하는 호스트 인터페이스 (WBS 7.2.3 Slice13-A).
//!
//! 기존 PostgreSQL 클라이언트(`Client`)뿐만 아니라 MySQL 등 임의의 [`RelationalBackend`]에서도
//! 공식 Python 확장 워커가 정책 제어된 읽기(`ctx.read` / `ctx.aggregate`)와 관리형 Mongo/Redis 접근을
//! 안전하게 수행할 수 있도록 추상화한다.

use crate::capability::{MongoCapability, RedisCapability};
use serde_json::Value;
use spike_v2_read::backend::RelationalBackend;
use spike_v2_read::id_wire::IdWire;
use spike_v2_read::plan::{Plan, Reject};
use spike_v2_read::sqlgen::Dialect;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio_postgres::Client;

pub type HostFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 워커의 ctx IPC 호출(`aggregate`, `read`, `mongo`, `redis`)을 처리하는 호스트 인터페이스.
pub trait WorkerHost: Send {
    /// 데이터베이스 방언 (Postgres vs MySQL 등)
    fn dialect(&self) -> Dialect;

    /// 계획된 쿼리 실행
    fn execute<'a>(&'a mut self, plan: &'a Plan, wire: IdWire) -> HostFuture<'a, Result<Vec<Value>, Reject>>;

    /// 관리형 Mongo 접근
    fn mongo<'a>(&'a mut self, collection: &'a str, input: &'a Value) -> HostFuture<'a, Result<Value, Reject>> {
        let _ = (collection, input);
        Box::pin(async { Err(Reject { code: "WIRE.CAPABILITY_DENIED", msg: "Mongo capability 미구성".into() }) })
    }

    /// 관리형 Redis 접근
    fn redis<'a>(&'a mut self, key: &'a str, input: &'a Value) -> HostFuture<'a, Result<Value, Reject>> {
        let _ = (key, input);
        Box::pin(async { Err(Reject { code: "WIRE.CAPABILITY_DENIED", msg: "Redis capability 미구성".into() }) })
    }
}

/// 기존 PostgreSQL 전용 어댑터 (기존 경로 100% 무회귀 보장).
pub struct PgWorkerHost<'a> {
    pub client: &'a mut Client,
}

impl<'a> WorkerHost for PgWorkerHost<'a> {
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn execute<'b>(&'b mut self, plan: &'b Plan, _wire: IdWire) -> HostFuture<'b, Result<Vec<Value>, Reject>> {
        Box::pin(async move {
            spike_v2_read::execute(self.client, plan).await
        })
    }
}

impl WorkerHost for Client {
    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }

    fn execute<'b>(&'b mut self, plan: &'b Plan, _wire: IdWire) -> HostFuture<'b, Result<Vec<Value>, Reject>> {
        Box::pin(async move {
            spike_v2_read::execute(self, plan).await
        })
    }
}

/// MySQL 등 임의의 [`RelationalBackend`]를 위한 호스트 어댑터.
pub struct RelationalWorkerHost<'a> {
    pub backend: &'a (dyn RelationalBackend + 'a),
    pub mongo: Option<Arc<dyn MongoCapability>>,
    pub redis: Option<Arc<dyn RedisCapability>>,
}

impl<'a> RelationalWorkerHost<'a> {
    pub fn new(backend: &'a (dyn RelationalBackend + 'a)) -> Self {
        Self {
            backend,
            mongo: None,
            redis: None,
        }
    }

    pub fn with_capabilities(
        backend: &'a (dyn RelationalBackend + 'a),
        mongo: Option<Arc<dyn MongoCapability>>,
        redis: Option<Arc<dyn RedisCapability>>,
    ) -> Self {
        Self {
            backend,
            mongo,
            redis,
        }
    }
}

impl<'a> WorkerHost for RelationalWorkerHost<'a> {
    fn dialect(&self) -> Dialect {
        self.backend.dialect()
    }

    fn execute<'b>(&'b mut self, plan: &'b Plan, wire: IdWire) -> HostFuture<'b, Result<Vec<Value>, Reject>> {
        Box::pin(async move {
            self.backend.read(plan, wire).await
        })
    }

    fn mongo<'b>(&'b mut self, collection: &'b str, input: &'b Value) -> HostFuture<'b, Result<Value, Reject>> {
        Box::pin(async move {
            match &self.mongo {
                Some(m) => m.call(collection, input).await,
                None => Err(Reject { code: "WIRE.CAPABILITY_DENIED", msg: "Mongo capability 미구성".into() }),
            }
        })
    }

    fn redis<'b>(&'b mut self, key: &'b str, input: &'b Value) -> HostFuture<'b, Result<Value, Reject>> {
        Box::pin(async move {
            match &self.redis {
                Some(r) => r.call(key, input).await,
                None => Err(Reject { code: "WIRE.CAPABILITY_DENIED", msg: "Redis capability 미구성".into() }),
            }
        })
    }
}
