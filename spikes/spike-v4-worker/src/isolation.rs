use std::ffi::{OsStr, OsString};

/// Automatic isolation uses the platform's network-denying worker sandbox.
/// MacNetDeny blocks network access only; LinuxBubblewrap also limits visible files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Isolation {
    /// Select MacNetDeny on macOS and LinuxBubblewrap on Linux.
    Auto,
    /// Run without an OS sandbox. Only use for explicitly trusted server-side configurations.
    None,
    /// macOS network denial; host filesystem remains visible to the worker.
    MacNetDeny,
    LinuxBubblewrap,
}

impl Isolation {
    /// Check that this isolation mode is supported and its launcher is executable.
    /// Call this during service startup to fail before accepting work.
    pub fn validate_available(self) -> std::io::Result<()> {
        match self {
            Self::None => Ok(()),
            Self::Auto => {
                #[cfg(target_os = "macos")]
                {
                    validate_executable(std::path::Path::new("/usr/bin/sandbox-exec"))
                }
                #[cfg(target_os = "linux")]
                {
                    find_executable("bwrap").map(|_| ())
                }
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                {
                    Err(unsupported("automatic worker isolation is unavailable on this platform"))
                }
            }
            Self::MacNetDeny => {
                #[cfg(target_os = "macos")]
                {
                    validate_executable(std::path::Path::new("/usr/bin/sandbox-exec"))
                }
                #[cfg(not(target_os = "macos"))]
                {
                    Err(unsupported("MacNetDeny requires macOS"))
                }
            }
            Self::LinuxBubblewrap => {
                #[cfg(target_os = "linux")]
                {
                    find_executable("bwrap").map(|_| ())
                }
                #[cfg(not(target_os = "linux"))]
                {
                    Err(unsupported("LinuxBubblewrap requires Linux"))
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn find_executable(name: &str) -> std::io::Result<std::path::PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if validate_executable(&candidate).is_ok() {
            return Ok(candidate);
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("required worker isolation executable `{name}` was not found in PATH")))
}

#[cfg(target_os = "linux")]
pub(crate) fn resolve_runtime(program: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    if !program.is_absolute() && program.components().count() == 1 {
        return find_executable(&program.to_string_lossy())?.canonicalize();
    }
    let parent = program.parent().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "worker runtime path has no parent"))?;
    let name =
        program.file_name().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "worker runtime path has no executable name"))?;
    Ok(parent.canonicalize()?.join(name))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn validate_executable(path: &std::path::Path) -> std::io::Result<()> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{} is not a regular file", path.display())));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{} is not executable", path.display())));
        }
    }
    Ok(())
}

/// Optional execution choices for a worker. Environment entries are explicit additions.
#[derive(Clone, Debug)]
pub struct WorkerLaunchConfig {
    pub isolation: Isolation,
    /// An explicitly trusted Node or Python executable, including a venv interpreter.
    pub runtime_executable: Option<std::path::PathBuf>,
    /// Additional variables passed after clearing the inherited environment.
    pub environment: std::collections::HashMap<String, String>,
    /// Server-trusted dependency directories mounted read-only inside Linux workers.
    /// Keep each entry scoped to dependencies; broad host directories are rejected.
    pub read_only_paths: Vec<std::path::PathBuf>,
}

impl Default for WorkerLaunchConfig {
    fn default() -> Self {
        Self { isolation: Isolation::Auto, runtime_executable: None, environment: std::collections::HashMap::new(), read_only_paths: Vec::new() }
    }
}

#[derive(Debug)]
pub(crate) struct CommandSpec {
    pub program: OsString,
    pub args: Vec<OsString>,
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn linux_bwrap_spec<I, S>(
    program: &OsStr,
    args: I,
    ext_dir: &std::path::Path,
    read_only_paths: &[std::path::PathBuf],
) -> std::io::Result<CommandSpec>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut bwrap_args: Vec<OsString> =
        ["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp", "--unshare-net", "--unshare-pid", "--die-with-parent", "--new-session"]
            .into_iter()
            .map(OsString::from)
            .collect();

    for system_path in ["/usr", "/lib", "/lib64", "/bin", "/sbin"] {
        let system_path = std::path::Path::new(system_path);
        if system_path.is_dir() {
            add_read_only_bind(&mut bwrap_args, system_path, system_path)?;
        }
    }
    let loader_cache = std::path::Path::new("/etc/ld.so.cache");
    if loader_cache.is_file() {
        add_read_only_bind(&mut bwrap_args, loader_cache, loader_cache)?;
    }
    let certificates = std::path::Path::new("/etc/ssl/certs");
    if certificates.is_dir() {
        add_read_only_bind(&mut bwrap_args, certificates, certificates)?;
    }

    let mut selected_paths = vec![ext_dir.canonicalize()?];
    let runtime_path = std::path::Path::new(program);
    if runtime_path.is_absolute() && !is_system_runtime_path(runtime_path) {
        if let Some(runtime_root) = runtime_path.parent().and_then(std::path::Path::parent) {
            selected_paths.push(runtime_root.to_path_buf());
        }
    }
    selected_paths.sort();
    selected_paths.dedup();
    let mut selected_paths = selected_paths.into_iter().map(|path| path.canonicalize()).collect::<std::io::Result<Vec<_>>>()?;
    selected_paths.sort_by_key(|path| path.components().count());
    selected_paths.dedup();
    selected_paths.retain(|path| !is_system_runtime_path(path));
    let mut compact_paths = Vec::new();
    for path in selected_paths {
        if !compact_paths.iter().any(|allowed: &std::path::PathBuf| path.starts_with(allowed)) {
            compact_paths.push(path);
        }
    }
    let mut extra_paths = read_only_paths
        .iter()
        .map(|path| {
            let canonical = path.canonicalize()?;
            if !canonical.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("Linux worker read-only path is not a directory: {}", canonical.display()),
                ));
            }
            Ok(canonical)
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    extra_paths.sort_by_key(|path| path.components().count());
    extra_paths.dedup();
    for path in extra_paths {
        if compact_paths.iter().any(|allowed| path.starts_with(allowed)) {
            continue;
        }
        if compact_paths.iter().any(|allowed| allowed.starts_with(&path)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Linux worker read-only path is broader than an existing mount: {}", path.display()),
            ));
        }
        compact_paths.push(path);
    }
    for path in &compact_paths {
        if !path.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Linux worker read-only path is not a directory: {}", path.display()),
            ));
        }
        if path == std::path::Path::new("/")
            || path == std::path::Path::new("/home")
            || path == std::path::Path::new("/root")
            || path == std::path::Path::new("/tmp")
            || path == std::path::Path::new("/var/tmp")
            || path == std::path::Path::new("/opt")
            || path == std::path::Path::new("/var")
            || path == std::path::Path::new("/run")
            || path == std::path::Path::new("/mnt")
            || path == std::path::Path::new("/media")
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Linux worker cannot mount a broad host directory: {}", path.display()),
            ));
        }
        if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from).and_then(|home| home.canonicalize().ok()) {
            if path.as_path() == home.as_path() || home.starts_with(path) {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Linux worker cannot mount the host home directory or its parent"));
            }
        }
        add_read_only_bind(&mut bwrap_args, path, path)?;
    }
    bwrap_args.push("--".into());
    bwrap_args.push(program.to_owned());
    bwrap_args.extend(args.into_iter().map(|arg| arg.as_ref().to_owned()));
    Ok(CommandSpec { program: OsString::from("bwrap"), args: bwrap_args })
}

#[cfg(any(target_os = "linux", test))]
fn is_system_runtime_path(path: &std::path::Path) -> bool {
    ["/usr", "/lib", "/lib64", "/bin", "/sbin"].iter().any(|root| path.starts_with(root))
}

#[cfg(any(target_os = "linux", test))]
fn add_read_only_bind(args: &mut Vec<OsString>, source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    if !source.is_absolute() || (!source.is_dir() && !source.is_file()) || !destination.is_absolute() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("invalid Linux worker mount: {}", source.display())));
    }
    let target = if source.is_dir() { destination } else { destination.parent().unwrap_or(destination) };
    let mut parents: Vec<_> = target.ancestors().filter(|path| *path != std::path::Path::new("/")).collect();
    parents.reverse();
    for parent in parents {
        args.extend(["--dir".into(), parent.as_os_str().to_owned()]);
    }
    args.extend(["--ro-bind".into(), source.as_os_str().to_owned(), destination.as_os_str().to_owned()]);
    Ok(())
}

pub(crate) fn command_spec(
    isolation: Isolation,
    program: &OsStr,
    args: &[OsString],
    _ext_dir: &std::path::Path,
    _read_only_paths: &[std::path::PathBuf],
) -> std::io::Result<CommandSpec> {
    match isolation {
        Isolation::None => Ok(CommandSpec { program: program.to_owned(), args: args.to_vec() }),
        Isolation::Auto => {
            #[cfg(target_os = "macos")]
            {
                mac_sandbox_spec(program, args)
            }
            #[cfg(target_os = "linux")]
            {
                linux_bwrap_spec(program, args.iter(), _ext_dir, _read_only_paths)
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                Err(unsupported("automatic worker isolation is unavailable on this platform"))
            }
        }
        Isolation::MacNetDeny => {
            #[cfg(target_os = "macos")]
            {
                mac_sandbox_spec(program, args)
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(unsupported("MacNetDeny requires macOS"))
            }
        }
        Isolation::LinuxBubblewrap => {
            #[cfg(target_os = "linux")]
            {
                linux_bwrap_spec(program, args.iter(), _ext_dir, _read_only_paths)
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err(unsupported("LinuxBubblewrap requires Linux"))
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn mac_sandbox_spec(program: &OsStr, args: &[OsString]) -> std::io::Result<CommandSpec> {
    let mut sandbox_args = vec!["-p".into(), "(version 1)(allow default)(deny network*)".into(), program.to_owned()];
    sandbox_args.extend_from_slice(args);
    Ok(CommandSpec { program: "/usr/bin/sandbox-exec".into(), args: sandbox_args })
}

fn unsupported(message: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Unsupported, message)
}

#[cfg(test)]
mod tests {
    use super::{command_spec, linux_bwrap_spec, Isolation, WorkerLaunchConfig};
    use crate::{Lang, Worker};
    use serde_json::{json, Value};
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!("aip-worker-{label}-{}-{}", std::process::id(), NEXT_DIR.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn invoke_worker(
        lang: Lang,
        ext_dir: &std::path::Path,
        environment: std::collections::HashMap<String, String>,
        read_only_paths: Vec<PathBuf>,
    ) -> Value {
        let config = WorkerLaunchConfig { environment, read_only_paths, ..WorkerLaunchConfig::default() };
        let mut worker = Worker::try_start_configured(lang, &ext_dir.to_string_lossy(), config).await.expect("isolated worker starts");
        worker.send(&json!({"type":"invoke","invoke":1,"impl":"probe.run","input":{},"token":"fixture"})).await.unwrap();
        let line = match worker.read_line().await.unwrap() {
            Some(line) => line,
            None => {
                let stderr_task = tokio::time::timeout(std::time::Duration::from_millis(100), &mut worker.stderr_task).await;
                let child_status = worker.child.try_wait();
                let stderr_tail = worker.stderr_tail.lock().unwrap().clone();
                panic!("worker stdout reached EOF; stderr_task={stderr_task:?}, child_status={child_status:?}, stderr_tail={stderr_tail:?}");
            }
        };
        worker.stop().await;
        serde_json::from_str(&line).unwrap()
    }

    #[test]
    fn linux_bwrap_mounts_host_read_only_and_disables_network() {
        let ext = TempDir::new("bwrap-ext");
        let deps = TempDir::new("bwrap-deps");
        let spec = linux_bwrap_spec("/usr/bin/node".as_ref(), ["--version"], ext.path(), &[deps.path().to_path_buf()]).unwrap();
        let args: Vec<String> = spec.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
        let ext_path = ext.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let deps_path = deps.path().canonicalize().unwrap().to_string_lossy().into_owned();
        let host_home = std::env::var_os("HOME").map(PathBuf::from).and_then(|home| home.canonicalize().ok());
        assert_eq!(spec.program, OsString::from("bwrap"));
        assert!(!args.windows(3).any(|part| part == ["--ro-bind", "/", "/"]));
        assert!(args.windows(3).any(|part| part == ["--ro-bind", "/usr", "/usr"]));
        assert!(args.windows(3).any(|part| part == ["--ro-bind", ext_path.as_str(), ext_path.as_str()]));
        assert!(args.windows(3).any(|part| part == ["--ro-bind", deps_path.as_str(), deps_path.as_str()]));
        assert!(!args.iter().any(|arg| arg == "/home" || arg == "/root"));
        if let Some(home) = host_home {
            assert!(!args.iter().any(|arg| arg == home.to_string_lossy().as_ref()));
            assert!(args.windows(3).filter(|part| part[0] == "--ro-bind").all(|part| {
                let source = PathBuf::from(&part[1]);
                !home.starts_with(&source)
            }));
        }
        assert!(args.contains(&"--unshare-net".to_owned()));
        assert!(args.contains(&"--die-with-parent".to_owned()));
        assert!(args.windows(2).any(|part| part == ["--tmpfs", "/tmp"]));
        assert!(args.windows(2).any(|part| part == ["--proc", "/proc"]));
        assert!(args.windows(2).any(|part| part == ["--dev", "/dev"]));
        assert!(args.windows(2).any(|part| part == ["--unshare-pid", "--die-with-parent"]));
        assert!(args.ends_with(&["--".to_owned(), "/usr/bin/node".to_owned(), "--version".to_owned()]));
    }

    #[test]
    fn linux_bwrap_rejects_broad_host_mounts_and_exposes_only_custom_runtime_root() {
        let ext = TempDir::new("bwrap-ext");
        let runtime_dir = TempDir::new("runtime-root");
        let runtime_root = runtime_dir.path().join("venv");
        std::fs::create_dir_all(runtime_root.join("bin")).unwrap();
        std::fs::write(runtime_root.join("bin/python"), "fixture").unwrap();
        let runtime = runtime_root.join("bin/python");
        let spec = linux_bwrap_spec(runtime.as_os_str(), ["--version"], ext.path(), &[]).unwrap();
        let args: Vec<String> = spec.args.iter().map(|arg| arg.to_string_lossy().into_owned()).collect();
        let root = runtime_root.canonicalize().unwrap().to_string_lossy().into_owned();
        assert!(args.windows(3).any(|part| part == ["--ro-bind", root.as_str(), root.as_str()]));

        let error = linux_bwrap_spec("/usr/bin/node".as_ref(), std::iter::empty::<&str>(), ext.path(), &[PathBuf::from("/")]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn linux_bwrap_rejects_non_directory_dependency_mounts() {
        let ext = TempDir::new("bwrap-ext");
        let dependency_file = ext.path().join("dependency.py");
        std::fs::write(&dependency_file, "answer = 42").unwrap();
        let error = linux_bwrap_spec("/usr/bin/node".as_ref(), std::iter::empty::<&str>(), ext.path(), &[dependency_file]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("not a directory"));
    }

    #[test]
    fn default_launch_config_selects_auto_and_keeps_legacy_path_only() {
        let config = WorkerLaunchConfig::default();
        assert_eq!(config.isolation, Isolation::Auto);
        assert!(config.runtime_executable.is_none());
        assert!(config.environment.is_empty());
        assert!(config.read_only_paths.is_empty());
    }

    #[test]
    fn isolation_availability_reports_missing_platform_runtime() {
        assert!(Isolation::None.validate_available().is_ok());
        let result = Isolation::Auto.validate_available();
        #[cfg(target_os = "linux")]
        assert_eq!(result.is_ok(), super::find_executable("bwrap").is_ok());
        #[cfg(target_os = "macos")]
        assert!(result.is_ok(), "macOS sandbox executable unavailable: {result:?}");
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Unsupported);
    }

    #[tokio::test]
    async fn node_extension_imports_local_package_from_node_modules() {
        if cfg!(target_os = "linux") && std::env::var("AIP_TEST_BWRAP").as_deref() != Ok("1") {
            eprintln!("skipped Linux bwrap integration test; set AIP_TEST_BWRAP=1 to enable");
            return;
        }
        let dir = TempDir::new("node-deps");
        std::fs::create_dir_all(dir.path().join("node_modules/aip-fixture")).unwrap();
        std::fs::write(dir.path().join("probe.mjs"), "import { answer } from 'aip-fixture'; export function run() { return { answer }; }\n").unwrap();
        std::fs::write(dir.path().join("node_modules/aip-fixture/package.json"), r#"{"name":"aip-fixture","type":"module","exports":"./index.mjs"}"#)
            .unwrap();
        std::fs::write(dir.path().join("node_modules/aip-fixture/index.mjs"), "export const answer = 42;\n").unwrap();

        let response = invoke_worker(Lang::Node, dir.path(), Default::default(), vec![]).await;
        assert_eq!(response, json!({"type":"done","invoke":1,"output":{"answer":42}}));
    }

    #[tokio::test]
    async fn python_extension_imports_explicit_pythonpath_dependency() {
        if cfg!(target_os = "linux") && std::env::var("AIP_TEST_BWRAP").as_deref() != Ok("1") {
            eprintln!("skipped Linux bwrap integration test; set AIP_TEST_BWRAP=1 to enable");
            return;
        }
        let dir = TempDir::new("python-deps");
        let deps = dir.path().join("deps");
        std::fs::create_dir_all(&deps).unwrap();
        std::fs::write(dir.path().join("probe.py"), "from aip_fixture import answer\ndef run(_input, _ctx): return {'answer': answer}\n").unwrap();
        std::fs::write(deps.join("aip_fixture.py"), "answer = 42\n").unwrap();

        let response = invoke_worker(Lang::Python, dir.path(), [("PYTHONPATH".into(), deps.to_string_lossy().into_owned())].into(), vec![deps]).await;
        assert_eq!(response, json!({"type":"done","invoke":1,"output":{"answer":42}}));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_bwrap_hides_host_files_blocks_writes_and_network_when_enabled() {
        if std::env::var("AIP_TEST_BWRAP").as_deref() != Ok("1") {
            return;
        }
        let dir = TempDir::new("linux-boundary");
        let host_secret = TempDir::new("host-secret");
        let host_secret_path = host_secret.path().join("credential.txt");
        std::fs::write(&host_secret_path, "host-secret-must-not-be-visible").unwrap();
        std::fs::write(
            dir.path().join("probe.py"),
            "import os, socket\ndef run(_input, _ctx):\n    try:\n        open(os.environ['AIP_HOST_SECRET_PATH']).read(); host_file = True\n    except OSError:\n        host_file = False\n    try:\n        open(os.path.join(os.path.dirname(__file__), 'escape'), 'w').write('bad'); writable = True\n    except OSError:\n        writable = False\n    try:\n        s = socket.socket(); s.settimeout(0.2); s.connect(('1.1.1.1', 53)); network = True\n    except OSError:\n        network = False\n    return {'host_file': host_file, 'writable': writable, 'network': network}\n",
        )
        .unwrap();

        let response = invoke_worker(
            Lang::Python,
            dir.path(),
            [("AIP_HOST_SECRET_PATH".into(), host_secret_path.to_string_lossy().into_owned())].into(),
            vec![],
        )
        .await;
        assert_eq!(response, json!({"type":"done","invoke":1,"output":{"host_file":false,"writable":false,"network":false}}));
    }

    #[test]
    fn custom_runtime_is_passed_as_an_executable_without_shell_parsing() {
        let runtime = std::path::Path::new("/opt/python env/bin/python");
        let arguments = [OsString::from("-c"), OsString::from("print('ok')")];
        let ext = TempDir::new("runtime-ext");
        let spec = command_spec(Isolation::None, runtime.as_os_str(), &arguments, ext.path(), &[]).unwrap();
        assert_eq!(spec.program, runtime.as_os_str());
        assert_eq!(spec.args, arguments);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn auto_uses_macos_network_sandbox() {
        let ext = TempDir::new("auto-ext");
        let spec = command_spec(Isolation::Auto, "python3".as_ref(), &["-c".into()], ext.path(), &[]).unwrap();
        assert_eq!(spec.program, OsString::from("/usr/bin/sandbox-exec"));
        assert!(spec.args.iter().any(|arg| arg == "(version 1)(allow default)(deny network*)"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn auto_uses_linux_bubblewrap() {
        let ext = TempDir::new("auto-ext");
        let spec = command_spec(Isolation::Auto, "python3".as_ref(), &["-c".into()], ext.path(), &[]).unwrap();
        assert_eq!(spec.program, OsString::from("bwrap"));
        assert!(spec.args.iter().any(|arg| arg == "--unshare-net"));
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn explicit_linux_sandbox_fails_closed_on_other_platforms() {
        let ext = TempDir::new("auto-ext");
        let error = command_spec(Isolation::LinuxBubblewrap, "node".as_ref(), &[], ext.path(), &[]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
    }
}
