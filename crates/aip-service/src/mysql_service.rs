//! `aip service` commands against an existing MySQL 8.4 business database (WBS 7.2.1.1): `adopt-external`, `principal`
//! and `serve` with the read path. Everything the MySQL adapter does not implement yet is refused by name before any
//! connection, never run through the PostgreSQL code or emulated.
use super::*;
use aip_migrate::mysql_backend::MysqlBackend;
use aip_migrate::mysql_catalog::CatalogError;
use aip_migrate::mysql_driver::{MysqlDeploymentFence, MysqlDriver, mysql_async};
use mysql_async::TxOpts;
use mysql_async::prelude::Queryable;
use spike_v2_read::external::Mapping;

const DB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn unsupported(setting: &str, why: &str) -> Value {
    error("DB_ADAPTER_UNSUPPORTED", &format!("MySQL adapter는 `{setting}` 설정을 아직 지원하지 않음: {why}"))
}

fn catalog_error(error_value: CatalogError) -> Value {
    error(&error_value.code, &error_value.message)
}

/// True when the resolved adapter is MySQL. MariaDB is a different gate and is refused here.
pub(crate) fn selected(config: &Config) -> Result<bool, Value> {
    match resolve_database_adapter(config)?.as_str() {
        "mysql" => Ok(true),
        "mariadb" => Err(error(
            "DB_ADAPTER_UNSUPPORTED",
            "MariaDB는 서빙·채택을 아직 지원하지 않음: read compiler가 쓰는 LATERAL을 MariaDB가 제공하지 않으며 별도 호환 게이트가 필요함",
        )),
        _ => Ok(false),
    }
}

/// Settings whose infrastructure lives in PostgreSQL today. A configuration that asks for one is rejected instead of being
/// served without it.
fn reject_unsupported_infrastructure(config: &Config) -> Result<(), Value> {
    if external_mapping(config)?.is_none() {
        return Err(error("EXTERNAL_MAPPING_REQUIRED", "MySQL adapter는 기존 업무 DB 채택만 지원하므로 external_mapping 설정이 필요함"));
    }
    if let Some(workers) = &config.workers
        && workers.enable_file_uploads
    {
        return Err(unsupported("workers.enable_file_uploads", "MySQL adapter는 worker 파일 업로드를 아직 지원하지 않음"));
    }
    let checks: [(&str, bool, &str); 6] = [
        ("objects", config.objects.is_some(), "객체 저장소 metadata가 PostgreSQL에 있음"),
        ("broker", config.broker.is_some(), "broker offset·outbox가 PostgreSQL에 있음"),
        ("rag", config.rag.is_some(), "RAG 색인이 PostgreSQL(pgvector)에 있음"),
        ("static_mounts", !config.static_mounts.is_empty(), "정적 mount는 아직 연결하지 않음"),
        ("file_mounts", !config.file_mounts.is_empty(), "관리 파일 저장소가 PostgreSQL에 있음"),
        (
            "rate_limit",
            config.rate_limit.is_some(),
            "rate limiter 저장소가 PostgreSQL에 있음. 앞단 proxy에서 제한하고 설정에 `\"rate_limit\": null`을 명시해야 함",
        ),
    ];
    match checks.iter().find(|(_, present, _)| *present) {
        Some((setting, _, why)) => Err(unsupported(setting, why)),
        None => Ok(()),
    }
}

fn driver_for(config: &Config, url: &str) -> Result<MysqlDriver, Value> {
    MysqlDriver::from_url_with_ca(url, config.database_ca_file.as_deref()).map_err(catalog_error)
}

async fn configure_optional_read_adapters(
    infrastructure: &mut spike_v6_transport::infrastructure::Infrastructure,
    workers: Option<&Workers>,
    uri_for: impl Fn(&str) -> Option<String>,
) -> Result<(), Value> {
    let Some(workers) = workers else { return Ok(()) };
    if let Some(store) = &workers.mongo {
        let uri = uri_for(&store.uri_env).ok_or_else(|| error("MONGO_CAPABILITY_CONFIG", "Mongo URI 환경 변수가 설정되지 않음"))?;
        let driver = spike_v4_worker::MongoDriverCapability::connect(
            &uri,
            &store.database,
            store.max_pool_size,
            std::time::Duration::from_millis(store.timeout_ms),
        )
        .await
        .map_err(|_| error("MONGO_CAPABILITY_UNAVAILABLE", "Mongo 연결/readiness를 확인할 수 없음"))?;
        infrastructure.mongo = Some(Arc::new(driver));
    }
    if let Some(store) = &workers.redis {
        let uri = uri_for(&store.uri_env).ok_or_else(|| error("REDIS_CAPABILITY_CONFIG", "Redis URI 환경 변수가 설정되지 않음"))?;
        let driver = spike_v4_worker::RedisDriverCapability::connect_with_limit(
            &uri,
            store.prefixes.clone(),
            std::time::Duration::from_millis(store.timeout_ms),
            store.max_in_flight,
        )
        .await
        .map_err(|_| error("REDIS_CAPABILITY_UNAVAILABLE", "Redis 연결/readiness를 확인할 수 없음"))?;
        infrastructure.redis = Some(Arc::new(driver));
    }
    Ok(())
}

fn mysql_listening_event(address: SocketAddr, fingerprint: String, id_wire: &str, read_adapters: Value) -> Value {
    json!({
        "ok": true, "event": "listening", "address": address.to_string(), "adapter": "mysql",
        "fingerprint": fingerprint, "idWire": id_wire, "authentication": "jwt-access-token",
        "readAdapters": read_adapters,
    })
}

pub(crate) async fn adopt_external(file: &Path, config: &Path, apply: Option<String>) -> Result<Value, Value> {
    let c = configuration_for_runtime(config)?;
    reject_unsupported_infrastructure(&c)?;
    let facts = load(file, false)?.output.execution;
    let mapping = external_mapping(&c)?.ok_or_else(|| error("EXTERNAL_MAPPING_REQUIRED", "external_mapping 설정이 필요함"))?;
    let url = migrate_database_url(&c)?;
    match apply {
        Some(hash) => aip_migrate::mysql_adopt::adopt_external(&url, &c.schema, &facts, &mapping, c.wire.deployment_label(), &hash).await,
        None => aip_migrate::mysql_adopt::plan_external(&url, &c.schema, &facts, &mapping, c.wire.deployment_label()).await,
    }
}

/// Principal lookup against `aip_principals` in the AIP schema. Token verification (RS256 JWKS, issuer, audience, lifetime)
/// is the PostgreSQL deployment's own verifier; only the binding store differs.
struct MysqlAuthenticator {
    verifier: aip_auth::AuthenticatorImpl,
    driver: MysqlDriver,
    schema: String,
    /// Resource that carries the actor's public identity (`None` for managed principals).
    actor: Option<ActorResource>,
}

struct ActorResource {
    view: String,
    uuid: bool,
}

fn map_auth(error: aip_auth::AuthError) -> spike_v6_transport::auth::AuthError {
    match error {
        aip_auth::AuthError::Invalid => spike_v6_transport::auth::AuthError::BadSignature,
        aip_auth::AuthError::Expired => spike_v6_transport::auth::AuthError::Expired,
        aip_auth::AuthError::Unavailable => spike_v6_transport::auth::AuthError::Unavailable,
    }
}

impl MysqlAuthenticator {
    async fn session(&self, token: &str) -> Result<spike_v6_transport::auth::Session, spike_v6_transport::auth::AuthError> {
        use spike_v6_transport::auth::AuthError;
        let claims = self.verifier.verify_token(token).await.map_err(map_auth)?;
        let mut conn = tokio::time::timeout(DB_TIMEOUT, self.driver.pool().get_conn())
            .await
            .map_err(|_| AuthError::Unavailable)?
            .map_err(|_| AuthError::Unavailable)?;
        let schema = &self.schema;
        let row: Option<(i64, i64, i64)> = tokio::time::timeout(
            DB_TIMEOUT,
            conn.exec_first(
                format!("SELECT actor_id, enabled, min_iat FROM `{schema}`.`aip_principals` WHERE issuer = ? AND subject = ?"),
                (claims.issuer.as_str(), claims.subject.as_str()),
            ),
        )
        .await
        .map_err(|_| AuthError::Unavailable)?
        .map_err(|_| AuthError::Unavailable)?;
        let (actor_id, enabled, min_iat) = row.ok_or(AuthError::BadSignature)?;
        if enabled != 1 || claims.issued_at < min_iat {
            return Err(AuthError::BadSignature);
        }
        // The public actor id, and proof that the business row the binding points at still exists.
        let public_id: Option<String> = match &self.actor {
            None => None,
            Some(actor) => {
                let sql = if actor.uuid {
                    format!(
                        "SELECT i.resource_id FROM `{schema}`.`aip_actor_identities` i JOIN `{schema}`.`{view}` a ON a.`id` = i.resource_id WHERE i.id = ?",
                        view = actor.view
                    )
                } else {
                    format!("SELECT CAST(a.`id` AS CHAR) FROM `{schema}`.`{view}` a WHERE a.`id` = ?", view = actor.view)
                };
                let found: Option<String> = tokio::time::timeout(DB_TIMEOUT, conn.exec_first(sql, (actor_id,)))
                    .await
                    .map_err(|_| AuthError::Unavailable)?
                    .map_err(|_| AuthError::Unavailable)?;
                Some(found.ok_or(AuthError::BadSignature)?)
            }
        };
        let grant = spike_v6_transport::auth::DurablePrincipal {
            issuer: claims.issuer,
            subject: claims.subject,
            actor_id,
            issued_at: claims.issued_at,
            min_iat,
        };
        spike_v6_transport::auth::Session::new_durable(grant, claims.expires_at).map(|mut session| {
            session.actor_resource_id = public_id;
            session
        })
    }
}

impl spike_v6_transport::auth::Authenticator for MysqlAuthenticator {
    fn verify_session<'a>(
        &'a self,
        token: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<spike_v6_transport::auth::Session, spike_v6_transport::auth::AuthError>> + Send + 'a>,
    > {
        Box::pin(self.session(token))
    }
}

pub(crate) async fn principal(config: Config, command: Principal) -> Result<Value, Value> {
    reject_unsupported_infrastructure(&config)?;
    let url = database_url(&config)?;
    let driver = driver_for(&config, &url)?;
    let schema = config.schema.clone();
    let issuer = config.auth.issuer.clone();
    let wire_label = config.wire.deployment_label().to_owned();
    let id_wire = config.wire.id_wire();
    let work = async {
        let mut conn = driver.pool().get_conn().await.map_err(|_| error("DB_CONNECT", "DB 연결 실패"))?;
        let mut tx = conn.start_transaction(TxOpts::default()).await.map_err(|_| error("PRINCIPAL_FAILED", "사용자 연결 트랜잭션 시작 실패"))?;
        // The marker is read shared: an adoption that rewrites it waits for this change to finish.
        let marker: Option<(String, Option<String>, Option<String>)> = tx
            .query_first(format!("SELECT CAST(facts AS CHAR), runtime_wire, status FROM `{schema}`.`aip_migrate_meta` WHERE singleton = 1 FOR SHARE"))
            .await
            .map_err(|_| error("PRINCIPAL_FAILED", "배포 신원 계약 조회 실패"))?
            .map(|(facts, wire, status): (Option<String>, Option<String>, String)| (facts.unwrap_or_default(), wire, Some(status)));
        let (facts_text, stored_wire, status) = marker.ok_or_else(|| error("PRINCIPAL_FAILED", "배포 신원 계약 조회 실패"))?;
        if status.as_deref() != Some("READY") {
            return Err(error("PRINCIPAL_FAILED", "채택이 끝나지 않은 배포에는 사용자를 연결할 수 없음"));
        }
        let facts: Value = serde_json::from_str(&facts_text).map_err(|_| error("PRINCIPAL_FAILED", "배포 신원 계약 형식 오류"))?;
        if stored_wire.as_deref().is_some_and(|wire| wire != wire_label) {
            return Err(error("WIRE_MODE_MISMATCH", "기존 배포 wire와 설정 wire가 다름"));
        }
        let managed = facts["actorMode"] == "principal";
        let affected = match command {
            Principal::Bind { subject, actor, not_before } => {
                if subject.is_empty() || subject.chars().count() > 256 || not_before < 0 {
                    return Err(error("BAD_PRINCIPAL", "subject·actor·not-before 값 오류"));
                }
                let actor_resource = facts["actor"].as_str().unwrap_or("principal");
                let view = sqlgen::snake(actor_resource);
                let handle: i64 = if spike_v2_read::id_wire::id_kind(&facts, actor_resource) == "uuid" {
                    let id = spike_v2_read::id_wire::parse_uuid(&actor)
                        .map_err(|_| error("BAD_PRINCIPAL", "actor는 정규 UUID 문자열이어야 함"))?
                        .to_string();
                    let present: Option<i64> = tx
                        .exec_first(format!("SELECT 1 FROM `{schema}`.`{view}` WHERE `id` = ?"), (id.as_str(),))
                        .await
                        .map_err(|_| error("BAD_PRINCIPAL", "actor resource 조회 실패"))?;
                    if present.is_none() {
                        return Err(error("BAD_PRINCIPAL", "actor resource 행이 없음"));
                    }
                    tx.exec_drop(
                        format!(
                            "INSERT INTO `{schema}`.`aip_actor_identities` (resource_id) VALUES (?) ON DUPLICATE KEY UPDATE resource_id = resource_id"
                        ),
                        (id.as_str(),),
                    )
                    .await
                    .map_err(|_| error("BAD_PRINCIPAL", "UUID 신원 연결 실패"))?;
                    tx.exec_first(format!("SELECT id FROM `{schema}`.`aip_actor_identities` WHERE resource_id = ?"), (id.as_str(),))
                        .await
                        .map_err(|_| error("BAD_PRINCIPAL", "UUID 신원 연결 실패"))?
                        .ok_or_else(|| error("BAD_PRINCIPAL", "UUID 신원 연결 실패"))?
                } else {
                    let handle = spike_v2_read::id_wire::parse_id(&json!(actor), IdWire::DecimalString)
                        .map_err(|_| error("BAD_PRINCIPAL", "actor 숫자 Id 형식 오류"))?;
                    if handle <= 0 {
                        return Err(error("BAD_PRINCIPAL", "actor 숫자 Id는 양수여야 함"));
                    }
                    if !managed {
                        let present: Option<i64> = tx
                            .exec_first(format!("SELECT 1 FROM `{schema}`.`{view}` WHERE `id` = ?"), (handle,))
                            .await
                            .map_err(|_| error("BAD_PRINCIPAL", "actor resource 조회 실패"))?;
                        if present.is_none() {
                            return Err(error("BAD_PRINCIPAL", "actor resource 행이 없음"));
                        }
                    }
                    if id_wire == IdWire::SafeNumber && handle > spike_v2_read::id_wire::MAX_SAFE_ID {
                        return Err(error("BAD_PRINCIPAL", "safe wire의 actor 안전 정수 상한 초과"));
                    }
                    tx.exec_drop(format!("INSERT IGNORE INTO `{schema}`.`aip_actors` (id) VALUES (?)"), (handle,))
                        .await
                        .map_err(|_| error("PRINCIPAL_FAILED", "운영 신원 등록 실패"))?;
                    handle
                };
                // An existing binding keeps its actor; only the same actor may be re-bound.
                let existing: Option<i64> = tx
                    .exec_first(
                        format!("SELECT actor_id FROM `{schema}`.`aip_principals` WHERE issuer = ? AND subject = ? FOR UPDATE"),
                        (issuer.as_str(), subject.as_str()),
                    )
                    .await
                    .map_err(|_| error("PRINCIPAL_FAILED", "actor 존재·매핑 테이블·DB 권한을 확인해야 함"))?;
                if existing.is_some_and(|current| current != handle) {
                    return Err(error("PRINCIPAL_CONFLICT", "이미 연결된 신원의 actor를 바꿀 수 없음"));
                }
                tx.exec_drop(
                    format!(
                        "INSERT INTO `{schema}`.`aip_principals` (issuer, subject, actor_id, enabled, min_iat) VALUES (?, ?, ?, 1, ?) \
                         ON DUPLICATE KEY UPDATE enabled = 1, min_iat = GREATEST(min_iat, VALUES(min_iat))"
                    ),
                    (issuer.as_str(), subject.as_str(), handle, not_before),
                )
                .await
                .map_err(|_| error("PRINCIPAL_FAILED", "actor 존재·매핑 테이블·DB 권한을 확인해야 함"))?;
                1_u64
            }
            Principal::Revoke { subject } => {
                if subject.is_empty() || subject.chars().count() > 256 {
                    return Err(error("BAD_PRINCIPAL", "subject 값 오류"));
                }
                let existing: Option<i64> = tx
                    .exec_first(
                        format!("SELECT actor_id FROM `{schema}`.`aip_principals` WHERE issuer = ? AND subject = ? FOR UPDATE"),
                        (issuer.as_str(), subject.as_str()),
                    )
                    .await
                    .map_err(|_| error("PRINCIPAL_FAILED", "actor 존재·매핑 테이블·DB 권한을 확인해야 함"))?;
                if existing.is_none() {
                    return Err(error("PRINCIPAL_NOT_FOUND", "연결된 신원이 없음"));
                }
                // Tokens issued before now stop working: min_iat moves one second past the database clock.
                tx.exec_drop(
                    format!(
                        "UPDATE `{schema}`.`aip_principals` SET enabled = 0, min_iat = GREATEST(min_iat, UNIX_TIMESTAMP() + 1) WHERE issuer = ? AND subject = ?"
                    ),
                    (issuer.as_str(), subject.as_str()),
                )
                .await
                .map_err(|_| error("PRINCIPAL_FAILED", "actor 존재·매핑 테이블·DB 권한을 확인해야 함"))?;
                1_u64
            }
        };
        tx.commit().await.map_err(|_| error("PRINCIPAL_UNSETTLED", "사용자 연결 커밋 결과 불명; 상태 확인 필요"))?;
        Ok(json!({"ok": true, "affected": affected}))
    };
    tokio::time::timeout(DB_TIMEOUT, work).await.map_err(|_| error("PRINCIPAL_UNSETTLED", "사용자 연결 변경 결과 불명; 상태 확인 필요"))?
}

pub(crate) async fn serve(file: &Path, config: Config) -> Result<Value, Value> {
    reject_unsupported_infrastructure(&config)?;
    let facts = load(file, false)?.output.execution;
    require_supported_runtime(&facts)?;
    let mapping: Mapping = external_mapping(&config)?.ok_or_else(|| error("EXTERNAL_MAPPING_REQUIRED", "external_mapping 설정이 필요함"))?;
    let url = database_url(&config)?;
    let deployment = aip_migrate::mysql_adopt::deployment(&url, &config.schema, &facts, &mapping, config.wire.deployment_label()).await?;
    let driver = driver_for(&config, &url)?;
    let fence =
        MysqlDeploymentFence::new(&config.schema, &mapping.schema, &deployment.source_tables, &deployment.facts_digest, &deployment.structure_digest)
            .map_err(catalog_error)?;
    let backend = Arc::new(MysqlBackend::new(driver.clone(), fence.clone()));
    backend.check_server_profile().await.map_err(catalog_error)?;
    sqlgen::try_set_schema(&config.schema).map_err(|_| error("SCHEMA_CONTEXT", "프로세스당 한 schema만 실행할 수 있음"))?;
    let deployed = deployment.facts;
    let actor_name = deployed["actor"].as_str().unwrap_or("principal");
    let actor = (deployed["actorMode"] != "principal" && deployed["resources"].get(actor_name).is_some_and(Value::is_object))
        .then(|| ActorResource { view: sqlgen::snake(actor_name), uuid: spike_v2_read::id_wire::id_kind(&deployed, actor_name) == "uuid" });
    let mut infrastructure = infrastructure::build(&config)?;
    let issuer = config.auth.issuer.clone();
    let verifier = aip_auth::AuthenticatorImpl::new_explained(config.auth, "mysql".into(), config.schema.clone())
        .await
        .map_err(|failure| error("AUTH_CONFIG", &format!("인증 설정 오류: {}", failure.reason)))?;
    let authenticator = Arc::new(MysqlAuthenticator { verifier, driver: driver.clone(), schema: config.schema.clone(), actor });
    infrastructure.rate_limit = None;
    // Subscriptions wake on commits of this process; the read-only adapter has none, so the transport answers
    // WIRE.NOT_AVAILABLE for event streams instead of serving a stream that never fires.
    infrastructure.realtime.enabled = false;
    let extensions = config.workers.as_ref().map(|w| spike_v6_transport::server::ReadExtensions {
        dir: w.directory.clone(),
        lang: match w.language {
            Language::Node => spike_v6_transport::server::WorkerLang::Node,
            Language::Python => spike_v6_transport::server::WorkerLang::Python,
        },
    });
    configure_optional_read_adapters(&mut infrastructure, config.workers.as_ref(), |name| std::env::var(name).ok()).await?;
    infrastructure.job_policies = triggers::build_job_policies(&config.job_policies).map_err(|message| error("JOB_POLICY_CONFIG", message))?;
    let mut built = triggers::build(&config.schedules, &config.webhooks, &config.outbox_handlers, &issuer, &|name| std::env::var(name).ok())
        .map_err(|message| error("TRIGGER_CONFIG", message))?;
    built.messages = triggers::build_message_handlers(&config.message_handlers, &issuer);
    let has_triggers = !built.is_empty();
    infrastructure.triggers = Arc::new(built);
    let has_jobs = deployed["operations"].as_object().is_some_and(|ops| ops.values().any(|op| op["kind"] == "job"));
    if has_triggers && !(has_jobs && extensions.is_some()) {
        return Err(error("TRIGGER_CONFIG", "schedules·webhooks·outbox_handlers는 job을 선언한 계약과 workers 설정이 있어야 함"));
    }
    if has_jobs && let Some(extensions) = &extensions {
        let integrations = config.workers.as_ref().map(|workers| workers.integrations.as_slice()).unwrap_or_default();
        let outbound_webhooks = config.workers.as_ref().map(|workers| workers.outbound_webhooks.as_slice()).unwrap_or_default();
        let broker = build_broker_with_webhooks(integrations, outbound_webhooks).await?;
        let jobs = super::mysql_jobs::MysqlJobs::new(
            driver,
            fence,
            config.schema.clone(),
            deployment.facts_digest,
            Arc::new(deployed.clone()),
            config.wire.id_wire(),
            backend.clone(),
            extensions.clone(),
            broker,
            &infrastructure,
        )?;
        tokio::time::timeout(DB_TIMEOUT, jobs.check_storage())
            .await
            .map_err(|_| error("JOB_STORAGE_NOT_READY", "MySQL effect journal 확인 시간 초과"))?
            .map_err(|_| {
                error("JOB_STORAGE_NOT_READY", "MySQL effect journal 미준비: migration 계정으로 adopt-external --apply를 다시 실행해야 함")
            })?;
        infrastructure.relational_jobs = Some(Arc::new(jobs));
    }
    let read_adapters = infrastructure.catalog()["readAdapters"].clone();
    let options = spike_v6_transport::server::ServerOptions { allowed_origins: config.allowed_origins };
    #[cfg(unix)]
    let mut termination =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(|_| error("SIGNAL_CONFIG", "종료 signal 등록 실패"))?;
    let enable_worker_writes = config.workers.as_ref().is_some_and(|workers| workers.enable_writes);
    let mut server = spike_v6_transport::server::listen_relational_with_worker_writes(
        deployed.clone(),
        config.wire.id_wire(),
        config.listen,
        backend,
        extensions,
        enable_worker_writes,
        options,
        infrastructure,
        Some(authenticator),
    )
    .await
    .map_err(|e| server_start_error(&e))?;
    let fingerprint = if enable_worker_writes || has_jobs {
        spike_v5_sdk::contract_fingerprint_with_all_extensions(&deployed, config.wire.id_wire())
    } else if config.workers.is_some() {
        spike_v5_sdk::contract_fingerprint_with_extensions(&deployed, config.wire.id_wire())
    } else {
        spike_v5_sdk::contract_fingerprint_with_apply(&deployed, config.wire.id_wire())
    };
    let event = mysql_listening_event(server.address, fingerprint, config.wire.id_wire().label(), read_adapters);
    if report(&event).is_err() {
        server.shutdown().await.map_err(|_| error("SERVER_STOP", "출력 실패 후 종료 실패"))?;
        return Err(error("OUTPUT_IO", "기동 결과 출력 실패"));
    }
    #[cfg(unix)]
    tokio::select! {
        result = tokio::signal::ctrl_c() => { result.map_err(|_| error("SIGNAL_FAILED", "종료 signal 실패"))?; server.shutdown().await.map_err(|_| error("SERVER_STOP", "서버 종료 실패"))?; },
        _ = termination.recv() => { server.shutdown().await.map_err(|_| error("SERVER_STOP", "서버 종료 실패"))?; },
        result = server.wait() => { result.map_err(|_| error("SERVER_FAILED", "서버 실행 실패"))?; },
    }
    #[cfg(not(unix))]
    tokio::select! {
        result = tokio::signal::ctrl_c() => { result.map_err(|_| error("SIGNAL_FAILED", "종료 signal 실패"))?; server.shutdown().await.map_err(|_| error("SERVER_STOP", "서버 종료 실패"))?; },
        result = server.wait() => { result.map_err(|_| error("SERVER_FAILED", "서버 실행 실패"))?; },
    }
    Ok(json!({"ok": true, "event": "stopped"}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_worker_config(mongo: bool, redis: bool, timeout_ms: u64) -> Workers {
        let mut config = json!({"directory":".","language":"python"});
        if mongo {
            config["mongo"] = json!({"uri_env":"TEST_MONGO_URI","database":"aip_test_slice13a","max_pool_size":2,"timeout_ms":timeout_ms});
        }
        if redis {
            config["redis"] = json!({"uri_env":"TEST_REDIS_URI","prefixes":["facereview:reco:"],"max_in_flight":2,"timeout_ms":timeout_ms});
        }
        serde_json::from_value(config).unwrap()
    }

    fn adapter_uri(name: &str, mongo_uri: &str, redis_uri: &str) -> Option<String> {
        match name {
            "TEST_MONGO_URI" => Some(mongo_uri.to_owned()),
            "TEST_REDIS_URI" => Some(redis_uri.to_owned()),
            _ => None,
        }
    }

    #[tokio::test]
    async fn optional_adapter_combinations_are_reported_by_discovery_and_listening_output() {
        let mongo_uri = std::env::var("AIP_TEST_MONGO_URL").expect("start the native-stores Mongo fixture");
        let redis_uri = std::env::var("AIP_TEST_REDIS_URL").expect("start the native-stores Redis fixture");

        let mut no_workers = spike_v6_transport::infrastructure::Infrastructure::default();
        configure_optional_read_adapters(&mut no_workers, None, |_| panic!("unconfigured adapters must not read URI values")).await.unwrap();
        assert_eq!(
            no_workers.catalog()["readAdapters"],
            json!({
                "mongo":{"configured":false,"status":"unavailable"},
                "redis":{"configured":false,"status":"unavailable"}
            })
        );

        for (mongo, redis) in [(false, false), (true, false), (false, true), (true, true)] {
            let workers = read_worker_config(mongo, redis, 2_000);
            let mut infrastructure = spike_v6_transport::infrastructure::Infrastructure::default();
            configure_optional_read_adapters(&mut infrastructure, Some(&workers), |name| adapter_uri(name, &mongo_uri, &redis_uri)).await.unwrap();

            let discovery = infrastructure.catalog();
            let expected_mongo = json!({"configured":mongo,"status":if mongo {"ready"} else {"unavailable"}});
            let expected_redis = json!({"configured":redis,"status":if redis {"ready"} else {"unavailable"}});
            assert_eq!(discovery["readAdapters"]["mongo"], expected_mongo);
            assert_eq!(discovery["readAdapters"]["redis"], expected_redis);
            assert!(!discovery.to_string().contains(&mongo_uri));
            assert!(!discovery.to_string().contains(&redis_uri));
            assert!(!discovery.to_string().contains("TEST_MONGO_URI"));
            assert!(!discovery.to_string().contains("TEST_REDIS_URI"));

            let event =
                mysql_listening_event("127.0.0.1:0".parse().unwrap(), "test-fingerprint".into(), "decimal", discovery["readAdapters"].clone());
            assert_eq!(event["event"], "listening");
            assert_eq!(event["readAdapters"]["mongo"], expected_mongo);
            assert_eq!(event["readAdapters"]["redis"], expected_redis);
            assert!(!event.to_string().contains(&mongo_uri));
            assert!(!event.to_string().contains(&redis_uri));
            assert!(!event.to_string().contains("TEST_MONGO_URI"));
            assert!(!event.to_string().contains("TEST_REDIS_URI"));
        }
    }

    #[tokio::test]
    async fn configured_unreachable_adapters_fail_startup_without_leaking_uri() {
        for (mongo, redis, expected_code) in [(true, false, "MONGO_CAPABILITY_UNAVAILABLE"), (false, true, "REDIS_CAPABILITY_UNAVAILABLE")] {
            let workers = read_worker_config(mongo, redis, 100);
            let mut infrastructure = spike_v6_transport::infrastructure::Infrastructure::default();
            let unavailable = if mongo { "mongodb://127.0.0.1:15443/aip_test_slice13a" } else { "redis://127.0.0.1:15443/" };
            let result = configure_optional_read_adapters(&mut infrastructure, Some(&workers), |name| match name {
                "TEST_MONGO_URI" => Some("mongodb://127.0.0.1:15443/aip_test_slice13a".into()),
                "TEST_REDIS_URI" => Some("redis://127.0.0.1:15443/".into()),
                _ => None,
            })
            .await;
            let err = result.unwrap_err();
            assert_eq!(err["code"], expected_code);
            assert!(!err.to_string().contains(unavailable));
            assert_eq!(infrastructure.catalog()["readAdapters"]["mongo"]["configured"], false);
            assert_eq!(infrastructure.catalog()["readAdapters"]["redis"]["configured"], false);
        }
    }

    #[test]
    fn reject_unsupported_allows_read_workers_and_rejects_writes() {
        let tmp_map = std::env::temp_dir().join("aip_test_mapping.json");
        std::fs::write(
            &tmp_map,
            serde_json::to_string(&json!({
                "schema": "test_src",
                "tables": { "Video": { "table": "video", "columns": { "id": "id" } } }
            }))
            .unwrap(),
        )
        .unwrap();

        let base_json = json!({
            "schema": "test_schema",
            "database_url_env": "DATABASE_URL",
            "external_mapping": tmp_map.to_str().unwrap(),
            "auth": {
                "issuer": "https://auth.example.com",
                "audience": "test_app",
                "jwks": { "kind": "https", "url": "https://auth.example.com/.well-known/jwks.json" }
            },
            "rate_limit": null
        });

        // 1. 읽기 워커는 허용
        let mut read_worker_json = base_json.clone();
        read_worker_json["workers"] = json!({
            "directory": "extensions",
            "language": "python"
        });
        let config: Config = serde_json::from_value(read_worker_json).unwrap();
        assert!(reject_unsupported_infrastructure(&config).is_ok());

        // 2. explicit write extension worker is supported; unrelated infrastructure remains gated below.
        let mut write_worker_json = base_json.clone();
        write_worker_json["workers"] = json!({
            "directory": "extensions",
            "language": "python",
            "enable_writes": true
        });
        let config: Config = serde_json::from_value(write_worker_json).unwrap();
        assert!(reject_unsupported_infrastructure(&config).is_ok());

        // A durable MySQL job uses the same retry-policy configuration as PostgreSQL.
        let mut job_json = base_json.clone();
        job_json["workers"] = json!({"directory":"extensions", "language":"python"});
        job_json["job_policies"] = json!([{"operation":"settle","max_attempts":3,"retry_uncaught_errors":false}]);
        let config: Config = serde_json::from_value(job_json).unwrap();
        assert!(reject_unsupported_infrastructure(&config).is_ok(), "MySQL job policy must reach the durable runtime");

        // Triggers start jobs from the adapter's own job store, and so do the managed message queues.
        let mut trigger_json = base_json.clone();
        trigger_json["workers"] = json!({"directory":"extensions", "language":"python"});
        trigger_json["schedules"] = json!([{"name":"nightly","operation":"settle","cron":"0 3 * * *","principal":"svc"}]);
        trigger_json["outbox_handlers"] = json!([{"topic":"video.requested","operation":"settle","principal":"svc"}]);
        let config: Config = serde_json::from_value(trigger_json.clone()).unwrap();
        assert!(reject_unsupported_infrastructure(&config).is_ok(), "MySQL triggers must reach the durable runtime");
        trigger_json["message_handlers"] = json!([{"queue":"orders","operation":"settle","principal":"svc"}]);
        trigger_json["messaging"] = json!({"enabled": true});
        let config: Config = serde_json::from_value(trigger_json).unwrap();
        assert!(reject_unsupported_infrastructure(&config).is_ok(), "MySQL message handlers must reach the durable runtime");

        // 3. 파일 업로드 워커는 거부
        let mut upload_worker_json = base_json.clone();
        upload_worker_json["workers"] = json!({
            "directory": "extensions",
            "language": "python",
            "enable_file_uploads": true
        });
        let config: Config = serde_json::from_value(upload_worker_json).unwrap();
        let err = reject_unsupported_infrastructure(&config).unwrap_err();
        assert_eq!(err["code"], "DB_ADAPTER_UNSUPPORTED");
        assert!(err["message"].as_str().unwrap().contains("workers.enable_file_uploads"));

        // 4. external_mapping 부재 시 거부
        let mut no_mapping_json = base_json.clone();
        no_mapping_json["external_mapping"] = Value::Null;
        let config: Config = serde_json::from_value(no_mapping_json).unwrap();
        let err = reject_unsupported_infrastructure(&config).unwrap_err();
        assert_eq!(err["code"], "EXTERNAL_MAPPING_REQUIRED");

        let _ = std::fs::remove_file(tmp_map);
    }
}
