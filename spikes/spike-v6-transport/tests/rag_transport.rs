use aip_execution::rag::{CorpusSpec, EmbeddingProvider, MemoryVectorStore, OwnerAuthorizer, RagEngine, RagFuture};
use serde_json::{json, Value};
use spike_v6_transport::rag_transport::handle;
use std::{collections::HashMap, sync::Arc};

struct Embed;
impl EmbeddingProvider for Embed {
    fn profile_id(&self) -> String {
        "test-v1".into()
    }
    fn embed<'a>(&'a self, inputs: &'a [String]) -> RagFuture<'a, Vec<Vec<f32>>> {
        Box::pin(async move { Ok(inputs.iter().map(|_| vec![1.0, 0.0]).collect()) })
    }
}
fn engine() -> RagEngine {
    RagEngine::new(
        vec![CorpusSpec {
            name: "manuals".into(),
            embedding_provider: "embed".into(),
            answer_provider: None,
            dimensions: 2,
            chunk_chars: 128,
            overlap_chars: 0,
            max_document_bytes: 1024,
            max_document_chunks: 10,
            max_corpus_chunks: 100,
            max_top_k: 10,
            max_context_chars: 1024,
            max_prompt_tokens: 4096,
            max_answer_tokens: 128,
            timeout_ms: 1000,
        }],
        HashMap::from([("embed".into(), Arc::new(Embed) as Arc<dyn EmbeddingProvider>)]),
        HashMap::new(),
        Arc::new(MemoryVectorStore::default()),
        Arc::new(OwnerAuthorizer),
    )
    .unwrap()
}

#[tokio::test]
async fn owner_is_server_derived_and_index_requires_opt_in() {
    let engine = engine();
    let doc = json!({"corpus":"manuals","documentId":"doc","source":"manual","text":"backend operations"});
    assert_eq!(handle(&engine, "alice", "/rag/index", &doc, false, "contract").await["code"], "NOT_AVAILABLE");
    assert_eq!(handle(&engine, "alice", "/rag/index", &doc, true, "contract").await["chunks"], 1);
    let query = json!({"corpus":"manuals","query":"operations"});
    assert_eq!(handle(&engine, "alice", "/rag/search", &query, false, "contract").await["hits"].as_array().unwrap().len(), 1);
    assert!(handle(&engine, "bob", "/rag/search", &query, false, "contract").await["hits"].as_array().unwrap().is_empty());
    let mut spoof = doc;
    spoof["ownerId"] = json!("bob");
    assert_ne!(handle(&engine, "alice", "/rag/index", &spoof, true, "contract").await["ok"], true);
}

#[tokio::test]
async fn invalid_optional_top_k_is_rejected_instead_of_using_the_default() {
    let engine = engine();
    for value in [Value::Null, json!("five"), json!(-1), json!(1.5)] {
        let result = handle(&engine, "alice", "/rag/search", &json!({"corpus":"manuals","query":"test","topK":value}), false, "contract").await;
        assert_eq!(result["code"], "BAD_REQUEST", "{result}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_http_rag_rechecks_principal_after_provider_work() {
    use spike_v1_fixture::{digest, load_str, Form};
    use spike_v2_read::{id_wire::IdWire, sqlgen};
    use spike_v6_transport::{
        auth::{AuthError, Authenticator, DurablePrincipal, Session},
        infrastructure::Infrastructure,
        server::{listen_with_infrastructure, DeploymentFence, ServerOptions},
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    struct Gate {
        active: AtomicBool,
        entered: tokio::sync::Notify,
        resume: tokio::sync::Notify,
    }
    impl EmbeddingProvider for Gate {
        fn profile_id(&self) -> String {
            "gate-v1".into()
        }
        fn embed<'a>(&'a self, inputs: &'a [String]) -> RagFuture<'a, Vec<Vec<f32>>> {
            Box::pin(async move {
                if self.active.load(Ordering::SeqCst) {
                    self.entered.notify_one();
                    self.resume.notified().await;
                }
                Ok(inputs.iter().map(|_| vec![1.0, 0.0]).collect())
            })
        }
    }
    struct Auth(Arc<AtomicBool>);
    impl Authenticator for Auth {
        fn verify_session<'a>(
            &'a self,
            token: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
            Box::pin(async move {
                if token != "alice" {
                    return Err(AuthError::BadSignature);
                }
                Session::new_durable(
                    DurablePrincipal { issuer: "test".into(), subject: "alice".into(), actor_id: 1, issued_at: 1, min_iat: 0 },
                    i64::MAX,
                )
            })
        }
        fn verify_durable<'a>(
            &'a self,
            _grant: &'a DurablePrincipal,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AuthError>> + Send + 'a>> {
            Box::pin(async move {
                if self.0.load(Ordering::SeqCst) {
                    Err(AuthError::BadSignature)
                } else {
                    Ok(())
                }
            })
        }
    }
    let schema = format!("aip_rag_http_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let facts=load_str("actor principal\noperation read noop {\n input {}\n output { value: Int }\n allow actor != null\n effect none\n deadline 1s\n implementation \"jobs.noop\"\n}",Form::A).unwrap().execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','alice',1)"
    ))
    .await
    .unwrap();
    let gate = Arc::new(Gate { active: AtomicBool::new(false), entered: tokio::sync::Notify::new(), resume: tokio::sync::Notify::new() });
    let spec = CorpusSpec {
        name: "manuals".into(),
        embedding_provider: "embed".into(),
        answer_provider: None,
        dimensions: 2,
        chunk_chars: 128,
        overlap_chars: 0,
        max_document_bytes: 1024,
        max_document_chunks: 10,
        max_corpus_chunks: 100,
        max_top_k: 10,
        max_context_chars: 1024,
        max_prompt_tokens: 4096,
        max_answer_tokens: 128,
        timeout_ms: 5000,
    };
    let storage_schema = aip_execution::infrastructure_schema(&schema).unwrap();
    let store = Arc::new(
        aip_execution::rag::PgJsonVectorStore::with_factory(
            Arc::new(spike_v6_transport::rag_transport::RagConnectionFactory::new(spike_v2_read::DB_URL, Default::default()).unwrap()),
            &storage_schema,
        )
        .unwrap(),
    );
    store.initialize().await.unwrap();
    let engine = Arc::new(
        RagEngine::new(
            vec![spec],
            HashMap::from([("embed".into(), gate.clone() as Arc<dyn EmbeddingProvider>)]),
            HashMap::new(),
            store,
            Arc::new(OwnerAuthorizer),
        )
        .unwrap(),
    );
    let owner = digest(&json!(["test", "alice", 1, 0]));
    engine.index_document(&owner, "manuals", &owner, "doc", "manual", "private data").await.unwrap();
    let revoked = Arc::new(AtomicBool::new(false));
    let infrastructure = Infrastructure {
        rag: Some(engine),
        rate_limit: Some(aip_execution::rate_limit::Policy { limit: 120, window: std::time::Duration::from_secs(60) }),
        ..Default::default()
    };
    let server = listen_with_infrastructure(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        spike_v2_read::DB_URL.into(),
        None,
        ServerOptions::default(),
        false,
        Arc::new(Auth(revoked.clone())),
        Some(DeploymentFence::new(schema.clone(), digest(&facts)).unwrap()),
        None,
        false,
        infrastructure,
    )
    .await
    .unwrap();
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);
    gate.active.store(true, Ordering::SeqCst);
    let address = server.address;
    let request = tokio::spawn(async move {
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let body = json!({"corpus":"manuals","query":"data"}).to_string();
        socket
            .write_all(
                format!(
                    "POST /rag/search HTTP/1.1\r\nauthorization: Bearer alice\r\nx-aip-contract: {fingerprint}\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).await.unwrap();
        response
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), gate.entered.notified()).await.unwrap();
    revoked.store(true, Ordering::SeqCst);
    gate.resume.notify_one();
    let response = request.await.unwrap();
    server.shutdown().await.unwrap();
    aip_migrate::preflight(spike_v2_read::DB_URL, &schema, &facts).await.expect("runtime tables must not cause application catalog drift on restart");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE; DROP SCHEMA {storage_schema} CASCADE")).await.unwrap();
    let body: Value = serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["code"], "UNAUTHENTICATED", "{body}");
    assert!(body.get("hits").is_none());
}
