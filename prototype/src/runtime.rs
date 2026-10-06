use crate::{error, load, Wire};
use serde_json::{json, Value};
use spike_v2_read::sqlgen;
use spike_v2_read::OwnedConnection;
use std::{net::SocketAddr, path::Path};

fn configure_schema(schema: &str) -> Result<(), Value> {
    let bytes = schema.as_bytes();
    if bytes.len() > 63
        || !bytes.starts_with(b"aip_")
        || !bytes.get(4).is_some_and(u8::is_ascii_lowercase)
        || !bytes[5..].iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
    {
        return Err(error("BAD_SCHEMA", "schema는 aip_ + 영문 소문자로 시작하는 ASCII 식별자이며 63바이트 이하이어야 함"));
    }
    sqlgen::set_schema(schema);
    Ok(())
}

async fn connect(url: &str) -> Result<OwnedConnection, Value> {
    use tokio_postgres::config::{Host, SslMode};
    let config = url.parse::<tokio_postgres::Config>().map_err(|_| error("DB_CONFIG", "잘못된 DB 설정"))?;
    let local = config.get_hosts().iter().all(|host| match host {
        Host::Tcp(host) => host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()),
        #[cfg(unix)]
        Host::Unix(_) => true,
    }) && config.get_hostaddrs().iter().all(std::net::IpAddr::is_loopback);
    if !local || config.get_ssl_mode() == SslMode::Require {
        return Err(error("DB_CONFIG", "현재 DB adapter는 로컬 개발 DB만 지원하며 TLS 필수 연결은 지원하지 않음"));
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), spike_v2_read::connect_owned_with_url(url))
        .await
        .map_err(|_| error("DB_CONNECT", "DB 연결 시간 초과"))?
        .map_err(|_| error("DB_CONNECT", "지정한 DB에 연결할 수 없음"))
}

fn creation(facts: &Value) -> Result<(Vec<String>, String), Value> {
    let mut statements = sqlgen::create_ddl(facts).map_err(|_| error("UNSUPPORTED_SCHEMA", "이 정의의 DB 구조를 생성할 수 없음"))?;
    statements.push(spike_v6_transport::idempotency_ddl());
    let digest = spike_v1_fixture::digest(&json!(statements));
    Ok((statements, digest))
}

async fn structure_digest(db: &impl tokio_postgres::GenericClient, schema: &str) -> Result<String, Value> {
    // PostgreSQL deparsers qualify names according to search_path. Keep both snapshots identical.
    db.batch_execute("SET LOCAL search_path TO pg_catalog").await.map_err(|_| error("DB_CATALOG", "DB 구조 검사 설정 실패"))?;
    let row = db.query_one(include_str!("schema_catalog.sql"), &[&schema]).await.map_err(|_| error("DB_CATALOG", "DB 구조를 확인할 수 없음"))?;
    let text = row.try_get::<_, String>(0).map_err(|_| error("DB_CATALOG", "DB 구조 결과 형식 오류"))?;
    let value: Value = serde_json::from_str(&text).map_err(|_| error("DB_CATALOG", "DB 구조 결과 형식 오류"))?;
    Ok(spike_v1_fixture::digest(&json!({"format":"prototype-catalog-v1","catalog":value})))
}

pub async fn init(file: &Path, schema: &str, db_url: &str) -> Result<Value, Value> {
    configure_schema(schema)?;
    let checked = load(file, false)?;
    let (statements, digest) = creation(&checked.output.execution)?;
    let mut db = connect(db_url).await?;
    let stage = async {
        let tx = db.transaction().await.map_err(|_| error("DB_INIT", "초기화 트랜잭션을 시작할 수 없음"))?;
        for (index, statement) in statements.iter().enumerate() {
            tx.batch_execute(statement).await.map_err(|e| {
                if index == 0 && e.code().is_some_and(|c| c.code() == "42P06") {
                    error("SCHEMA_EXISTS", "기존 schema는 초기화하지 않음")
                } else {
                    error("DB_INIT", "schema 생성 실패. 초기화 트랜잭션을 되돌림")
                }
            })?;
        }
        tx.batch_execute(&format!("CREATE TABLE {schema}.aip_proto_meta (singleton boolean PRIMARY KEY CHECK(singleton), ddl_digest text NOT NULL, structure_digest text NOT NULL)"))
        .await
        .map_err(|_| error("DB_INIT", "초기화 marker 생성 실패"))?;
        let structure = structure_digest(&tx, schema).await?;
        tx.execute(&format!("INSERT INTO {schema}.aip_proto_meta VALUES (true,$1,$2)"), &[&digest, &structure])
            .await
            .map_err(|_| error("DB_INIT", "초기화 marker 저장 실패"))?;
        tx.commit().await.map_err(|_| error("INIT_UNSETTLED", "초기화 커밋 결과 불명. schema 상태를 확인해야 함"))?;
        Ok(json!({"ok":true,"schema":schema,"ddlDigest":digest}))
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), stage)
        .await
        .map_err(|_| error("INIT_UNSETTLED", "초기화 SQL 기한 초과. 커밋 여부는 schema 상태로 확인해야 함"))?
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum WorkerLanguage {
    Node,
    Python,
}

#[derive(clap::Args)]
pub struct Serve {
    pub file: std::path::PathBuf,
    #[arg(long)]
    pub schema: String,
    #[arg(long)]
    pub db_url: String,
    #[arg(long)]
    pub listen: SocketAddr,
    #[arg(long, value_enum)]
    pub wire: Wire,
    #[arg(long)]
    pub dev_actor: Option<i64>,
    #[arg(long, default_value_t = 60, requires = "dev_actor")]
    pub dev_token_ttl: i64,
    #[arg(long, requires = "worker_lang")]
    pub worker_dir: Option<std::path::PathBuf>,
    #[arg(long, value_enum, requires = "worker_dir")]
    pub worker_lang: Option<WorkerLanguage>,
    #[arg(long)]
    pub allow_origin: Vec<String>,
    #[arg(long, requires_all=["worker_dir", "worker_lang"])]
    pub enable_write_extensions: bool,
}

pub async fn serve(args: Serve) -> Result<Value, Value> {
    configure_schema(&args.schema)?;
    if !args.listen.ip().is_loopback() {
        return Err(error("BAD_LISTEN", "현재 프로토타입은 loopback 주소만 사용함"));
    }
    if args.dev_actor.is_some() && (!(1..=300).contains(&args.dev_token_ttl) || args.dev_actor.is_some_and(|id| id <= 0)) {
        return Err(error("BAD_DEV_SESSION", "개발 actor는 양의 Id, 토큰 수명은 1~300초여야 함"));
    }
    let options = spike_v6_transport::server::ServerOptions { allowed_origins: args.allow_origin };
    options.validate().map_err(|_| error("BAD_ORIGIN", "허용 출처는 정규화한 HTTP loopback origin이어야 함"))?;
    let facts = load(&args.file, false)?.output.execution;
    let extensions = match (args.worker_dir, args.worker_lang) {
        (Some(dir), Some(lang)) => {
            use spike_v6_transport::server::{ReadExtensions, WorkerLang};
            let config = ReadExtensions {
                dir,
                lang: match lang {
                    WorkerLanguage::Node => WorkerLang::Node,
                    WorkerLanguage::Python => WorkerLang::Python,
                },
            };
            let checked = if args.enable_write_extensions {
                config.validate_with_writes(&facts, args.wire.id_wire())
            } else {
                config.validate(&facts, args.wire.id_wire())
            };
            Some(checked.map_err(|_| error("WORKER_CONFIG", "macOS 로컬 공식 확장 경로/구현 설정 오류"))?)
        }
        (None, None) => None,
        _ => return Err(error("WORKER_CONFIG", "worker-dir와 worker-lang을 함께 지정해야 함")),
    };
    let (statements, expected) = creation(&facts)?;
    let mut db = connect(&args.db_url).await?;
    let stage = async {
        let tx = db.transaction().await.map_err(|_| error("SCHEMA_NOT_READY", "DB 기동 검사 시작 실패"))?;
        tx.batch_execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .await
            .map_err(|_| error("SCHEMA_NOT_READY", "DB 기동 검사 설정 실패"))?;
        let marker = tx
            .query_opt(&format!("SELECT ddl_digest, structure_digest FROM {}.aip_proto_meta WHERE singleton=true", args.schema), &[])
            .await
            .map_err(|_| error("SCHEMA_NOT_READY", "현재 prototype init의 구조 marker가 필요함. 구형 schema는 자동 변경하지 않음"))?;
        let actual = marker.as_ref().and_then(|row| row.try_get::<_, String>(0).ok());
        if actual.as_deref() != Some(&expected) {
            return Err(error("SCHEMA_MISMATCH", "정의의 DB 구조가 초기화 구조와 다름. 마이그레이션은 미구현"));
        }
        for statement in &statements[1..] {
            if let Some(table) = statement.strip_prefix("CREATE TABLE ").and_then(|s| s.split_whitespace().next()) {
                let exists = tx
                    .query_one("SELECT to_regclass($1) IS NOT NULL", &[&table])
                    .await
                    .map_err(|_| error("SCHEMA_NOT_READY", "schema 테이블 확인 실패"))?;
                if !exists.get::<_, bool>(0) {
                    return Err(error("SCHEMA_NOT_READY", "초기화 테이블이 없음"));
                }
            }
        }
        let saved = marker.and_then(|row| row.try_get::<_, String>(1).ok());
        if saved.as_deref() != Some(&structure_digest(&tx, &args.schema).await?) {
            return Err(error("SCHEMA_MISMATCH", "초기화 이후 DB 구조가 달라짐. serve는 구조나 데이터를 변경하지 않음"));
        }
        if let Some(id) = args.dev_actor {
            let actor = facts["actor"].as_str().ok_or_else(|| error("BAD_DEV_SESSION", "actor 선언이 없음"))?;
            if tx
                .query_opt(&format!("SELECT id FROM {} WHERE id=$1", sqlgen::table(actor)), &[&id])
                .await
                .map_err(|_| error("BAD_DEV_SESSION", "개발 actor 조회 실패"))?
                .is_none()
            {
                return Err(error("BAD_DEV_SESSION", "개발 actor 행이 없음. serve는 데이터를 생성하지 않음"));
            }
        }
        tx.commit().await.map_err(|_| error("SCHEMA_NOT_READY", "DB 기동 검사 종료 실패"))?;
        Ok::<(), Value>(())
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), stage).await.map_err(|_| error("DB_PREFLIGHT", "DB 기동 검사 SQL 기한 초과"))??;
    drop(db);
    let wire = args.wire.id_wire();
    let mut signals = StopSignals::new()?;
    let mut server = spike_v6_transport::server::listen_with_prototype_options(
        facts.clone(),
        wire,
        args.listen,
        args.db_url,
        extensions,
        options,
        args.enable_write_extensions,
    )
    .await
    .map_err(|_| error("LISTEN_FAILED", "지정한 서버 주소를 사용할 수 없음"))?;
    let mut event = json!({"ok":true,"event":"listening","address":server.address.to_string(),"fingerprint":spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts,wire),"idWire":wire.label()});
    if let Some(actor) = args.dev_actor {
        event["devToken"] = json!(server.keys.issue(actor, args.dev_token_ttl));
    }
    if crate::report(&event).is_err() {
        server.shutdown().await.map_err(|_| error("SERVER_STOP", "기동 결과 출력 실패 후 서버 종료 실패"))?;
        return Err(error("OUTPUT_IO", "기동 결과 출력 실패"));
    }
    tokio::select! {
        result=signals.wait()=>{result?;server.shutdown().await.map_err(|_|error("SERVER_STOP","서버 종료 실패"))?;},
        result=server.wait()=>{result.map_err(|_|error("SERVER_FAILED","서버 실행 실패"))?;},
    }
    Ok(json!({"ok":true,"event":"stopped"}))
}

struct StopSignals {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
}

impl StopSignals {
    fn new() -> Result<Self, Value> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let register = |kind| signal(kind).map_err(|_| error("SIGNAL_FAILED", "종료 신호 등록 실패"));
            Ok(Self { interrupt: register(SignalKind::interrupt())?, terminate: register(SignalKind::terminate())? })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {})
        }
    }

    async fn wait(&mut self) -> Result<(), Value> {
        #[cfg(unix)]
        {
            let received = tokio::select! {signal=self.interrupt.recv()=>signal,signal=self.terminate.recv()=>signal};
            received.ok_or_else(|| error("SIGNAL_FAILED", "종료 신호 채널이 닫힘"))
        }
        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c().await.map_err(|_| error("SIGNAL_FAILED", "종료 신호 실패"))
        }
    }
}
