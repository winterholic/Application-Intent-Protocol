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
    if let Some(workers) = &config.workers {
        if workers.enable_writes {
            return Err(unsupported("workers.enable_writes", "MySQL adapter는 worker write 확장을 아직 지원하지 않음(쓰기·작업 slice 이후)"));
        }
        if workers.enable_file_uploads {
            return Err(unsupported("workers.enable_file_uploads", "MySQL adapter는 worker 파일 업로드를 아직 지원하지 않음"));
        }
    }
    let checks: [(&str, bool, &str); 12] = [
        ("objects", config.objects.is_some(), "객체 저장소 metadata가 PostgreSQL에 있음"),
        ("broker", config.broker.is_some(), "broker offset·outbox가 PostgreSQL에 있음"),
        ("rag", config.rag.is_some(), "RAG 색인이 PostgreSQL(pgvector)에 있음"),
        ("static_mounts", !config.static_mounts.is_empty(), "정적 mount는 아직 연결하지 않음"),
        ("file_mounts", !config.file_mounts.is_empty(), "관리 파일 저장소가 PostgreSQL에 있음"),
        ("schedules", !config.schedules.is_empty(), "durable job 저장소가 PostgreSQL에 있음"),
        ("webhooks", !config.webhooks.is_empty(), "durable job 저장소가 PostgreSQL에 있음"),
        ("outbox_handlers", !config.outbox_handlers.is_empty(), "outbox가 PostgreSQL에 있음"),
        ("message_handlers", !config.message_handlers.is_empty(), "메시지 저장소가 PostgreSQL에 있음"),
        ("job_policies", !config.job_policies.is_empty(), "durable job 저장소가 PostgreSQL에 있음"),
        ("messaging", config.messaging.enabled, "메시지 저장소가 PostgreSQL에 있음"),
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
    let mapping: Mapping = external_mapping(&config)?.ok_or_else(|| error("EXTERNAL_MAPPING_REQUIRED", "external_mapping 설정이 필요함"))?;
    let url = database_url(&config)?;
    let deployment = aip_migrate::mysql_adopt::deployment(&url, &config.schema, &facts, &mapping, config.wire.deployment_label()).await?;
    let driver = driver_for(&config, &url)?;
    let fence =
        MysqlDeploymentFence::new(&config.schema, &mapping.schema, &deployment.source_tables, &deployment.facts_digest, &deployment.structure_digest)
            .map_err(catalog_error)?;
    let backend = MysqlBackend::new(driver.clone(), fence);
    backend.check_server_profile().await.map_err(catalog_error)?;
    sqlgen::try_set_schema(&config.schema).map_err(|_| error("SCHEMA_CONTEXT", "프로세스당 한 schema만 실행할 수 있음"))?;
    let deployed = deployment.facts;
    let actor_name = deployed["actor"].as_str().unwrap_or("principal");
    let actor = (deployed["actorMode"] != "principal" && deployed["resources"].get(actor_name).is_some_and(Value::is_object))
        .then(|| ActorResource { view: sqlgen::snake(actor_name), uuid: spike_v2_read::id_wire::id_kind(&deployed, actor_name) == "uuid" });
    let mut infrastructure = infrastructure::build(&config)?;
    let verifier = aip_auth::AuthenticatorImpl::new_explained(config.auth, "mysql".into(), config.schema.clone())
        .await
        .map_err(|failure| error("AUTH_CONFIG", &format!("인증 설정 오류: {}", failure.reason)))?;
    let authenticator = Arc::new(MysqlAuthenticator { verifier, driver, schema: config.schema.clone(), actor });
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
    // WBS 7.2.3 Slice13-A: 관리형 Mongo/Redis capability 연결
    if infrastructure.mongo.is_none() {
        infrastructure.mongo = Some(Arc::new(spike_v4_worker::ManagedMongoCapability::default()));
    }
    if infrastructure.redis.is_none() {
        infrastructure.redis = Some(Arc::new(spike_v4_worker::ManagedRedisCapability::default()));
    }
    let options = spike_v6_transport::server::ServerOptions { allowed_origins: config.allowed_origins };
    #[cfg(unix)]
    let mut termination =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(|_| error("SIGNAL_CONFIG", "종료 signal 등록 실패"))?;
    let mut server = spike_v6_transport::server::listen_relational_with_extensions(
        deployed.clone(),
        config.wire.id_wire(),
        config.listen,
        Arc::new(backend),
        extensions,
        options,
        infrastructure,
        Some(authenticator),
    )
    .await
    .map_err(|e| server_start_error(&e))?;
    let fingerprint = if config.workers.is_some() {
        spike_v5_sdk::contract_fingerprint_with_extensions(&deployed, config.wire.id_wire())
    } else {
        spike_v5_sdk::contract_fingerprint_with_apply(&deployed, config.wire.id_wire())
    };
    let event = json!({
        "ok": true, "event": "listening", "address": server.address.to_string(), "adapter": "mysql",
        "fingerprint": fingerprint,
        "idWire": config.wire.id_wire().label(), "authentication": "jwt-access-token",
    });
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

        // 2. 쓰기 워커는 거부
        let mut write_worker_json = base_json.clone();
        write_worker_json["workers"] = json!({
            "directory": "extensions",
            "language": "python",
            "enable_writes": true
        });
        let config: Config = serde_json::from_value(write_worker_json).unwrap();
        let err = reject_unsupported_infrastructure(&config).unwrap_err();
        assert_eq!(err["code"], "DB_ADAPTER_UNSUPPORTED");
        assert!(err["message"].as_str().unwrap().contains("workers.enable_writes"));

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
