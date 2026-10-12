use crate::auth::Keyring;
use serde_json::Value;
use spike_v2_read::id_wire::IdWire;
use std::{io, net::SocketAddr, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::oneshot,
    task::{JoinHandle, JoinSet},
};

pub use crate::cors::ServerOptions;
pub use spike_v4_worker::{Isolation as WorkerIsolation, Lang as WorkerLang, WorkerLaunchConfig, WorkerResourceLimits};

pub const MAX_CONNECTIONS: usize = 64;
pub const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
pub const DB_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
pub const DB_PREFLIGHT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Clone)]
pub struct ReadExtensions {
    pub lang: WorkerLang,
    pub dir: std::path::PathBuf,
}

#[derive(Clone)]
pub struct DeploymentFence {
    pub(crate) schema: String,
    pub(crate) expected_digest: String,
}

impl DeploymentFence {
    pub fn new(schema: String, expected_digest: String) -> io::Result<Self> {
        let bytes = schema.as_bytes();
        if bytes.len() > 63
            || !bytes.starts_with(b"aip_")
            || !bytes.get(4).is_some_and(u8::is_ascii_lowercase)
            || !bytes[5..].iter().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
            || expected_digest.len() != 64
            || !expected_digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid deployment fence"));
        }
        Ok(Self { schema, expected_digest })
    }
}

pub(crate) struct Runtime {
    pub infrastructure: crate::infrastructure::Infrastructure,
    pub db_pool: Option<Arc<crate::pool::ConnectionPool>>,
    pub admission: Option<Arc<crate::admission::Admission>>,
    pub realtime_slots: tokio::sync::Semaphore,
    pub object_slots: tokio::sync::Semaphore,
    pub db_url: String,
    pub broker: Option<Arc<aip_execution::integrations::Broker>>,
    pub enable_file_uploads: bool,
    pub fault_injection: bool,
    pub extensions: Option<ReadExtensions>,
    pub write_extensions: bool,
    pub extension_slots: tokio::sync::Semaphore,
    pub options: ServerOptions,
    pub authenticator: Option<Arc<dyn crate::auth::Authenticator>>,
    pub deployment: Option<DeploymentFence>,
}

pub struct Server {
    pub address: SocketAddr,
    pub keys: Keyring,
    pub(crate) task: Option<JoinHandle<io::Result<()>>>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Server {
    pub async fn wait(&mut self) -> io::Result<()> {
        let result = match self.task.as_mut() {
            Some(task) => task.await.map_err(io::Error::other)?,
            None => return Ok(()),
        };
        self.task.take();
        result
    }

    pub async fn shutdown(mut self) -> io::Result<()> {
        if let Some(task) = self.task.take() {
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(());
            }
            match task.await {
                Ok(result) => result,
                Err(e) if e.is_cancelled() => Ok(()),
                Err(e) => Err(io::Error::other(e)),
            }
        } else {
            Ok(())
        }
    }

    pub(crate) fn detach(mut self) {
        self.task.take();
        self.shutdown.take();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

pub async fn listen_with_apply_contract(facts: Value, wire: IdWire, address: SocketAddr, db_url: String) -> io::Result<Server> {
    let fingerprint = facts["resources"].is_object().then(|| spike_v5_sdk::contract_fingerprint_with_apply(&facts, wire).into());
    listen_with_fingerprint(
        facts,
        wire,
        address,
        Runtime {
            infrastructure: crate::infrastructure::Infrastructure::default(),
            db_pool: None,
            admission: None,
            realtime_slots: tokio::sync::Semaphore::new(32),
            object_slots: tokio::sync::Semaphore::new(4),
            db_url,
            broker: None,
            enable_file_uploads: false,
            fault_injection: false,
            extensions: None,
            write_extensions: false,
            extension_slots: tokio::sync::Semaphore::new(4),
            options: ServerOptions::default(),
            authenticator: None,
            deployment: None,
        },
        fingerprint,
    )
    .await
}

pub(crate) async fn listen_with_fingerprint(
    facts: Value,
    wire: IdWire,
    address: SocketAddr,
    mut runtime: Runtime,
    fingerprint: Option<Arc<str>>,
) -> io::Result<Server> {
    if !address.ip().is_loopback() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "prototype server requires loopback"));
    }
    if let Some(extensions) = &runtime.extensions {
        runtime.infrastructure.worker_launch.probe(extensions.lang, &extensions.dir).await?;
    }
    if runtime.db_pool.is_none() {
        let pool = crate::pool::ConnectionPool::new(runtime.db_url.clone(), runtime.infrastructure.db_pool)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid database pool options"))?;
        runtime.db_pool = Some(Arc::new(pool));
    }
    if let Some(policy) = runtime.infrastructure.rate_limit {
        if runtime.authenticator.is_none() || runtime.deployment.is_none() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "rate limiting requires durable authentication and deployment fencing"));
        }
        let schema = &runtime.deployment.as_ref().expect("deployment checked").schema;
        let storage_schema = aip_execution::infrastructure_schema(schema).map_err(|_| io::Error::other("invalid deployment schema"))?;
        let limiter = aip_execution::rate_limit::RateLimiter::new(&storage_schema)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid rate limiter schema"))?;
        let admission = crate::admission::Admission::new(limiter.clone(), policy)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid rate limiter policy"))?;
        let pool = runtime.db_pool.as_ref().expect("pool initialized");
        let install = tokio::time::timeout(DB_PREFLIGHT_TIMEOUT, async {
            let db = pool.acquire().await.map_err(|_| io::Error::other("rate limiter database unavailable"))?;
            db.batch_execute(&format!("CREATE SCHEMA IF NOT EXISTS {storage_schema}"))
                .await
                .map_err(|_| io::Error::other("rate limiter schema initialization failed"))?;
            limiter.install(&*db).await.map_err(|_| io::Error::other("rate limiter installation failed"))?;
            db.release().await.map_err(|_| io::Error::other("rate limiter connection reset failed"))?;
            Ok::<_, io::Error>(())
        })
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "rate limiter installation timed out"))?;
        install?;
        runtime.admission = Some(Arc::new(admission));
    }
    let listener = TcpListener::bind(address).await?;
    let address = listener.local_addr()?;
    let facts = Arc::new(facts);
    let runtime = Arc::new(runtime);
    let keys = Keyring::generate();
    let request_keys = keys.clone();
    let (shutdown, mut stop) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut requests = JoinSet::new();
        let mut background = JoinSet::new();
        if crate::durable::enabled(&facts) && runtime.authenticator.is_some() && runtime.deployment.is_some() {
            background.spawn(crate::durable::supervisor(runtime.clone(), facts.clone(), wire));
        }
        let connections = Arc::new(tokio::sync::Semaphore::new(runtime.infrastructure.resource_limits.connections));
        let mut stop_open = true;
        let result = loop {
            tokio::select! {
                signal=&mut stop, if stop_open => {
                    if signal.is_ok() { break Ok(()); }
                    // Legacy test wrappers detach; losing their sender doesn't stop them.
                    stop_open=false;
                }
                connection=listener.accept()=> match connection {
                    Ok((sock,_))=>{
                        if let Ok(permit) = connections.clone().try_acquire_owned() {
                            let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
                            let (facts, keys, fingerprint, runtime) = (facts.clone(), request_keys.clone(), fingerprint.clone(), runtime.clone());
                            requests.spawn(async move {
                                let _permit = permit;
                                crate::serve_conn(sock, facts, keys, fingerprint, wire, runtime, deadline).await;
                            });
                        }
                    }
                    Err(e)=>break Err(e),
                },
                Some(result)=requests.join_next(), if !requests.is_empty()=> {
                    // 요청 하나의 panic이 다른 연결과 listener 전체를 내리지 않게 한다. 취소는 종료 중에만 생긴다.
                    if let Err(e)=result {
                        if !e.is_panic() {break Err(io::Error::other(e));}
                        eprintln!("{{\"event\":\"request_panicked\"}}");
                    }
                }
            }
        };
        drop(listener);
        requests.shutdown().await;
        background.shutdown().await;
        if let Some(pool) = &runtime.db_pool {
            pool.close();
        }
        result
    });
    Ok(Server { address, keys, task: Some(task), shutdown: Some(shutdown) })
}

pub async fn listen_with_extensions(
    facts: Value,
    wire: IdWire,
    address: SocketAddr,
    db_url: String,
    extensions: Option<ReadExtensions>,
) -> io::Result<Server> {
    listen_with_options(facts, wire, address, db_url, extensions, ServerOptions::default()).await
}

pub async fn listen_with_options(
    facts: Value,
    wire: IdWire,
    address: SocketAddr,
    db_url: String,
    extensions: Option<ReadExtensions>,
    options: ServerOptions,
) -> io::Result<Server> {
    options.validate()?;
    let extensions = extensions.map(|config| config.validate(&facts, wire)).transpose()?;
    let fingerprint = facts["resources"].is_object().then(|| spike_v5_sdk::contract_fingerprint_with_extensions(&facts, wire).into());
    listen_with_fingerprint(
        facts,
        wire,
        address,
        Runtime {
            infrastructure: crate::infrastructure::Infrastructure::default(),
            db_pool: None,
            admission: None,
            realtime_slots: tokio::sync::Semaphore::new(32),
            object_slots: tokio::sync::Semaphore::new(4),
            db_url,
            broker: None,
            enable_file_uploads: false,
            fault_injection: false,
            extensions,
            write_extensions: false,
            extension_slots: tokio::sync::Semaphore::new(4),
            options,
            authenticator: None,
            deployment: None,
        },
        fingerprint,
    )
    .await
}

pub async fn listen_with_prototype_options(
    facts: Value,
    wire: IdWire,
    address: SocketAddr,
    db_url: String,
    extensions: Option<ReadExtensions>,
    options: ServerOptions,
    enable_write: bool,
) -> io::Result<Server> {
    options.validate()?;
    if enable_write && extensions.is_none() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "WRITE extensions require configured workers"));
    }
    let extensions =
        extensions.map(|config| if enable_write { config.validate_with_writes(&facts, wire) } else { config.validate(&facts, wire) }).transpose()?;
    let fingerprint = facts["resources"].is_object().then(|| spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, wire).into());
    listen_with_fingerprint(
        facts,
        wire,
        address,
        Runtime {
            infrastructure: crate::infrastructure::Infrastructure::default(),
            db_pool: None,
            admission: None,
            realtime_slots: tokio::sync::Semaphore::new(32),
            object_slots: tokio::sync::Semaphore::new(4),
            db_url,
            broker: None,
            enable_file_uploads: false,
            fault_injection: false,
            extensions,
            write_extensions: enable_write,
            extension_slots: tokio::sync::Semaphore::new(4),
            options,
            authenticator: None,
            deployment: None,
        },
        fingerprint,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn listen_with_authenticator(
    facts: Value,
    wire: IdWire,
    address: SocketAddr,
    db_url: String,
    extensions: Option<ReadExtensions>,
    options: ServerOptions,
    enable_write: bool,
    authenticator: Arc<dyn crate::auth::Authenticator>,
) -> io::Result<Server> {
    listen_with_authenticator_and_deployment(facts, wire, address, db_url, extensions, options, enable_write, authenticator, None).await
}

#[allow(clippy::too_many_arguments)]
pub async fn listen_with_authenticator_and_deployment(
    facts: Value,
    wire: IdWire,
    address: SocketAddr,
    db_url: String,
    extensions: Option<ReadExtensions>,
    options: ServerOptions,
    enable_write: bool,
    authenticator: Arc<dyn crate::auth::Authenticator>,
    deployment: Option<DeploymentFence>,
) -> io::Result<Server> {
    listen_with_managed(facts, wire, address, db_url, extensions, options, enable_write, authenticator, deployment, None, false).await
}

#[allow(clippy::too_many_arguments)]
pub async fn listen_with_managed(
    facts: Value,
    wire: IdWire,
    address: SocketAddr,
    db_url: String,
    extensions: Option<ReadExtensions>,
    options: ServerOptions,
    enable_write: bool,
    authenticator: Arc<dyn crate::auth::Authenticator>,
    deployment: Option<DeploymentFence>,
    broker: Option<Arc<aip_execution::integrations::Broker>>,
    enable_file_uploads: bool,
) -> io::Result<Server> {
    listen_with_infrastructure(
        facts,
        wire,
        address,
        db_url,
        extensions,
        options,
        enable_write,
        authenticator,
        deployment,
        broker,
        enable_file_uploads,
        crate::infrastructure::Infrastructure::default(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn listen_with_infrastructure(
    facts: Value,
    wire: IdWire,
    address: SocketAddr,
    db_url: String,
    extensions: Option<ReadExtensions>,
    options: ServerOptions,
    enable_write: bool,
    authenticator: Arc<dyn crate::auth::Authenticator>,
    deployment: Option<DeploymentFence>,
    broker: Option<Arc<aip_execution::integrations::Broker>>,
    enable_file_uploads: bool,
    infrastructure: crate::infrastructure::Infrastructure,
) -> io::Result<Server> {
    infrastructure.validate()?;
    let resource_limits = infrastructure.resource_limits;
    options.validate_product()?;
    validate_job_broker(&facts, extensions.is_some(), broker.as_deref())?;
    if enable_write && extensions.is_none() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "WRITE extensions require configured workers"));
    }
    let extensions =
        extensions.map(|config| if enable_write { config.validate_with_writes(&facts, wire) } else { config.validate(&facts, wire) }).transpose()?;
    let fingerprint = facts["resources"].is_object().then(|| spike_v5_sdk::contract_fingerprint_with_all_extensions(&facts, wire).into());
    listen_with_fingerprint(
        facts,
        wire,
        address,
        Runtime {
            realtime_slots: tokio::sync::Semaphore::new(resource_limits.realtime_capacity(infrastructure.realtime.max_connections)),
            object_slots: tokio::sync::Semaphore::new(resource_limits.uploads),
            infrastructure,
            db_pool: None,
            admission: None,
            db_url,
            broker,
            enable_file_uploads,
            fault_injection: false,
            extensions,
            write_extensions: enable_write,
            extension_slots: tokio::sync::Semaphore::new(resource_limits.workers),
            options,
            authenticator: Some(authenticator),
            deployment,
        },
        fingerprint,
    )
    .await
}

fn validate_job_broker(facts: &Value, workers_configured: bool, broker: Option<&aip_execution::integrations::Broker>) -> io::Result<()> {
    if !workers_configured {
        return Ok(());
    }
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "durable job references an unconfigured HTTP integration");
    for operation in facts["operations"].as_object().into_iter().flatten().map(|(_, operation)| operation) {
        if operation["kind"] != "job" {
            continue;
        }
        for capability in operation["access"].as_object().into_iter().flatten().map(|(capability, _)| capability) {
            let Some(name) = capability.strip_prefix("http.") else { continue };
            if !broker.is_some_and(|configured| configured.contains(name)) {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

impl ReadExtensions {
    pub fn validate(&self, facts: &Value, wire: IdWire) -> io::Result<Self> {
        self.validate_kinds(facts, wire, false)
    }

    pub fn validate_with_writes(&self, facts: &Value, wire: IdWire) -> io::Result<Self> {
        self.validate_kinds(facts, wire, true)
    }

    fn validate_kinds(&self, facts: &Value, wire: IdWire, include_write: bool) -> io::Result<Self> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "invalid local READ extension configuration");
        if !cfg!(any(target_os = "macos", target_os = "linux")) || wire == IdWire::Legacy {
            return Err(invalid());
        }
        let dir = self.dir.canonicalize().map_err(|_| invalid())?;
        if !dir.is_dir() || dir.to_str().is_none() {
            return Err(invalid());
        }
        let resource_extensions = facts["resources"]
            .as_object()
            .into_iter()
            .flatten()
            .flat_map(|(_, resource)| resource["extensions"].as_object().into_iter().flatten().map(|(_, extension)| extension));
        let operations = facts["operations"].as_object().into_iter().flatten().map(|(_, operation)| operation);
        for ext in resource_extensions.chain(operations) {
            let read = (ext["kind"] == "read" && ext["effect"] == "none")
                || (ext["kind"] == "job" && (ext["effect"] == "none" || ext["effect"] == "managed"));
            let write = include_write && ext["kind"] == "write" && ext["effect"] == "db";
            if !read && !write {
                continue;
            }
            let implementation = ext["implementation"].as_str().ok_or_else(invalid)?;
            let (module, function) = implementation.split_once('.').ok_or_else(invalid)?;
            let identifier = |part: &str| {
                let mut bytes = part.bytes();
                bytes.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_') && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
            };
            if !identifier(module) || !identifier(function) {
                return Err(invalid());
            }
            let suffix = match self.lang {
                WorkerLang::Node => "mjs",
                WorkerLang::Python => "py",
            };
            let file = dir.join(format!("{module}.{suffix}")).canonicalize().map_err(|_| invalid())?;
            if !file.starts_with(&dir) || !file.is_file() {
                return Err(invalid());
            }
        }
        Ok(Self { lang: self.lang, dir })
    }
}
#[cfg(test)]
mod extension_config_tests {
    use super::*;
    use serde_json::json;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use std::sync::atomic::{AtomicU64, Ordering};

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    static NEXT_PROBE_DIR: AtomicU64 = AtomicU64::new(0);

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    struct ProbeAuth;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    impl crate::auth::Authenticator for ProbeAuth {
        fn verify_session<'a>(
            &'a self,
            _token: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<crate::auth::Session, crate::auth::AuthError>> + Send + 'a>> {
            Box::pin(async { Err(crate::auth::AuthError::Unavailable) })
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    struct ProbeDir(std::path::PathBuf);

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    impl ProbeDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "aip-worker-probe-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
                NEXT_PROBE_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("probe.mjs"), "export function run() { return {}; }\n").unwrap();
            Self(path)
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    impl Drop for ProbeDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(target_os = "macos")]
    async fn start_with_probe_runtime(runtime: std::path::PathBuf, dir: &ProbeDir) -> io::Result<Server> {
        let mut infrastructure = crate::infrastructure::Infrastructure::default();
        infrastructure.worker_launch.runtime_executable = Some(runtime);
        let mut facts =
            spike_v1_fixture::load_str(include_str!("../../spike-v1-fixture/fixture/recruitment.aip"), spike_v1_fixture::Form::A).unwrap().execution;
        facts["resources"]["Recruitment"]["extensions"]["stats"]["implementation"] = json!("probe.run");
        listen_with_infrastructure(
            facts,
            IdWire::SafeNumber,
            "127.0.0.1:0".parse().unwrap(),
            "host=127.0.0.1 port=1 dbname=postgres connect_timeout=1".into(),
            Some(ReadExtensions { lang: WorkerLang::Node, dir: dir.0.clone() }),
            ServerOptions::default(),
            false,
            Arc::new(ProbeAuth),
            None,
            None,
            false,
            infrastructure,
        )
        .await
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn product_start_fails_when_worker_runtime_is_missing() {
        let dir = ProbeDir::new();
        let result = start_with_probe_runtime(dir.0.join("missing-node"), &dir).await;
        assert!(result.is_err(), "product startup must reject a missing worker runtime");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn product_start_fails_when_worker_runtime_exits_nonzero() {
        use std::os::unix::fs::PermissionsExt;
        let dir = ProbeDir::new();
        let runtime = dir.0.join("broken-node");
        std::fs::write(&runtime, "#!/bin/sh\necho probe-failed >&2\nexit 42\n").unwrap();
        let mut permissions = std::fs::metadata(&runtime).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&runtime, permissions).unwrap();
        let result = start_with_probe_runtime(runtime, &dir).await;
        assert!(result.is_err(), "product startup must reject a worker runtime that exits nonzero");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn worker_isolation_selection_respects_custom_mode_without_bwrap_in_path() {
        const CHILD_MODE: &str = "AIP_ISOLATION_SELECTION_TEST_CHILD";
        if let Ok(mode) = std::env::var(CHILD_MODE) {
            use std::os::unix::fs::PermissionsExt;
            let dir = ProbeDir::new();
            let runtime = dir.0.join("node-runtime");
            std::fs::write(&runtime, "#!/bin/sh\necho node-fixture-1\n").unwrap();
            let mut permissions = std::fs::metadata(&runtime).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&runtime, permissions).unwrap();

            let mut infrastructure = crate::infrastructure::Infrastructure::default();
            infrastructure.worker_launch.isolation = if mode == "none" { spike_v4_worker::Isolation::None } else { spike_v4_worker::Isolation::Auto };
            infrastructure.worker_launch.runtime_executable = Some(runtime);
            let result = listen_with_infrastructure(
                json!({}),
                IdWire::SafeNumber,
                "127.0.0.1:0".parse().unwrap(),
                "host=127.0.0.1 port=1 dbname=postgres connect_timeout=1".into(),
                Some(ReadExtensions { lang: WorkerLang::Node, dir: dir.0.clone() }),
                ServerOptions::default(),
                false,
                Arc::new(ProbeAuth),
                None,
                None,
                false,
                infrastructure,
            )
            .await;
            if mode == "none" {
                result.expect("explicit trusted None mode must start without bubblewrap").shutdown().await.unwrap();
            } else {
                assert!(result.is_err(), "Auto mode must fail closed when bubblewrap is missing");
            }
            return;
        }

        let dir = ProbeDir::new();
        let empty_path = dir.0.join("empty-path");
        std::fs::create_dir(&empty_path).unwrap();
        let test_executable = std::env::current_exe().unwrap();
        for mode in ["none", "auto"] {
            let mut command = tokio::process::Command::new(&test_executable);
            command
                .args(["--exact", "server::extension_config_tests::worker_isolation_selection_respects_custom_mode_without_bwrap_in_path"])
                .env(CHILD_MODE, mode)
                .env("PATH", &empty_path)
                .kill_on_drop(true);
            let status = tokio::time::timeout(std::time::Duration::from_secs(5), command.status())
                .await
                .expect("isolation selection subprocess exceeded its deadline")
                .unwrap();
            assert!(status.success(), "{mode} isolation subprocess failed with {status}");
        }
    }

    #[test]
    fn configured_job_workers_require_each_declared_http_broker() {
        let facts = json!({"operations":{"root":{"kind":"job","access":{"http.billing":true}}}});
        assert!(validate_job_broker(&facts, false, None).is_ok());
        assert!(validate_job_broker(&facts, true, None).is_err());

        let broker = aip_execution::integrations::Broker::new(vec![aip_execution::integrations::EndpointConfig {
            name: "other".into(),
            url: "http://127.0.0.1:9000/call".into(),
            method: "POST".into(),
            timeout: std::time::Duration::from_secs(1),
            max_request_bytes: 1024,
            max_response_bytes: 1024,
            secret_header: None,
            resolved: vec!["127.0.0.1:9000".parse().unwrap()],
            allow_loopback: true,
            allow_private_networks: false,
        }])
        .unwrap();
        assert!(validate_job_broker(&facts, true, Some(&broker)).is_err());

        let broker = aip_execution::integrations::Broker::new(vec![aip_execution::integrations::EndpointConfig {
            name: "billing".into(),
            url: "http://127.0.0.1:9000/call".into(),
            method: "POST".into(),
            timeout: std::time::Duration::from_secs(1),
            max_request_bytes: 1024,
            max_response_bytes: 1024,
            secret_header: None,
            resolved: vec!["127.0.0.1:9000".parse().unwrap()],
            allow_loopback: true,
            allow_private_networks: false,
        }])
        .unwrap();
        assert!(validate_job_broker(&facts, true, Some(&broker)).is_ok());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn rejects_an_extension_module_symlink_outside_its_directory() {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!("aip-module-boundary-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let outside = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../spike-v4-worker/extensions/values.mjs")).canonicalize().unwrap();
        symlink(&outside, dir.join("values.mjs")).unwrap();
        let config = ReadExtensions { lang: WorkerLang::Node, dir: dir.clone() };
        let facts = json!({"resources":{"Echo":{"extensions":{"echo":{"kind":"read","effect":"none","implementation":"values.echo"}}}}});
        let result = config.validate(&facts, IdWire::SafeNumber);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(result.is_err(), "existing module outside configured directory cannot be selected by symlink");
    }

    #[test]
    fn rejects_implementation_paths_before_listening() {
        let config = ReadExtensions {
            lang: WorkerLang::Node,
            dir: std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../spike-v4-worker/extensions")),
        };
        for implementation in ["/tmp/escape.echo", "../escape.echo", "values.echo.extra", "values.", "values/other.echo"] {
            let facts = json!({"resources":{"Echo":{"extensions":{"echo":{"kind":"read","effect":"none","implementation":implementation}}}}});
            assert!(config.validate(&facts, IdWire::SafeNumber).is_err(), "accepted {implementation}");
        }
    }
}
