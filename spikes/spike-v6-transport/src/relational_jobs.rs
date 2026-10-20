//! Adapter-owned durable execution. The transport retains authentication, contract and request limits;
//! the provider owns its database transactions, leases and worker lifetime.
use crate::auth::DurablePrincipal;
use crate::triggers::ServicePrincipal;
use serde_json::Value;
use spike_v2_read::backend::BackendFuture;
use std::sync::Arc;

/// Adapter requests use the transport's key grammar, including Unicode and invisible-character rejection.
pub fn valid_key(key: &str) -> bool {
    crate::valid_key(key)
}

/// Replayed business rejections keep the same public code on every database adapter.
pub fn data_error(code: &str) -> &'static str {
    crate::durable::data_error(code)
}

pub trait RelationalJobs: Send + Sync {
    fn handle<'a>(&'a self, path: &'a str, body: &'a Value, grant: &'a DurablePrincipal) -> BackendFuture<'a, Value>;
    /// Dropping this future must also stop its workers. The server owns and cancels it on shutdown.
    fn supervise(self: Arc<Self>) -> BackendFuture<'static, ()>;
    /// Accepts a job for a server trigger (inbound webhook) as the operator-bound service principal. The adapter reads the
    /// principal binding at submission time, so a revoked or disabled principal stops future triggers without a restart.
    fn submit_service<'a>(
        &'a self,
        principal: &'a ServicePrincipal,
        operation: &'a str,
        input: &'a Value,
        key: &'a str,
    ) -> BackendFuture<'a, Result<aip_execution::Run, &'static str>> {
        let _ = (principal, operation, input, key);
        Box::pin(async { Err("WIRE.NOT_AVAILABLE") })
    }
}
