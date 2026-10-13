//! RAG database leases backed by the bounded, TLS-policy-aware service pool.

use crate::pool::{ConnectionPool, PoolOptions, PooledConnection};
use aip_execution::rag::{PgAcquireFuture, PgConnectionFactory, PgConnectionLease, RagError, RagFuture};
use serde_json::{json, Value};
use std::{io, sync::Arc};
use tokio_postgres::Client;

/// A dedicated bounded pool for RAG storage. It follows the same URL/TLS policy
/// as the service pool, while keeping RAG connection use isolated and capped.
#[derive(Clone)]
pub struct RagConnectionFactory {
    pool: Arc<ConnectionPool>,
}

impl RagConnectionFactory {
    pub fn new(database_url: impl Into<Arc<str>>, options: PoolOptions) -> io::Result<Self> {
        let pool = ConnectionPool::new(database_url, options)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid RAG database pool options"))?;
        Ok(Self { pool: Arc::new(pool) })
    }
}

struct RagConnection(PooledConnection);

impl PgConnectionLease for RagConnection {
    fn client(&mut self) -> &mut Client {
        &mut self.0
    }

    fn release(self: Box<Self>) -> RagFuture<'static, ()> {
        let Self(connection) = *self;
        Box::pin(async move { connection.release().await.map_err(|_| RagError::Storage) })
    }
}

impl PgConnectionFactory for RagConnectionFactory {
    fn acquire<'a>(&'a self) -> PgAcquireFuture<'a> {
        let pool = self.pool.clone();
        Box::pin(async move {
            let connection = pool.acquire().await.map_err(|_| RagError::Storage)?;
            Ok(Box::new(RagConnection(connection)) as Box<dyn PgConnectionLease>)
        })
    }
}

fn exact_keys(body: &Value, required: &[&str], optional: &[&str]) -> bool {
    let Some(object) = body.as_object() else { return false };
    required.iter().all(|key| object.contains_key(*key))
        && object.keys().all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
}

fn err(error: RagError) -> Value {
    let code = match error {
        RagError::Forbidden => "WIRE.FORBIDDEN",
        RagError::InvalidInput => "WIRE.BAD_REQUEST",
        RagError::NotFound => "WIRE.NOT_FOUND",
        RagError::LimitExceeded => "WIRE.LIMIT_EXCEEDED",
        RagError::Provider => "WIRE.PROVIDER_UNAVAILABLE",
        RagError::Storage => "WIRE.DB_UNAVAILABLE",
        RagError::Timeout => "WIRE.REQUEST_TIMEOUT",
    };
    json!({"ok":false,"code":code})
}

fn top_k(body: &Value) -> Result<usize, RagError> {
    match body.get("topK") {
        None => Ok(5),
        Some(value) => value.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or(RagError::InvalidInput),
    }
}

/// Handle the four fixed RAG routes. Owner scope is always derived from the
/// product durable-principal validator; callers cannot provide an owner.
pub async fn handle(
    engine: &aip_execution::rag::RagEngine,
    principal: &str,
    path: &str,
    body: &Value,
    client_index: bool,
    contract_fingerprint: &str,
) -> Value {
    use aip_execution::rag::{AnswerRequest, SearchRequest};
    match path {
        "/rag/search" if exact_keys(body, &["corpus", "query"], &["topK", "minScore"]) => {
            let (Some(corpus), Some(query)) = (body["corpus"].as_str(), body["query"].as_str()) else { return err(RagError::InvalidInput) };
            let top_k = match top_k(body) {
                Ok(value) => value,
                Err(error) => return err(error),
            };
            let min_score = match body.get("minScore") {
                None => None,
                Some(value) => match value.as_f64().filter(|score| score.is_finite()).map(|score| score as f32) {
                    Some(score) => Some(score),
                    None => return err(RagError::InvalidInput),
                },
            };
            match engine.search(SearchRequest { principal, corpus, query, top_k, min_score }).await {
                Ok(hits) => {
                    let hits = hits
                        .into_iter()
                        .map(|hit| {
                            json!({
                                "text":hit.text, "score":hit.score, "citation":{
                                    "sourceId":hit.citation.source_id, "documentId":hit.citation.document_id,
                                    "chunkIndex":hit.citation.chunk_index, "source":hit.citation.source
                                }
                            })
                        })
                        .collect::<Vec<_>>();
                    json!({"ok":true,"hits":hits,"contractFingerprint":contract_fingerprint})
                }
                Err(error) => err(error),
            }
        }
        "/rag/answer" if exact_keys(body, &["corpus", "question"], &["topK", "minScore"]) => {
            let (Some(corpus), Some(question)) = (body["corpus"].as_str(), body["question"].as_str()) else { return err(RagError::InvalidInput) };
            let top_k = match top_k(body) {
                Ok(value) => value,
                Err(error) => return err(error),
            };
            let min_score = match body.get("minScore") {
                None => None,
                Some(value) => match value.as_f64().filter(|score| score.is_finite()).map(|score| score as f32) {
                    Some(score) => Some(score),
                    None => return err(RagError::InvalidInput),
                },
            };
            match engine.answer(AnswerRequest { principal, corpus, question, top_k, min_score }).await {
                Ok(result) => {
                    let citations = result
                        .citations
                        .into_iter()
                        .map(|citation| {
                            json!({
                                "sourceId":citation.source_id, "documentId":citation.document_id,
                                "chunkIndex":citation.chunk_index, "source":citation.source
                            })
                        })
                        .collect::<Vec<_>>();
                    json!({"ok":true,"answer":result.answer,"citations":citations,"contractFingerprint":contract_fingerprint})
                }
                Err(error) => err(error),
            }
        }
        "/rag/index" if client_index && exact_keys(body, &["corpus", "documentId", "source", "text"], &[]) => {
            let (Some(corpus), Some(document), Some(source), Some(text)) =
                (body["corpus"].as_str(), body["documentId"].as_str(), body["source"].as_str(), body["text"].as_str())
            else {
                return err(RagError::InvalidInput);
            };
            match engine.index_document(principal, corpus, principal, document, source, text).await {
                Ok(chunks) => json!({"ok":true,"chunks":chunks,"contractFingerprint":contract_fingerprint}),
                Err(error) => err(error),
            }
        }
        "/rag/delete" if client_index && exact_keys(body, &["corpus", "documentId"], &[]) => {
            let (Some(corpus), Some(document)) = (body["corpus"].as_str(), body["documentId"].as_str()) else { return err(RagError::InvalidInput) };
            match engine.delete_document(principal, corpus, principal, document).await {
                Ok(deleted) => json!({"ok":true,"deleted":deleted,"contractFingerprint":contract_fingerprint}),
                Err(error) => err(error),
            }
        }
        "/rag/index" | "/rag/delete" if !client_index => json!({"ok":false,"code":"WIRE.NOT_AVAILABLE"}),
        _ => json!({"ok":false,"code":"WIRE.NOT_FOUND"}),
    }
}
