#![cfg(feature = "mysql")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! WBS 7.2.3 Slice13-B live host. Serves an already adopted FaceReview MySQL deployment in process with the Slice13-B read
//! extensions (`Video.homeSearch`, `Video.homeVideoAll`, `Video.homeBookmark`), a real MongoDB driver capability and the Python
//! worker, then waits for a stop file. The driver script `product/tests/fixtures/facereview-mysql/reference/slice13b_oracle.py`
//! seeds the data, adopts, binds principals, starts this host and compares the HTTP answers with the original HomeService.
//!
//! Why in process: the definition grammar has no way to declare `access mongo.<collection>` (DEF.UNKNOWN_SYMBOL), so a definition
//! served by `aip service serve` cannot hold a Mongo grant. This host compiles the real definition text (extensions declared without
//! the Mongo access) and adds only the `mongo.video_distribution` grant to those extensions' facts. That is the same facts-level
//! step the Slice13-A bridge tests use. Authentication is also replaced: the bearer token is the principal subject, resolved through
//! the real `aip_principals` / `aip_actor_identities` tables like `MysqlAuthenticator` does, without JWT signature checking.
use aip_migrate::mysql_adopt;
use aip_migrate::mysql_backend::MysqlBackend;
use aip_migrate::mysql_driver::mysql_async::prelude::Queryable;
use aip_migrate::mysql_driver::{MysqlDeploymentFence, MysqlDriver};
use serde_json::json;
use spike_v1_fixture::{Form, load_str};
use spike_v2_read::external::Mapping;
use spike_v2_read::id_wire::IdWire;
use spike_v2_read::sqlgen;
use spike_v4_worker::MongoDriverCapability;
use spike_v6_transport::auth::{AuthError, Authenticator, DurablePrincipal, Session};
use spike_v6_transport::infrastructure::Infrastructure;
use spike_v6_transport::server::{ReadExtensions, ServerOptions, WorkerLang, listen_relational_with_extensions};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ISSUER: &str = "https://issuer.example";
const EXTENSIONS: [&str; 3] = ["homeSearch", "homeVideoAll", "homeBookmark"];

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name}"))
}

struct SubjectAuthenticator {
    driver: MysqlDriver,
    schema: String,
    actor_view: String,
}

impl Authenticator for SubjectAuthenticator {
    fn verify_session<'a>(&'a self, token: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Session, AuthError>> + Send + 'a>> {
        Box::pin(async move {
            let mut conn = self.driver.pool().get_conn().await.map_err(|_| AuthError::Unavailable)?;
            let schema = &self.schema;
            let row: Option<(i64, i64, i64)> = conn
                .exec_first(
                    format!("SELECT actor_id, enabled, min_iat FROM `{schema}`.`aip_principals` WHERE issuer = ? AND subject = ?"),
                    (ISSUER, token),
                )
                .await
                .map_err(|_| AuthError::Unavailable)?;
            let (actor_id, enabled, min_iat) = row.ok_or(AuthError::BadSignature)?;
            if enabled != 1 {
                return Err(AuthError::BadSignature);
            }
            let sql = format!(
                "SELECT i.resource_id FROM `{schema}`.`aip_actor_identities` i JOIN `{schema}`.`{view}` a ON a.`id` = i.resource_id WHERE i.id = ?",
                view = self.actor_view
            );
            let public: Option<String> = conn.exec_first(sql, (actor_id,)).await.map_err(|_| AuthError::Unavailable)?;
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
            let grant = DurablePrincipal { issuer: ISSUER.into(), subject: token.into(), actor_id, issued_at: now.max(min_iat), min_iat };
            let mut session = Session::new_durable(grant, now + 600)?;
            session.actor_resource_id = Some(public.ok_or(AuthError::BadSignature)?);
            Ok(session)
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: driven by slice13b_oracle.py against run-owned MySQL 8.4 and MongoDB containers"]
async fn slice13b_host_serves_the_adopted_facereview_deployment_with_mongo_grants() {
    let definition = std::fs::read_to_string(env("AIP_S13B_DEFINITION")).unwrap();
    let mapping: Mapping = serde_json::from_str(&std::fs::read_to_string(env("AIP_S13B_MAPPING")).unwrap()).unwrap();
    let meta = env("AIP_S13B_META");
    let runtime_url = env("AIP_S13B_RUNTIME_URL");
    let facts = load_str(&definition, Form::A).unwrap_or_else(|d| panic!("definition: {d:?}")).execution;
    sqlgen::try_set_schema(&meta).unwrap();
    let deployment = mysql_adopt::deployment(&runtime_url, &meta, &facts, &mapping, "decimal").await.unwrap_or_else(|e| panic!("deployment: {e}"));
    let driver = MysqlDriver::from_url(&runtime_url).unwrap();
    let fence =
        MysqlDeploymentFence::new(&meta, &mapping.schema, &deployment.source_tables, &deployment.facts_digest, &deployment.structure_digest).unwrap();
    let backend = Arc::new(MysqlBackend::new(driver.clone(), fence));
    backend.check_server_profile().await.expect("MySQL 8.4 strict profile");

    let mut deployed = deployment.facts;
    for name in EXTENSIONS {
        let extension = deployed["resources"]["Video"]["extensions"].get_mut(name).unwrap_or_else(|| panic!("definition declares Video.{name}"));
        extension["access"]["mongo.video_distribution"] = json!({});
    }

    let mongo = MongoDriverCapability::connect(&env("AIP_S13B_MONGO_URL"), &env("AIP_S13B_MONGO_DB"), 4, Duration::from_millis(3000))
        .await
        .map_err(|e| e.code)
        .expect("mongo");
    let mut infrastructure = Infrastructure { mongo: Some(Arc::new(mongo)), ..Infrastructure::default() };
    infrastructure.realtime.enabled = false;
    let wire = IdWire::DecimalString;
    let authenticator = Arc::new(SubjectAuthenticator { driver, schema: meta.clone(), actor_view: sqlgen::snake("User") });
    let server = listen_relational_with_extensions(
        deployed.clone(),
        wire,
        "127.0.0.1:0".parse().unwrap(),
        backend,
        Some(ReadExtensions { lang: WorkerLang::Python, dir: env("AIP_S13B_EXT_DIR").into() }),
        ServerOptions::default(),
        infrastructure,
        Some(authenticator),
    )
    .await
    .expect("listen");
    let fingerprint = spike_v5_sdk::contract_fingerprint_with_extensions(&deployed, wire);
    std::fs::write(env("AIP_S13B_READY_FILE"), json!({"address": server.address.to_string(), "fingerprint": fingerprint}).to_string()).unwrap();
    let stop = env("AIP_S13B_STOP_FILE");
    while !std::path::Path::new(&stop).exists() {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    server.shutdown().await.unwrap();
}
