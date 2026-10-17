//! 관계형 저장소 어댑터 경계. 전송 계층은 이 trait만 보고, PostgreSQL 이외의 저장소 구현은 해당 어댑터 crate가 넣는다.
//!
//! 어댑터는 요청마다 배포 펜스를 통과한 읽기 전용 트랜잭션 하나에서 계획을 실행한다. 이 trait에 없는 능력(쓰기, 작업, 파일 등)은
//! 전송 계층이 조용히 흉내 내지 않고 `WIRE.ADAPTER_UNSUPPORTED`로 거부한다.
use crate::{
    id_wire::IdWire,
    plan::{Caller, Plan, Reject},
    sqlgen::Dialect,
};
use serde_json::{json, Value};
use std::{future::Future, pin::Pin};

pub type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait RelationalBackend: Send + Sync {
    /// 계획 생성에 쓸 SQL 방언.
    fn dialect(&self) -> Dialect;

    /// 공개 진단에 쓰는 어댑터 이름(`mysql` 등).
    fn adapter(&self) -> &'static str;

    /// `/ready`용 검사. 연결과 배포 marker·구조 지문을 확인하고, 실패하면 공개 오류 코드를 돌려준다.
    fn ready(&self) -> BackendFuture<'_, Result<(), &'static str>>;

    /// 요청 시각. 정책 식의 `now`는 항상 DB가 정한 UTC 시각 하나로 바인딩한다. 형식은 계획 바인딩이 받는 ISO-8601 UTC 문자열이다.
    fn now(&self) -> BackendFuture<'_, Result<String, &'static str>>;

    /// 읽기 계획 하나를 펜스된 읽기 전용 트랜잭션에서 실행하고, 계획 출력 타입에 맞게 정규화한 행 JSON을 돌려준다.
    fn read<'a>(&'a self, plan: &'a Plan, wire: IdWire) -> BackendFuture<'a, Result<Vec<serde_json::Value>, Reject>>;

    /// `/apply`: one write request (`body` is the whole request body with `key` and `request`) in one transaction behind the
    /// deployment fence. The answer is the wire response itself. An adapter without a write path refuses by name.
    fn apply<'a>(&'a self, _facts: &'a Value, _body: &'a Value, _caller: &'a Caller, _wire: IdWire) -> BackendFuture<'a, Value> {
        let adapter = self.adapter();
        Box::pin(async move {
            json!({"ok": false, "code": "WIRE.ADAPTER_UNSUPPORTED", "msg": format!("{adapter} adapter는 /apply를 아직 지원하지 않음")})
        })
    }

    /// `/status`: the stored result of a write key.
    fn status<'a>(&'a self, _body: &'a Value, _caller: &'a Caller, _wire: IdWire) -> BackendFuture<'a, Value> {
        let adapter = self.adapter();
        Box::pin(async move {
            json!({"ok": false, "code": "WIRE.ADAPTER_UNSUPPORTED", "msg": format!("{adapter} adapter는 /status를 아직 지원하지 않음")})
        })
    }
}
