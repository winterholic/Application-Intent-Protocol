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
pub use spike_v4_worker::Lang as WorkerLang;

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
    pub db_url: String,
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
            db_url,
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
    runtime: Runtime,
    fingerprint: Option<Arc<str>>,
) -> io::Result<Server> {
    if !address.ip().is_loopback() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "prototype server requires loopback"));
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
        let connections = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
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
            db_url,
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
            db_url,
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
    options.validate_product()?;
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
            db_url,
            fault_injection: false,
            extensions,
            write_extensions: enable_write,
            extension_slots: tokio::sync::Semaphore::new(4),
            options,
            authenticator: Some(authenticator),
            deployment,
        },
        fingerprint,
    )
    .await
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
        if !cfg!(target_os = "macos") || wire == IdWire::Legacy {
            return Err(invalid());
        }
        if !std::path::Path::new("/usr/bin/sandbox-exec").is_file() {
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
            let read = ext["kind"] == "read" && ext["effect"] == "none";
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
