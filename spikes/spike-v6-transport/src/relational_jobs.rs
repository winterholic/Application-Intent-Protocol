//! Adapter-owned durable execution. The transport retains authentication, contract and request limits;
//! the provider owns its database transactions, leases and worker lifetime.
use crate::auth::DurablePrincipal;
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
}
