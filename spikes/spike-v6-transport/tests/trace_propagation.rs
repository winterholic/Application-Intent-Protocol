//! A request's `traceparent` reaches the outbound calls its handler makes: the embedding provider of a RAG search sees the
//! caller's trace id with a fresh span, and a request without the header gets a trace that starts at the server.
use aip_execution::rag::{CorpusSpec, EmbeddingProvider, OwnerAuthorizer, PgJsonVectorStore, RagEngine, RagFuture};
use serde_json::json;
use spike_v1_fixture::{digest, load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::Infrastructure,
    server::{listen_with_infrastructure, DeploymentFence, ServerOptions},
};
use std::{collections::HashMap, sync::Arc, sync::Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Remembers the `traceparent` an outbound call of the request would carry.
struct Spy(Arc<Mutex<Vec<Option<String>>>>);
impl EmbeddingProvider for Spy {
    fn profile_id(&self) -> String {
        "spy-v1".into()
    }
    fn embed<'a>(&'a self, inputs: &'a [String]) -> RagFuture<'a, Vec<Vec<f32>>> {
        Box::pin(async move {
            self.0.lock().expect("spy lock").push(aip_execution::trace_context::outgoing());
            Ok(inputs.iter().map(|_| vec![1.0, 0.0]).collect())
        })
    }
}

struct Auth;
impl Authenticator for Auth {
    fn verify_session<'a>(&'a self, token: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            if token != "alice" {
                return Err(AuthError::BadSignature);
            }
            Session::new_durable(DurablePrincipal { issuer: "test".into(), subject: "alice".into(), actor_id: 1, issued_at: 1, min_iat: 0 }, i64::MAX)
        })
    }
    fn verify_durable<'a>(
        &'a self,
        _grant: &'a DurablePrincipal,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async move { Ok(()) })
    }
}

async fn search(address: std::net::SocketAddr, fingerprint: &str, extra_headers: &str) {
    let mut socket = tokio::net::TcpStream::connect(address).await.expect("connect");
    let body = json!({"corpus":"manuals","query":"data"}).to_string();
    socket
        .write_all(
            format!(
                "POST /rag/search HTTP/1.1\r\nauthorization: Bearer alice\r\nx-aip-contract: {fingerprint}\r\n{extra_headers}content-length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .expect("send");
    let mut response = String::new();
    socket.read_to_string(&mut response).await.expect("read");
    assert!(response.contains("\"ok\":true"), "{response}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_trace_of_a_request_reaches_its_outbound_calls() {
    let schema = format!("aip_trace_prop_{}", std::process::id());
    sqlgen::try_set_schema(&schema).expect("schema");
    let facts = load_str(
        "actor principal\noperation read noop {\n input {}\n output { value: Int }\n allow actor != null\n effect none\n deadline 1s\n implementation \"jobs.noop\"\n}",
        Form::A,
    )
    .expect("definition")
    .execution;
    aip_migrate::init_with_wire(aip_test_support::test_database_url!(), &schema, &facts, "safe").await.expect("init");
    let db = spike_v2_read::connect_owned_with_url(aip_test_support::test_database_url!()).await.expect("db");
    db.batch_execute(&format!(
        "INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','alice',1)"
    ))
    .await
    .expect("principal");
    let seen = Arc::new(Mutex::new(Vec::new()));
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
        max_owner_chunks: None,
        max_top_k: 10,
        max_context_chars: 1024,
        max_prompt_tokens: 4096,
        max_answer_tokens: 128,
        timeout_ms: 5000,
    };
    let storage_schema = aip_execution::infrastructure_schema(&schema).expect("infra schema");
    let store = Arc::new(
        PgJsonVectorStore::with_factory(
            Arc::new(
                spike_v6_transport::rag_transport::RagConnectionFactory::new(aip_test_support::test_database_url!(), Default::default())
                    .expect("factory"),
            ),
            &storage_schema,
        )
        .expect("store"),
    );
    store.initialize().await.expect("store init");
    let engine = Arc::new(
        RagEngine::new(
            vec![spec],
            HashMap::from([("embed".into(), Arc::new(Spy(seen.clone())) as Arc<dyn EmbeddingProvider>)]),
            HashMap::new(),
            store,
            Arc::new(OwnerAuthorizer),
        )
        .expect("engine"),
    );
    let owner = digest(&json!(["test", "alice", 1, 0]));
    engine.index_document(&owner, "manuals", &owner, "doc", "manual", "private data").await.expect("index");
    seen.lock().expect("spy lock").clear();
    let server = listen_with_infrastructure(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().expect("address"),
        aip_test_support::test_database_url!().into(),
        None,
        ServerOptions::default(),
        false,
        Arc::new(Auth),
        Some(DeploymentFence::new(schema.clone(), digest(&facts)).expect("fence")),
        None,
        false,
        Infrastructure { rag: Some(engine), ..Default::default() },
    )
    .await
    .expect("server");
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, IdWire::SafeNumber);

    search(server.address, &fingerprint, "traceparent: 00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01\r\n").await;
    search(server.address, &fingerprint, "traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00\r\n").await;
    search(server.address, &fingerprint, "").await;
    search(server.address, &fingerprint, "traceparent: garbage\r\n").await;
    server.shutdown().await.expect("shutdown");
    db.batch_execute(&format!("DROP SCHEMA {schema} CASCADE; DROP SCHEMA {storage_schema} CASCADE")).await.expect("cleanup");

    let seen = seen.lock().expect("spy lock").clone();
    let sent: Vec<String> = seen.into_iter().map(|value| value.expect("every request has a trace context")).collect();
    assert_eq!(sent.len(), 4, "{sent:?}");
    let field = |value: &str, index: usize| value.split('-').nth(index).expect("field").to_owned();
    assert_eq!((field(&sent[0], 1), field(&sent[0], 3)), ("0af7651916cd43dd8448eb211c80319c".to_owned(), "01".to_owned()), "{sent:?}");
    assert_ne!(field(&sent[0], 2), "b7ad6b7169203331", "a fresh span, not the caller's");
    assert_eq!(
        (field(&sent[1], 1), field(&sent[1], 3)),
        ("4bf92f3577b34da6a3ce929d0e0e4736".to_owned(), "00".to_owned()),
        "an unsampled caller stays unsampled"
    );
    for own in [&sent[2], &sent[3]] {
        assert_eq!(field(own, 3), "00", "a trace that starts here is not sampled: {own}");
        assert!(![field(&sent[0], 1), field(&sent[1], 1)].contains(&field(own, 1)), "{own}");
    }
    assert_ne!(field(&sent[2], 1), field(&sent[3], 1), "each request without a caller trace starts its own");
}
