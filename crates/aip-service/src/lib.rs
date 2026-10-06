//! Product assembly reuses the validated definition, authorization, SQL and SDK engines.
use clap::{Subcommand, ValueEnum};
use serde::Deserialize;
use serde_json::{Value, json};
use spike_v2_read::{id_wire::IdWire, sqlgen};
use std::{
    fs,
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

const SOURCE_BYTES: usize = 1 << 20;
const CONFIG_BYTES: usize = 64 << 10;

#[derive(Subcommand)]
pub enum Command {
    /// Validate any of the five caller-definition formats
    Check {
        file: PathBuf,
        #[arg(long)]
        dev_checks: bool,
    },
    /// Generate a caller read/apply binding for the shared product SDK
    Gen {
        file: PathBuf,
        #[arg(long, value_enum, default_value = "decimal")]
        wire: Wire,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value = "@aip/sdk")]
        sdk_import: String,
    },
    /// Create a fresh owned schema and its deployment journal
    Init {
        file: PathBuf,
        #[arg(long)]
        config: PathBuf,
    },
    /// Print a deployment plan, or apply exactly the acknowledged plan digest
    Migrate {
        file: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        migration: Option<PathBuf>,
        #[arg(long)]
        apply: Option<String>,
    },
    /// Adopt a verified prototype schema while preserving rows and idempotency records
    Adopt {
        file: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        apply: Option<String>,
    },
    /// Serve a previously initialized deployment behind a TLS reverse proxy
    Serve {
        file: PathBuf,
        #[arg(long)]
        config: PathBuf,
    },
    /// Operator-only persistent issuer/subject to actor mapping
    Principal {
        #[arg(long)]
        config: PathBuf,
        #[command(subcommand)]
        command: Principal,
    },
}

#[derive(Subcommand)]
pub enum Principal {
    Bind {
        #[arg(long)]
        subject: String,
        #[arg(long)]
        actor: i64,
        #[arg(long, default_value_t = 0)]
        not_before: i64,
    },
    Revoke {
        #[arg(long)]
        subject: String,
    },
}

#[derive(Clone, Copy, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Wire {
    Safe,
    Decimal,
}
impl Wire {
    fn deployment_label(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Decimal => "decimal",
        }
    }
    fn id_wire(self) -> IdWire {
        match self {
            Self::Safe => IdWire::SafeNumber,
            Self::Decimal => IdWire::DecimalString,
        }
    }
}
fn default_wire() -> Wire {
    Wire::Decimal
}
fn default_listen() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 8080))
}
fn default_env() -> String {
    "AIP_DATABASE_URL".into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema: String,
    #[serde(default = "default_env")]
    pub database_url_env: String,
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    #[serde(default = "default_wire")]
    pub wire: Wire,
    pub auth: aip_auth::Config,
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    pub workers: Option<Workers>,
    pub database_ca_file: Option<PathBuf>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workers {
    pub directory: PathBuf,
    pub language: Language,
    #[serde(default)]
    pub enable_writes: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Node,
    Python,
}

fn error(code: &str, message: &str) -> Value {
    json!({"ok":false,"code":code,"message":message})
}
fn read_bounded(path: &Path, limit: usize, code: &str) -> Result<String, Value> {
    let file = fs::File::open(path).map_err(|_| error(code, "파일을 열 수 없음"))?;
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Err(error(code, "일반 파일이 필요함"));
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64).read_to_end(&mut bytes).map_err(|_| error(code, "파일을 읽을 수 없음"))?;
    if bytes.len() > limit {
        return Err(error("INPUT_TOO_LARGE", "파일의 제품 입력 상한 초과"));
    }
    String::from_utf8(bytes).map_err(|_| error(code, "UTF-8 파일이 필요함"))
}
pub fn load(file: &Path, dev_checks: bool) -> Result<spike_v11_dev_checks::Checked, Value> {
    let form = file.to_str().and_then(spike_v1_fixture::form_of).ok_or_else(|| error("UNSUPPORTED_FORM", "지원하는 작성 형식이 아님"))?;
    let source = read_bounded(file, SOURCE_BYTES, "SOURCE_IO")?;
    spike_v11_dev_checks::load_checked(&source,form,&json!({"devChecks":dev_checks})).map_err(|ds|json!({"ok":false,"code":"INVALID_DEFINITION","diagnostics":ds.iter().map(|d|json!({"code":d.code,"message":d.msg,"line":d.span.line,"column":d.span.col})).collect::<Vec<_>>()}))
}
fn configuration(path: &Path) -> Result<Config, Value> {
    let source = read_bounded(path, CONFIG_BYTES, "CONFIG_IO")?;
    let mut config: Config = serde_json::from_str(&source).map_err(|_| error("BAD_CONFIG", "제품 설정 형식 오류"))?;
    aip_migrate::configure_schema(&config.schema)?;
    if !config.listen.ip().is_loopback() {
        return Err(error("BAD_LISTEN", "TLS reverse proxy 뒤의 loopback listener가 필요함"));
    }
    if config.database_url_env.is_empty() || !config.database_url_env.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(error("BAD_CONFIG", "DB 환경 변수 이름 오류"));
    }
    spike_v6_transport::server::ServerOptions { allowed_origins: config.allowed_origins.clone() }
        .validate_product()
        .map_err(|_| error("BAD_ORIGIN", "정규화된 HTTPS 또는 로컬 HTTP origin이 필요함"))?;
    let parent = path.parent().unwrap_or(Path::new("."));
    if let aip_auth::JwksSource::File { path } = &mut config.auth.jwks
        && path.is_relative()
    {
        *path = parent.join(&*path);
    }
    if let Some(path) = &mut config.database_ca_file
        && path.is_relative()
    {
        *path = parent.join(&*path);
    }
    if let Some(worker) = &mut config.workers
        && worker.directory.is_relative()
    {
        worker.directory = parent.join(&worker.directory);
    }
    Ok(config)
}
fn database_url(config: &Config) -> Result<String, Value> {
    if let Some(path) = &config.database_ca_file {
        let pem = read_bounded(path, CONFIG_BYTES, "DB_CA_IO")?;
        spike_v2_read::install_ca_bundle(pem.as_bytes()).map_err(|_| error("DB_CA_CONFIG", "DB CA bundle 형식 또는 프로세스 설정 충돌"))?;
    }
    std::env::var(&config.database_url_env)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| error("DB_CONFIG", "설정에 지정한 DB 환경 변수가 필요함"))
}
fn report(value: &Value) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{value}")?;
    out.flush()
}
fn output_error(value: &Value) {
    let _ = writeln!(std::io::stderr().lock(), "{value}");
}

fn write_binding(file: &Path, out: &Path, content: &str) -> Result<(), Value> {
    let source = file.canonicalize().map_err(|_| error("SOURCE_IO", "정의 경로 확인 실패"))?;
    if let Ok(target) = out.canonicalize() {
        let mut conflict = source == target;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(a), Ok(b)) = (fs::metadata(file), fs::metadata(out)) {
                conflict |= a.dev() == b.dev() && a.ino() == b.ino();
            }
        }
        if conflict {
            return Err(error("OUTPUT_CONFLICT", "정의 원본에 생성 계약을 쓸 수 없음"));
        }
    }
    let directory = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_| error("OUTPUT_IO", "시계 오류"))?.as_nanos();
    let tmp = directory.join(format!(".aip-gen-{}-{nonce}.tmp", std::process::id()));
    let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|_| error("OUTPUT_IO", "출력 임시 파일 생성 실패"))?;
    let result = f.write_all(content.as_bytes()).and_then(|_| f.sync_all()).and_then(|_| fs::rename(&tmp, out));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|_| error("OUTPUT_IO", "생성 계약 교체 실패"))
}

pub async fn run(command: Command) -> Result<Value, Value> {
    match command {
        Command::Check { file, dev_checks } => {
            let checked = load(&file, dev_checks)?;
            Ok(
                json!({"ok":true,"executionDigest":spike_v1_fixture::digest(&checked.output.execution),"advisories":checked.advisories.iter().map(|a|json!({"code":a.code,"anchor":a.anchor,"message":a.msg})).collect::<Vec<_>>()}),
            )
        }
        Command::Gen { file, wire, out, sdk_import } => {
            let facts = load(&file, false)?.output.execution;
            if sdk_import.trim().is_empty() {
                return Err(error("BAD_SDK_IMPORT", "SDK import 경로가 필요함"));
            }
            let content = spike_v5_sdk::contract_module_with_all_extensions(&facts, &sdk_import, wire.id_wire());
            write_binding(&file, &out, &content)?;
            Ok(
                json!({"ok":true,"fingerprint":spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts,wire.id_wire()),"idWire":wire.id_wire().label()}),
            )
        }
        Command::Init { file, config } => {
            let c = configuration(&config)?;
            let facts = load(&file, false)?.output.execution;
            aip_migrate::init_with_wire(&database_url(&c)?, &c.schema, &facts, c.wire.deployment_label()).await
        }
        Command::Migrate { file, config, migration, apply } => {
            let c = configuration(&config)?;
            let facts = load(&file, false)?.output.execution;
            let migration = migration
                .map(|p| {
                    read_bounded(&p, SOURCE_BYTES, "MIGRATION_IO")
                        .and_then(|text| serde_json::from_str(&text).map_err(|_| error("BAD_MIGRATION", "이행 선언 JSON 형식 오류")))
                })
                .transpose()?;
            match apply {
                Some(hash) => aip_migrate::apply(&database_url(&c)?, &c.schema, &facts, &migration, &hash).await,
                None => aip_migrate::plan(&database_url(&c)?, &c.schema, &facts, &migration).await,
            }
        }
        Command::Adopt { file, config, apply } => {
            let c = configuration(&config)?;
            let facts = load(&file, false)?.output.execution;
            match apply {
                Some(hash) => aip_migrate::adopt::apply_with_wire(&database_url(&c)?, &c.schema, &facts, &hash, c.wire.deployment_label()).await,
                None => aip_migrate::adopt::plan_with_wire(&database_url(&c)?, &c.schema, &facts, c.wire.deployment_label()).await,
            }
        }
        Command::Serve { file, config } => serve(&file, configuration(&config)?).await,
        Command::Principal { config, command } => principal(configuration(&config)?, command).await,
    }
}

async fn principal(config: Config, command: Principal) -> Result<Value, Value> {
    let db = tokio::time::timeout(std::time::Duration::from_secs(5), spike_v2_read::connect_owned_with_url(&database_url(&config)?))
        .await
        .map_err(|_| error("DB_CONNECT", "DB 연결 기한 초과"))?
        .map_err(|_| error("DB_CONNECT", "DB 연결 실패"))?;
    let issuer = &config.auth.issuer;
    let binding = matches!(&command, Principal::Bind { .. });
    let operation = async {
        let count=match command {
            Principal::Bind{subject,actor,not_before}=>{
                if subject.is_empty()||subject.len()>256||actor<=0||not_before<0{return Err(error("BAD_PRINCIPAL","subject·actor·not-before 값 오류"));}
                db.execute(&format!("INSERT INTO {}.aip_principals(issuer,subject,actor_id,enabled,min_iat) VALUES($1,$2,$3,true,$4) ON CONFLICT(issuer,subject) DO UPDATE SET enabled=true,min_iat=GREATEST(aip_principals.min_iat,EXCLUDED.min_iat) WHERE aip_principals.actor_id=EXCLUDED.actor_id",config.schema),&[issuer,&subject,&actor,&not_before]).await
            },
            Principal::Revoke{subject}=>{
                if subject.is_empty()||subject.len()>256{return Err(error("BAD_PRINCIPAL","subject 값 오류"));}
                db.execute(&format!("UPDATE {}.aip_principals SET enabled=false,min_iat=GREATEST(min_iat,floor(extract(epoch FROM clock_timestamp()))::bigint+1) WHERE issuer=$1 AND subject=$2",config.schema),&[issuer,&subject]).await
            },
        }.map_err(|_|error("PRINCIPAL_FAILED","actor 존재·매핑 테이블·DB 권한을 확인해야 함"))?;
        if count == 0 && binding {
            return Err(error("PRINCIPAL_CONFLICT", "이미 연결된 신원의 actor를 바꿀 수 없음"));
        }
        if count == 0 {
            return Err(error("PRINCIPAL_NOT_FOUND", "연결된 신원이 없음"));
        }
        Ok(json!({"ok":true,"affected":count}))
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), operation)
        .await
        .map_err(|_| error("PRINCIPAL_UNSETTLED", "사용자 연결 변경 결과 불명; 상태 확인 필요"))?
}

async fn serve(file: &Path, config: Config) -> Result<Value, Value> {
    let facts = load(file, false)?.output.execution;
    let db_url = database_url(&config)?;
    aip_migrate::preflight(&db_url, &config.schema, &facts).await?;
    aip_migrate::bind_wire(&db_url, &config.schema, config.wire.deployment_label()).await?;
    let deployment = spike_v6_transport::server::DeploymentFence::new(config.schema.clone(), spike_v1_fixture::digest(&facts))
        .map_err(|_| error("SCHEMA_CONTEXT", "배포 fence 설정 실패"))?;
    sqlgen::try_set_schema(&config.schema).map_err(|_| error("SCHEMA_CONTEXT", "프로세스당 한 schema만 실행할 수 있음"))?;
    let auth = Arc::new(
        aip_auth::AuthenticatorImpl::new(config.auth, db_url.clone(), config.schema)
            .await
            .map_err(|_| error("AUTH_CONFIG", "인증 설정 또는 공개키 집합 검증 실패"))?,
    );
    let enable_writes = config.workers.as_ref().is_some_and(|w| w.enable_writes);
    let extensions = config.workers.map(|w| spike_v6_transport::server::ReadExtensions {
        dir: w.directory,
        lang: match w.language {
            Language::Node => spike_v6_transport::server::WorkerLang::Node,
            Language::Python => spike_v6_transport::server::WorkerLang::Python,
        },
    });
    let options = spike_v6_transport::server::ServerOptions { allowed_origins: config.allowed_origins };
    #[cfg(unix)]
    let mut termination =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(|_| error("SIGNAL_CONFIG", "종료 signal 등록 실패"))?;
    let mut server = spike_v6_transport::server::listen_with_authenticator_and_deployment(
        facts.clone(),
        config.wire.id_wire(),
        config.listen,
        db_url,
        extensions,
        options,
        enable_writes,
        auth,
        Some(deployment),
    )
    .await
    .map_err(|_| error("SERVER_CONFIG", "listener·공식 확장 설정 실패"))?;
    let event = json!({"ok":true,"event":"listening","address":server.address.to_string(),"fingerprint":spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts,config.wire.id_wire()),"idWire":config.wire.id_wire().label(),"authentication":"jwt-access-token"});
    if report(&event).is_err() {
        server.shutdown().await.map_err(|_| error("SERVER_STOP", "출력 실패 후 종료 실패"))?;
        return Err(error("OUTPUT_IO", "기동 결과 출력 실패"));
    }
    #[cfg(unix)]
    tokio::select! {result=tokio::signal::ctrl_c()=>{result.map_err(|_|error("SIGNAL_FAILED","종료 signal 실패"))?;server.shutdown().await.map_err(|_|error("SERVER_STOP","서버 종료 실패"))?;},_=termination.recv()=>{server.shutdown().await.map_err(|_|error("SERVER_STOP","서버 종료 실패"))?;},result=server.wait()=>{result.map_err(|_|error("SERVER_FAILED","서버 실행 실패"))?;}}
    #[cfg(not(unix))]
    tokio::select! {result=tokio::signal::ctrl_c()=>{result.map_err(|_|error("SIGNAL_FAILED","종료 signal 실패"))?;server.shutdown().await.map_err(|_|error("SERVER_STOP","서버 종료 실패"))?;},result=server.wait()=>{result.map_err(|_|error("SERVER_FAILED","서버 실행 실패"))?;}}
    Ok(json!({"ok":true,"event":"stopped"}))
}

pub fn execute(command: Command) -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(_) => {
            output_error(&error("RUNTIME_START", "Rust runtime 기동 실패"));
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(command)) {
        Ok(value) => match report(&value) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
            Err(_) => {
                output_error(&error("OUTPUT_IO", "결과 출력 실패"));
                ExitCode::FAILURE
            }
        },
        Err(value) => {
            output_error(&value);
            ExitCode::FAILURE
        }
    }
}
