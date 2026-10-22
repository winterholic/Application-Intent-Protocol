//! Password login for a MySQL deployment (WBS 7.2.2): `/auth/check-email`, `signup`, `login`, `reissue` and `logout`.
//!
//! The original FaceReview checks a Werkzeug password hash in a Flask controller and signs an HS256 token with a shared secret.
//! Here the service owns the whole path and nothing in a request names an identity: the subject is the account row the service
//! itself looked up, the role of a new account is the configured default, and the actor behind a token is the principal the
//! service bound, never a claim. The password is checked by the official Python extension (`product/providers/password_auth.py`)
//! in the sandboxed worker, with the stored hash read by the server from a column no resource exposes; the extension answers
//! with a verdict only. Refresh tokens are single use (`aip_migrate::mysql_auth_sessions`); the original accepted a used one
//! again, and this port does not.
use super::*;
use crate::credentials::CredentialsConfig;
use aip_auth::issuer::LocalIssuer;
use aip_migrate::mysql_auth_sessions::{Consume, MysqlAuthSessions, SessionPolicy};
use aip_migrate::mysql_driver::{CommitFailure, FencedMysqlTransaction, MysqlDeploymentFence, MysqlDriver, mysql_async};
use mysql_async::prelude::Queryable;
use mysql_async::{Transaction, Value as SqlValue};
use spike_v2_read::external::Mapping;
use spike_v2_read::plan::Caller;
use spike_v6_transport::credential_auth::{AuthReply, AuthRequest, CredentialAuth};
use spike_v6_transport::server::ReadExtensions;
use std::time::Duration;

/// At most this many password checks run at once: hashing is deliberately expensive, so unbounded concurrency is a way to
/// exhaust the host. Requests beyond it are told to come back instead of queueing.
const MAX_CONCURRENT_HASHING: usize = 4;
const HASH_DEADLINE: Duration = Duration::from_secs(10);
const HASH_QUEUE: Duration = Duration::from_secs(5);
/// A real, valid scrypt hash of a password nobody has. Checking a login for an unknown email against it keeps that
/// answer as slow as a wrong password, so response time does not say which emails have accounts.
const DECOY_HASH: &str = "scrypt:32768:8:1$U4NpDaHhisZBcgqK$2f67362298b732a37ce8754c4d643673eaccbd3631ced43122d83e24f1252c203bdf30d0cdf9ee5087b38c3255441d4efb564b104c8f663f613b11935a89450a";

struct Table {
    name: String,
    columns: std::collections::BTreeMap<String, String>,
}

impl Table {
    fn column(&self, field: &str) -> Result<String, Value> {
        self.columns
            .get(field)
            .filter(|column| valid_name(column))
            .cloned()
            .ok_or_else(|| error("CREDENTIALS_CONFIG", &format!("`{field}` 필드가 external_mapping에 없음")))
    }
}

fn valid_name(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

struct Favorites {
    table: String,
    id: String,
    user: String,
    value: String,
    times: Vec<String>,
    members: Vec<String>,
    min: usize,
}

pub(crate) struct CredentialService {
    driver: MysqlDriver,
    fence: MysqlDeploymentFence,
    meta: String,
    source: String,
    table: String,
    id: String,
    identifier: String,
    secret: String,
    name: String,
    role: String,
    active: String,
    false_cols: Vec<String>,
    time_cols: Vec<String>,
    default_role: String,
    favorites: Option<Favorites>,
    facts: Arc<Value>,
    wire: IdWire,
    managed: bool,
    issuer_name: String,
    issuer: LocalIssuer,
    sessions: MysqlAuthSessions,
    workers: ReadExtensions,
    launch: spike_v4_worker::WorkerLaunchConfig,
    verify_operation: String,
    hash_operation: String,
    cookie: String,
    cookie_path: String,
    cookie_secure: bool,
    cookie_same_site: String,
    refresh_max_age: u32,
    limits: Limits,
    hashing: tokio::sync::Semaphore,
}

struct Limits {
    password: (usize, usize),
    name: (usize, usize),
}

#[derive(Debug)]
enum Fail {
    Status(u16, &'static str, &'static str),
}

impl From<Fail> for AuthReply {
    fn from(Fail::Status(status, code, reason): Fail) -> Self {
        AuthReply::fail(status, code, reason)
    }
}

type Outcome<T> = Result<T, Fail>;

fn db<T>(_: T) -> Fail {
    Fail::Status(503, "WIRE.DB_UNAVAILABLE", "database_unavailable")
}

fn bad(reason: &'static str) -> Fail {
    Fail::Status(422, "WIRE.BAD_VALUE", reason)
}

fn invalid_credentials() -> Fail {
    Fail::Status(401, "WIRE.UNAUTHENTICATED", "invalid_credentials")
}

impl CredentialService {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn new(
        config: &CredentialsConfig,
        mapping: &Mapping,
        facts: Arc<Value>,
        wire: IdWire,
        driver: MysqlDriver,
        fence: MysqlDeploymentFence,
        meta: &str,
        workers: ReadExtensions,
        launch: spike_v4_worker::WorkerLaunchConfig,
        issuer_name: &str,
        audience: &str,
    ) -> Result<Self, Value> {
        let account =
            mapping.tables.get(&config.resource).ok_or_else(|| error("CREDENTIALS_CONFIG", "credentials.resource가 external_mapping에 없음"))?;
        let table = Table { name: account.table.clone(), columns: account.columns.clone() };
        if !valid_name(&table.name) || !valid_name(&config.secret_column) {
            return Err(error("CREDENTIALS_CONFIG", "표·열 이름은 영숫자와 밑줄만 허용"));
        }
        if table.columns.values().any(|column| *column == config.secret_column) {
            return Err(error("CREDENTIALS_CONFIG", "secret_column이 resource 필드에 매핑되어 있어 읽기로 노출될 수 있음"));
        }
        if facts["actor"] != config.resource.as_str() {
            return Err(error("CREDENTIALS_CONFIG", "credentials.resource는 정의의 actor여야 함"));
        }
        let mut false_cols = Vec::new();
        for field in &config.false_fields {
            false_cols.push(table.column(field)?);
        }
        let mut time_cols = Vec::new();
        for field in &config.time_fields {
            time_cols.push(table.column(field)?);
        }
        let favorites = match &config.favorites {
            None => None,
            Some(favorites) => {
                let child = mapping
                    .tables
                    .get(&favorites.resource)
                    .ok_or_else(|| error("CREDENTIALS_CONFIG", "credentials.favorites.resource가 external_mapping에 없음"))?;
                let child = Table { name: child.table.clone(), columns: child.columns.clone() };
                let enum_name = facts["resources"][&favorites.resource]["fields"][&favorites.value_field]["ty"]
                    .as_str()
                    .and_then(|ty| ty.trim_end_matches('?').strip_prefix("Enum<")?.strip_suffix('>'))
                    .ok_or_else(|| error("CREDENTIALS_CONFIG", "favorites.value_field는 enum 필드여야 함"))?;
                let members: Vec<String> = facts["enums"][enum_name]
                    .as_array()
                    .map(|members| members.iter().filter_map(|member| member.as_str().map(str::to_owned)).collect())
                    .unwrap_or_default();
                if members.is_empty() || !valid_name(&child.name) {
                    return Err(error("CREDENTIALS_CONFIG", "favorites enum 값을 정의에서 찾지 못함"));
                }
                let mut times = Vec::new();
                for field in &favorites.time_fields {
                    times.push(child.column(field)?);
                }
                Some(Favorites {
                    id: child.column("id")?,
                    user: child.column(&favorites.user_field)?,
                    value: child.column(&favorites.value_field)?,
                    table: child.name,
                    times,
                    members,
                    min: favorites.min,
                })
            }
        };
        for operation in [&config.verify_operation, &config.hash_operation] {
            let declared = &facts["operations"][operation.as_str()];
            if declared["kind"] != "read" || declared["effect"] != "none" || !declared["access"].as_object().is_some_and(|access| access.is_empty()) {
                return Err(error("CREDENTIALS_CONFIG", "credentials 연산은 effect none인 `operation read`여야 함"));
            }
        }
        if config.cookie_name.is_empty()
            || !config.cookie_name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || !config.cookie_path.starts_with('/')
            || config.cookie_path.bytes().any(|b| b.is_ascii_control() || b == b';' || b == b' ')
            || !matches!(config.cookie_same_site.as_str(), "None" | "Lax" | "Strict")
            || (config.cookie_same_site == "None" && !config.cookie_secure)
            || config.refresh_idle_days == 0
            || config.refresh_absolute_days < config.refresh_idle_days
            || config.password_min_chars == 0
            || config.password_max_chars < config.password_min_chars
            || config.name_min_chars == 0
            || config.name_max_chars < config.name_min_chars
        {
            return Err(error("CREDENTIALS_CONFIG", "쿠키·길이·수명 설정이 올바르지 않음 (SameSite=None은 Secure가 필요)"));
        }
        let pem = std::fs::read(&config.signing_key_file).map_err(|_| error("CREDENTIALS_CONFIG", "signing_key_file을 읽을 수 없음"))?;
        let issuer = LocalIssuer::from_pem(&pem, &config.kid, issuer_name, audience, config.access_ttl_secs)
            .map_err(|_| error("CREDENTIALS_CONFIG", "서명 키·kid·access_ttl_secs가 올바르지 않음"))?;
        let policy = SessionPolicy { idle_secs: config.refresh_idle_days * 86_400, absolute_secs: config.refresh_absolute_days * 86_400 };
        let sessions = MysqlAuthSessions::new(meta, policy).map_err(|_| error("CREDENTIALS_CONFIG", "세션 정책이 올바르지 않음"))?;
        Ok(Self {
            managed: facts["actorMode"] == "principal",
            id: table.column("id")?,
            identifier: table.column(&config.identifier_field)?,
            name: table.column(&config.name_field)?,
            role: table.column(&config.role_field)?,
            active: table.column(&config.active_field)?,
            secret: config.secret_column.clone(),
            table: table.name,
            false_cols,
            time_cols,
            default_role: config.default_role.clone(),
            favorites,
            facts,
            wire,
            driver,
            fence,
            meta: meta.to_owned(),
            source: mapping.schema.clone(),
            issuer_name: issuer_name.to_owned(),
            issuer,
            sessions,
            workers,
            launch,
            verify_operation: config.verify_operation.clone(),
            hash_operation: config.hash_operation.clone(),
            cookie: config.cookie_name.clone(),
            cookie_path: config.cookie_path.clone(),
            cookie_secure: config.cookie_secure,
            cookie_same_site: config.cookie_same_site.clone(),
            refresh_max_age: policy.idle_secs,
            limits: Limits { password: (config.password_min_chars, config.password_max_chars), name: (config.name_min_chars, config.name_max_chars) },
            hashing: tokio::sync::Semaphore::new(MAX_CONCURRENT_HASHING),
        })
    }

    /// A token signed the way a login signs one, for the startup check that the verifier trusts the signing key.
    pub(crate) fn probe_token(&self) -> Result<String, Value> {
        self.issuer
            .issue("startup-probe", "startup-probe")
            .map(|minted| minted.token)
            .map_err(|_| error("CREDENTIALS_CONFIG", "서명 키로 토큰을 만들 수 없음"))
    }

    /// The sessions store, for the verifier that rejects an access token whose login session ended.
    pub(crate) fn sessions(&self) -> MysqlAuthSessions {
        self.sessions.clone()
    }

    /// A startup self-check: the table and columns exist, the identifier is unique (a duplicate signup race would otherwise
    /// create two accounts) and the secret column can hold a hash. Fails the start, never a request.
    pub(crate) async fn check_storage(&self) -> Result<(), Value> {
        let mut conn = self.driver.pool().get_conn().await.map_err(|_| error("CREDENTIALS_STORAGE", "DB 연결 실패"))?;
        let present: Vec<(String, String, Option<u64>)> = conn
            .exec(
                "SELECT COLUMN_NAME, DATA_TYPE, CHARACTER_MAXIMUM_LENGTH FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
                (&self.source, &self.table),
            )
            .await
            .map_err(|_| error("CREDENTIALS_STORAGE", "표 구조를 읽을 수 없음"))?;
        let secret = present.iter().find(|(name, _, _)| *name == self.secret);
        if !secret.is_some_and(|(_, kind, length)| kind == "varchar" && length.is_some_and(|length| length >= 255)) {
            return Err(error("CREDENTIALS_STORAGE", "secret_column은 varchar(255) 이상이어야 함"));
        }
        let indexes: Vec<(String, String, i64)> = conn
            .exec(
                "SELECT INDEX_NAME, COLUMN_NAME, NON_UNIQUE FROM INFORMATION_SCHEMA.STATISTICS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?",
                (&self.source, &self.table),
            )
            .await
            .map_err(|_| error("CREDENTIALS_STORAGE", "인덱스를 읽을 수 없음"))?;
        let unique_on_identifier = indexes.iter().any(|(index, column, non_unique)| {
            *non_unique == 0 && *column == self.identifier && indexes.iter().filter(|(other, _, _)| other == index).count() == 1
        });
        if !unique_on_identifier {
            return Err(error("CREDENTIALS_STORAGE", "identifier 열에 단일 열 UNIQUE 인덱스가 필요함"));
        }
        let tables: Option<u8> = conn
            .exec_first("SELECT 1 FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = 'aip_auth_refresh_tokens'", (&self.meta,))
            .await
            .map_err(|_| error("CREDENTIALS_STORAGE", "세션 표를 확인할 수 없음"))?;
        if tables.is_none() {
            return Err(error("CREDENTIALS_STORAGE", "세션 표가 없음: migration 계정으로 adopt-external --apply를 다시 실행해야 함"));
        }
        Ok(())
    }

    fn src(&self, name: &str) -> String {
        format!("`{}`.`{name}`", self.source)
    }

    fn meta_table(&self, name: &str) -> String {
        format!("`{}`.`{name}`", self.meta)
    }

    async fn begin(&self) -> Outcome<FencedMysqlTransaction> {
        let mut fenced = self.driver.begin_fenced_transaction(false, &self.fence).await.map_err(|error| {
            if error.code == "MIGRATE.DRIFT_DETECTED" { Fail::Status(503, "WIRE.DEPLOYMENT_CHANGED", "deployment_changed") } else { db(error) }
        })?;
        let tx = fenced.as_mut().map_err(db)?;
        tx.query_drop("SET SESSION time_zone = '+00:00', SESSION innodb_lock_wait_timeout = 3, SESSION max_execution_time = 5000")
            .await
            .map_err(db)?;
        Ok(fenced)
    }

    async fn commit(fenced: FencedMysqlTransaction) -> Outcome<()> {
        fenced.commit_classified(|| ()).await.map_err(|failure| match failure {
            CommitFailure::Unknown => Fail::Status(503, "WIRE.COMMIT_UNKNOWN", "commit_unknown"),
            _ => db(()),
        })
    }

    /// One call of an extension operation in a fresh sandboxed worker. The input is built by the server, never by a request.
    async fn run_operation(&self, operation: &str, input: Value) -> Outcome<Value> {
        // A short queue absorbs a burst of signups; a request that would wait longer than the queue allows is turned away.
        let Ok(Ok(_permit)) = tokio::time::timeout(HASH_QUEUE, self.hashing.acquire()).await else {
            return Err(Fail::Status(429, "WIRE.RATE_LIMITED", "busy"));
        };
        let dir = self.workers.dir.to_str().ok_or(Fail::Status(503, "WIRE.WORKER_FAILED", "worker_failed"))?;
        let caller = Caller { actor_id: None, now: "1970-01-01T00:00:00.000000Z".into() };
        let work = async {
            let mut worker = spike_v4_worker::Worker::try_start_configured(self.workers.lang, dir, self.launch.clone())
                .await
                .map_err(|_| Fail::Status(503, "WIRE.WORKER_FAILED", "worker_failed"))?;
            let result = spike_v4_worker::invoke_operation(&mut worker, &self.facts, operation, &input, &caller, self.wire).await;
            worker.stop().await;
            result.map_err(|_| Fail::Status(503, "WIRE.WORKER_FAILED", "worker_failed"))
        };
        tokio::time::timeout(HASH_DEADLINE, work).await.map_err(|_| Fail::Status(503, "WIRE.DEADLINE_EXCEEDED", "deadline_exceeded"))?
    }

    /// The verdict, and whether the stored hash is weaker than the current default so a good login should replace it.
    async fn verify(&self, hash: &str, password: &str) -> Outcome<(bool, bool)> {
        let output = self.run_operation(&self.verify_operation, json!({"password": password, "hash": hash})).await?;
        Ok((output["ok"] == true, output["rehash"] == true))
    }

    async fn hash(&self, password: &str) -> Outcome<String> {
        let output = self.run_operation(&self.hash_operation, json!({"password": password})).await?;
        output["hash"].as_str().filter(|hash| !hash.is_empty() && hash.len() <= 255).map(str::to_owned).ok_or(Fail::Status(
            503,
            "WIRE.OUTPUT_INVALID",
            "hash_invalid",
        ))
    }

    fn cookie_value(&self, token: &str, max_age: u32) -> String {
        let secure = if self.cookie_secure { "; Secure" } else { "" };
        format!("{}={token}; Max-Age={max_age}; Path={}; HttpOnly{secure}; SameSite={}", self.cookie, self.cookie_path, self.cookie_same_site)
    }

    fn clear_cookie(&self) -> String {
        self.cookie_value("", 0)
    }

    fn refresh_from(&self, header: Option<&str>) -> Option<String> {
        let prefix = format!("{}=", self.cookie);
        header?
            .split(';')
            .map(str::trim)
            .find_map(|part| part.strip_prefix(prefix.as_str()))
            .filter(|value| value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()))
            .map(str::to_owned)
    }

    fn email(&self, body: &Value) -> Outcome<String> {
        let email = body["email"].as_str().ok_or(bad("email_required"))?;
        valid_email(email).then(|| email.to_owned()).ok_or(bad("email_invalid"))
    }

    fn only_keys(body: &Value, allowed: &[&str]) -> Outcome<()> {
        match body.as_object() {
            Some(object) if object.keys().all(|key| allowed.contains(&key.as_str())) => Ok(()),
            _ => Err(bad("unknown_field")),
        }
    }

    async fn check_email(&self, body: &Value) -> Outcome<AuthReply> {
        Self::only_keys(body, &["email"])?;
        let email = self.email(body)?;
        let mut conn = self.driver.pool().get_conn().await.map_err(db)?;
        let found: Option<u8> = conn
            .exec_first(format!("SELECT 1 FROM {} WHERE `{}` = ? LIMIT 1", self.src(&self.table), self.identifier), (&email,))
            .await
            .map_err(db)?;
        let duplicate = found.is_some();
        let message = if duplicate { "이미 가입된 이메일입니다." } else { "사용 가능한 이메일입니다." };
        Ok(AuthReply { status: 200, body: json!({"ok": true, "is_duplicate": duplicate, "message": message}), set_cookie: None })
    }

    async fn signup(&self, body: &Value) -> Outcome<AuthReply> {
        Self::only_keys(body, &["email", "password", "name", "favorite_genres"])?;
        let email = self.email(body)?;
        let password = body["password"].as_str().ok_or(bad("password_required"))?;
        if !(self.limits.password.0..=self.limits.password.1).contains(&password.chars().count()) {
            return Err(bad("password_length"));
        }
        let name = body["name"].as_str().ok_or(bad("name_required"))?;
        if !(self.limits.name.0..=self.limits.name.1).contains(&name.chars().count()) || name.chars().any(char::is_control) {
            return Err(bad("name_length"));
        }
        let mut genres: Vec<&str> = Vec::new();
        if let Some(favorites) = &self.favorites {
            let listed = body["favorite_genres"].as_array().ok_or(bad("favorite_genres_required"))?;
            for genre in listed {
                let genre =
                    genre.as_str().filter(|genre| favorites.members.iter().any(|member| member == genre)).ok_or(bad("favorite_genre_unknown"))?;
                if !genres.contains(&genre) {
                    genres.push(genre);
                }
            }
            if genres.len() < favorites.min {
                return Err(bad("favorite_genres_required"));
            }
        } else if !body["favorite_genres"].is_null() {
            return Err(bad("unknown_field"));
        }
        let hash = self.hash(password).await?;
        let user_id = uuid::Uuid::new_v4().to_string();

        let mut fenced = self.begin().await?;
        let tx = fenced.as_mut().map_err(db)?;
        // No look-before-insert: the identifier's UNIQUE index (checked at startup) decides. A locking pre-read would only add gap
        // locks that make two concurrent signups deadlock.
        let mut columns =
            vec![self.id.clone(), self.identifier.clone(), self.secret.clone(), self.name.clone(), self.role.clone(), self.active.clone()];
        let mut values = vec!["?".to_owned(), "?".into(), "?".into(), "?".into(), "?".into(), "0".into()];
        for column in &self.false_cols {
            columns.push(column.clone());
            values.push("0".into());
        }
        for column in &self.time_cols {
            columns.push(column.clone());
            values.push("UTC_TIMESTAMP()".into());
        }
        let insert = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            self.src(&self.table),
            columns.iter().map(|column| format!("`{column}`")).collect::<Vec<_>>().join(", "),
            values.join(", ")
        );
        let params: Vec<SqlValue> =
            vec![user_id.as_str().into(), email.as_str().into(), hash.as_str().into(), name.into(), self.default_role.as_str().into()];
        if let Err(error) = tx.exec_drop(insert, params).await {
            return Err(if matches!(&error, mysql_async::Error::Server(server) if server.code == 1062) {
                Fail::Status(409, "WIRE.ALREADY_EXISTS", "duplicate_email")
            } else {
                db(error)
            });
        }
        if let Some(favorites) = &self.favorites {
            for genre in genres {
                let mut columns = vec![favorites.id.clone(), favorites.user.clone(), favorites.value.clone()];
                let mut values = vec!["?".to_owned(), "?".into(), "?".into()];
                for column in &favorites.times {
                    columns.push(column.clone());
                    values.push("UTC_TIMESTAMP()".into());
                }
                tx.exec_drop(
                    format!(
                        "INSERT INTO {} ({}) VALUES ({})",
                        self.src(&favorites.table),
                        columns.iter().map(|column| format!("`{column}`")).collect::<Vec<_>>().join(", "),
                        values.join(", ")
                    ),
                    (uuid::Uuid::new_v4().to_string(), &user_id, genre),
                )
                .await
                .map_err(db)?;
            }
        }
        self.bind(tx, &user_id).await?;
        let session = self.sessions.open(tx, &user_id).await.map_err(db)?;
        Self::commit(fenced).await?;
        self.tokens(&user_id, &session, 201)
    }

    /// Binds the account to its actor once, and refuses an account an operator has revoked: a login must not undo
    /// `service principal revoke`.
    async fn bind(&self, tx: &mut Transaction<'static>, user_id: &str) -> Outcome<()> {
        let existing: Option<i64> = tx
            .exec_first(
                format!("SELECT enabled FROM {} WHERE issuer = ? AND subject = ?", self.meta_table("aip_principals")),
                (&self.issuer_name, user_id),
            )
            .await
            .map_err(db)?;
        match existing {
            Some(1) => Ok(()),
            Some(_) => Err(Fail::Status(403, "WIRE.ACCESS_DENIED", "account_disabled")),
            None => {
                let (user, issuer) = (user_id.to_owned(), self.issuer_name.clone());
                crate::mysql_service::bind_principal(tx, &self.meta, &self.facts, self.wire, self.managed, &issuer, &user, &user, 0)
                    .await
                    .map_err(|_| Fail::Status(503, "WIRE.DB_UNAVAILABLE", "principal_bind_failed"))
            }
        }
    }

    fn tokens(&self, user_id: &str, session: &aip_migrate::mysql_auth_sessions::Issued, status: u16) -> Outcome<AuthReply> {
        let access = self.issuer.issue(user_id, &session.family).map_err(|_| Fail::Status(503, "WIRE.DB_UNAVAILABLE", "token_unavailable"))?;
        Ok(AuthReply {
            status,
            body: json!({"ok": true, "access_token": access.token, "expires_at": access.expires_at, "token_type": "Bearer"}),
            set_cookie: Some(self.cookie_value(&session.refresh_token, self.refresh_max_age)),
        })
    }

    async fn login(&self, body: &Value) -> Outcome<AuthReply> {
        Self::only_keys(body, &["email", "password"])?;
        let email = self.email(body)?;
        let password = body["password"].as_str().ok_or(bad("password_required"))?;
        if password.is_empty() || password.chars().count() > self.limits.password.1 {
            return Err(invalid_credentials());
        }
        let mut conn = self.driver.pool().get_conn().await.map_err(db)?;
        let row: Option<(String, String)> = conn
            .exec_first(
                format!(
                    "SELECT `{}`, `{}` FROM {} WHERE `{}` = ? AND `{}` = 0",
                    self.id,
                    self.secret,
                    self.src(&self.table),
                    self.identifier,
                    self.active
                ),
                (&email,),
            )
            .await
            .map_err(db)?;
        drop(conn);
        let Some((user_id, hash)) = row else {
            // Same work as a wrong password, so the answer's timing does not tell which emails exist.
            let _ = self.verify(DECOY_HASH, password).await?;
            return Err(invalid_credentials());
        };
        let (verified, rehash) = self.verify(&hash, password).await?;
        if !verified {
            return Err(invalid_credentials());
        }
        // A hash made with weaker parameters than today's default is replaced after the proof of the password. The new hash is
        // made before the transaction opens, so the account row is not locked during the hashing.
        let upgraded = if rehash { Some(self.hash(password).await?) } else { None };
        let mut fenced = self.begin().await?;
        let tx = fenced.as_mut().map_err(db)?;
        // The account may have been suspended while the password was being checked.
        let active: Option<u8> = tx
            .exec_first(format!("SELECT 1 FROM {} WHERE `{}` = ? AND `{}` = 0 FOR SHARE", self.src(&self.table), self.id, self.active), (&user_id,))
            .await
            .map_err(db)?;
        if active.is_none() {
            return Err(invalid_credentials());
        }
        if let Some(upgraded) = &upgraded {
            // Only while the stored hash is still the one that was checked, so a password changed in between is not undone.
            tx.exec_drop(
                format!("UPDATE {} SET `{}` = ? WHERE `{}` = ? AND `{}` = ?", self.src(&self.table), self.secret, self.id, self.secret),
                (upgraded, &user_id, &hash),
            )
            .await
            .map_err(db)?;
        }
        self.bind(tx, &user_id).await?;
        let session = self.sessions.open(tx, &user_id).await.map_err(db)?;
        Self::commit(fenced).await?;
        self.tokens(&user_id, &session, 200)
    }

    async fn reissue(&self, cookie: Option<&str>) -> Outcome<AuthReply> {
        let denied =
            |reason: &'static str| AuthReply { set_cookie: Some(self.clear_cookie()), ..AuthReply::fail(401, "WIRE.UNAUTHENTICATED", reason) };
        let Some(presented) = self.refresh_from(cookie) else { return Ok(denied("refresh_token_missing")) };
        let mut fenced = self.begin().await?;
        let tx = fenced.as_mut().map_err(db)?;
        let consumed = match self.sessions.consume(tx, &presented).await.map_err(db)? {
            Consume::Fresh(consumed) => consumed,
            other => {
                // A replay revoked its session inside `consume`; that has to commit.
                Self::commit(fenced).await?;
                return Ok(denied(match other {
                    Consume::Replayed { .. } => "refresh_token_reused",
                    Consume::Expired => "refresh_token_expired",
                    Consume::Revoked => "session_revoked",
                    _ => "refresh_token_invalid",
                }));
            }
        };
        let active: Option<u8> = tx
            .exec_first(
                format!("SELECT 1 FROM {} WHERE `{}` = ? AND `{}` = 0 FOR SHARE", self.src(&self.table), self.id, self.active),
                (&consumed.subject,),
            )
            .await
            .map_err(db)?;
        let enabled: Option<i64> = tx
            .exec_first(
                format!("SELECT enabled FROM {} WHERE issuer = ? AND subject = ? FOR SHARE", self.meta_table("aip_principals")),
                (&self.issuer_name, &consumed.subject),
            )
            .await
            .map_err(db)?;
        if active.is_none() || enabled != Some(1) {
            // Every session of the account ends, not only this one: its other devices must not outlive the suspension.
            self.sessions.revoke_subject(tx, &consumed.subject, "account_inactive").await.map_err(db)?;
            Self::commit(fenced).await?;
            return Ok(denied("account_inactive"));
        }
        let session = self.sessions.issue_next(tx, &consumed).await.map_err(db)?;
        let subject = consumed.subject;
        Self::commit(fenced).await?;
        self.tokens(&subject, &session, 200)
    }

    async fn logout(&self, cookie: Option<&str>) -> Outcome<AuthReply> {
        if let Some(presented) = self.refresh_from(cookie) {
            let mut conn = self.driver.pool().get_conn().await.map_err(db)?;
            self.sessions.revoke_by_token(&mut conn, &presented, "logout").await.map_err(db)?;
        }
        // The answer is the same whether or not a session ended, so it is no oracle for a token's state.
        Ok(AuthReply { status: 200, body: json!({"ok": true, "message": "로그아웃되었습니다."}), set_cookie: Some(self.clear_cookie()) })
    }
}

/// A conservative reading of the original's marshmallow `Email`: one `@`, a non-empty local part and a dotted domain, no
/// whitespace or control characters, within the column's 255 bytes.
pub(crate) fn valid_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else { return false };
    email.len() <= 255
        && !local.is_empty()
        && local.len() <= 64
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
        && !email.chars().any(|c| c.is_whitespace() || c.is_control())
}

impl CredentialAuth for CredentialService {
    fn handle<'a>(&'a self, request: AuthRequest<'a>) -> spike_v2_read::backend::BackendFuture<'a, AuthReply> {
        Box::pin(async move {
            let result = match request.route {
                "/auth/check-email" => self.check_email(&request.body).await,
                "/auth/signup" => self.signup(&request.body).await,
                "/auth/login" => self.login(&request.body).await,
                "/auth/reissue" => self.reissue(request.cookie).await,
                "/auth/logout" => self.logout(request.cookie).await,
                _ => Err(Fail::Status(404, "WIRE.NOT_FOUND", "unknown_route")),
            };
            result.unwrap_or_else(AuthReply::from)
        })
    }
}
