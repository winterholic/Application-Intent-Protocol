//! O-05: one principal must not be able to take every shared realtime slot.
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use spike_v1_fixture::{digest, load_str, Form};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use spike_v6_transport::{
    auth::{AuthError, Authenticator, DurablePrincipal, Session},
    infrastructure::{Infrastructure, ResourceLimits},
    server::{listen_with_infrastructure, DeploymentFence, ReadExtensions, ServerOptions, WorkerLang},
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

struct Auth {
    schema: String,
    issued_at: i64,
}

impl Authenticator for Auth {
    fn verify_session<'a>(&'a self, token: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let grant = DurablePrincipal { issuer: "test".into(), subject: token.into(), actor_id: 1, issued_at: self.issued_at, min_iat: 0 };
            self.verify_durable(&grant).await?;
            Session::new_durable(grant, self.issued_at + 600)
        })
    }

    fn verify_durable<'a>(
        &'a self,
        grant: &'a DurablePrincipal,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.map_err(|_| AuthError::Unavailable)?;
            let row = db
                .query_opt(
                    &format!("SELECT enabled,min_iat FROM {}.aip_principals WHERE issuer=$1 AND subject=$2 AND actor_id=$3", self.schema),
                    &[&grant.issuer, &grant.subject, &grant.actor_id],
                )
                .await
                .map_err(|_| AuthError::Unavailable)?;
            if row.is_some_and(|row| row.get::<_, bool>(0) && row.get::<_, i64>(1) == grant.min_iat) {
                Ok(())
            } else {
                Err(AuthError::BadSignature)
            }
        })
    }
}

struct SchemaCleanup(String);
impl Drop for SchemaCleanup {
    fn drop(&mut self) {
        let schema = self.0.clone();
        let cleanup = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            runtime.block_on(async move {
                if let Ok(db) = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await {
                    let _ = db.batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).await;
                }
            });
        });
        let _ = cleanup.join();
    }
}

fn request(address: SocketAddr) -> tokio_tungstenite::tungstenite::http::Request<()> {
    let mut request = format!("ws://{address}/events/socket").into_client_request().unwrap();
    request.headers_mut().insert("origin", format!("http://{address}").parse().unwrap());
    request
}

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Connects and sends the auth frame; returns the socket and the server's first reply.
async fn authenticate(address: SocketAddr, token: &str) -> (Socket, Value) {
    let (mut socket, _) = tokio_tungstenite::connect_async(request(address)).await.unwrap();
    socket.send(Message::Text(json!({"type":"auth","token":token}).to_string().into())).await.unwrap();
    let reply = tokio::time::timeout(Duration::from_secs(5), socket.next()).await.unwrap().unwrap().unwrap();
    (socket, serde_json::from_str(&reply.into_text().unwrap()).unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_principal_cannot_take_every_realtime_slot() {
    let schema = format!("aip_principal_slots_{}", std::process::id());
    sqlgen::try_set_schema(&schema).unwrap();
    let _cleanup = SchemaCleanup(schema.clone());
    let facts = load_str(include_str!("fixtures/jobs.aip"), Form::A).unwrap().execution;
    aip_migrate::init_with_wire(spike_v2_read::DB_URL, &schema, &facts, "safe").await.unwrap();
    let db = spike_v2_read::connect_owned_with_url(spike_v2_read::DB_URL).await.unwrap();
    db.batch_execute(&format!(
        "INSERT INTO {schema}.aip_actors VALUES(1); INSERT INTO {schema}.aip_principals(issuer,subject,actor_id) VALUES('test','hog',1),('test','other',1)"
    ))
    .await
    .unwrap();
    let issued_at: i64 = db.query_one("SELECT floor(extract(epoch FROM clock_timestamp()))::bigint", &[]).await.unwrap().get(0);
    let mut infrastructure = Infrastructure::default();
    infrastructure.realtime.max_connections = 3;
    infrastructure.resource_limits = ResourceLimits { per_principal_realtime: 2, ..ResourceLimits::default() };
    let server = listen_with_infrastructure(
        facts.clone(),
        IdWire::SafeNumber,
        "127.0.0.1:0".parse().unwrap(),
        spike_v2_read::DB_URL.into(),
        Some(ReadExtensions { lang: WorkerLang::Node, dir: concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/jobs").into() }),
        ServerOptions::default(),
        false,
        Arc::new(Auth { schema: schema.clone(), issued_at }),
        Some(DeploymentFence::new(schema, digest(&facts)).unwrap()),
        None,
        false,
        infrastructure,
    )
    .await
    .unwrap();

    let (first, reply) = authenticate(server.address, "hog").await;
    assert_eq!(reply, json!({"type":"authenticated"}));
    let (second, reply) = authenticate(server.address, "hog").await;
    assert_eq!(reply, json!({"type":"authenticated"}));
    // The third connection is within the server's capacity of 3 but beyond the principal's share of 2.
    let (_third, reply) = authenticate(server.address, "hog").await;
    assert_eq!((reply["type"].as_str(), reply["code"].as_str()), (Some("error"), Some("SERVER_BUSY")), "{reply}");
    // The remaining capacity still serves everybody else.
    let (_other, reply) = authenticate(server.address, "other").await;
    assert_eq!(reply, json!({"type":"authenticated"}), "another principal was locked out by one that held every slot");
    // A connection that ends returns the principal's share.
    drop(first);
    let regained = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let (socket, reply) = authenticate(server.address, "hog").await;
            if reply["type"] == "authenticated" {
                break socket;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("a closed connection must free the principal's share");
    drop((second, regained));
    server.shutdown().await.unwrap();
}
