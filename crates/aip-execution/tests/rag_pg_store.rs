use aip_execution::rag::{PgAcquireFuture, PgConnectionFactory, PgConnectionLease, PgJsonVectorStore, RagError, RagFuture, RagStore, StoredChunk};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_postgres::Client;

struct TestFactory(String);

struct TestLease {
    client: Client,
    _driver: tokio::task::JoinHandle<()>,
}

impl PgConnectionLease for TestLease {
    fn client(&mut self) -> &mut Client {
        &mut self.client
    }

    fn release(self: Box<Self>) -> RagFuture<'static, ()> {
        Box::pin(async move {
            drop(self);
            Ok(())
        })
    }
}

impl PgConnectionFactory for TestFactory {
    fn acquire<'a>(&'a self) -> PgAcquireFuture<'a> {
        Box::pin(async move {
            let (client, connection) = tokio_postgres::connect(&self.0, tokio_postgres::NoTls).await.map_err(|_| RagError::Storage)?;
            let driver = tokio::spawn(async move {
                let _ = connection.await;
            });
            Ok(Box::new(TestLease { client, _driver: driver }) as Box<dyn PgConnectionLease>)
        })
    }
}

fn database_url() -> String {
    std::env::var("AIP_TEST_DATABASE_URL").unwrap_or_else(|_| "host=localhost dbname=postgres".to_owned())
}

fn schema() -> String {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    format!("aip_rag_test_{}_{}", std::process::id(), stamp)
}

fn chunk(owner: &str, document: &str, text: &str) -> StoredChunk {
    StoredChunk {
        corpus: "handbook".into(),
        owner_id: owner.into(),
        document_id: document.into(),
        chunk_index: 0,
        embedding_profile: "test-v1".into(),
        source: format!("https://docs.example/{document}"),
        text: text.into(),
        vector: vec![1.0, 0.0],
    }
}

#[tokio::test]
async fn postgres_store_replaces_atomically_scopes_owner_and_serializes_capacity() {
    let schema = schema();
    let store = PgJsonVectorStore::with_factory(std::sync::Arc::new(TestFactory(database_url())), &schema).unwrap();
    store.initialize().await.unwrap();

    store.replace_document(vec![chunk("alice", "doc-a", "old")], 1, 1).await.unwrap();
    store.replace_document(vec![chunk("alice", "doc-a", "new")], 1, 1).await.unwrap();
    let alice = store.scan_owner("handbook", "alice", 1).await.unwrap();
    assert_eq!(alice.len(), 1);
    assert_eq!(alice[0].text, "new", "same document replacement is idempotent");
    assert!(store.scan_owner("handbook", "bob", 1).await.unwrap().is_empty(), "SQL scan must include owner scope");

    let too_large = vec![chunk("alice", "doc-a", "replacement-1"), {
        let mut second = chunk("alice", "doc-a", "replacement-2");
        second.chunk_index = 1;
        second
    }];
    assert!(matches!(store.replace_document(too_large, 1, 1).await, Err(RagError::LimitExceeded)));
    let after_reject = store.scan_owner("handbook", "alice", 1).await.unwrap();
    assert_eq!(after_reject.len(), 1);
    assert_eq!(after_reject[0].text, "new", "failed replacement leaves prior document intact");

    assert_eq!(store.delete_document("handbook", "alice", "doc-a").await.unwrap(), 1);
    assert_eq!(store.delete_document("handbook", "alice", "doc-a").await.unwrap(), 0);
    let (left, right) = tokio::join!(
        store.replace_document(vec![chunk("alice", "doc-left", "left")], 1, 1),
        store.replace_document(vec![chunk("alice", "doc-right", "right")], 1, 1),
    );
    assert_ne!(left.is_ok(), right.is_ok(), "corpus advisory lock must prevent concurrent capacity oversubscription");
    let alice = store.scan_owner("handbook", "alice", 1).await.unwrap();
    assert_eq!(alice.len(), 1);
    assert!(store.scan_owner("handbook", "bob", 1).await.unwrap().is_empty());

    let (client, connection) = tokio_postgres::connect(&database_url(), tokio_postgres::NoTls).await.unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client.batch_execute(&format!("DROP SCHEMA \"{schema}\" CASCADE")).await.unwrap();
}
