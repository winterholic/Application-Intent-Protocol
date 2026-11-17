use std::ffi::{OsStr, OsString};

/// Automatic isolation uses the platform's worker sandbox: MacSandbox on macOS, LinuxBubblewrap on Linux.
/// MacNetDeny blocks network access only. MacSandbox also denies file writes, reads of user data
/// directories and spawning processes. LinuxBubblewrap limits visible files and runs in its own PID namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Isolation {
    /// Select MacSandbox on macOS and LinuxBubblewrap on Linux.
    Auto,
    /// Run without an OS sandbox. Only use for explicitly trusted server-side configurations.
    None,
    /// macOS network denial; host filesystem remains visible to the worker.
    MacNetDeny,
    /// macOS Seatbelt profile: no network, no file writes, no reads under `/Users`, `/Volumes` and
    /// temporary directories except the extension and runtime, and no `fork`/`spawn`.
    MacSandbox,
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
            Self::MacNetDeny | Self::MacSandbox => {
                #[cfg(target_os = "macos")]
                {
                    validate_executable(std::path::Path::new("/usr/bin/sandbox-exec"))
                }
                #[cfg(not(target_os = "macos"))]
                {
                    Err(unsupported("MacNetDeny and MacSandbox require macOS"))
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
    /// Server-trusted delegated cgroup v2 directory. Its cpu, memory, and pids controllers must be enabled for children.
    pub cgroup_root: Option<std::path::PathBuf>,
    /// Per-worker hard limits. Requires `cgroup_root`; unsupported outside Linux.
    pub resource_limits: Option<crate::WorkerResourceLimits>,
}

impl Default for WorkerLaunchConfig {
    fn default() -> Self {
        Self {
            isolation: Isolation::Auto,
            runtime_executable: None,
            environment: std::collections::HashMap::new(),
            read_only_paths: Vec::new(),
            cgroup_root: None,
            resource_limits: None,
        }
    }
}

impl WorkerLaunchConfig {
    /// Verify the selected runtime, mounts, environment, and isolation before accepting work.
    pub async fn probe(&self, lang: crate::Lang, ext_dir: &std::path::Path) -> std::io::Result<()> {
        #[cfg(target_os = "linux")]
        let diagnostic = std::env::var("AIP_TEST_PROBE_DIAGNOSTICS").as_deref() == Ok("1");
        #[cfg(target_os = "linux")]
        let probe_started = std::time::Instant::now();
        crate::cgroup::validate_config(self.cgroup_root.as_deref(), self.resource_limits)?;
        self.isolation.validate_available()?;
        let program = runtime_path(lang, self)?;
        let ext_dir = extension_path(self.isolation, ext_dir)?;
        let args = [OsString::from("--version")];
        let spec = command_spec(self.isolation, program.as_os_str(), &args, &ext_dir, &self.read_only_paths)?;
        #[cfg(target_os = "linux")]
        let (probe_program, probe_args) = (spec.program.clone(), spec.args.clone());
        let mut command = tokio::process::Command::new(spec.program);
        command.args(spec.args);
        configure_environment(&mut command, self)?;
        set_working_directory(&mut command, self.isolation, &ext_dir);
        command.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
        let cgroup = crate::cgroup::create(self.cgroup_root.as_deref(), self.resource_limits)?;
        if let Some(cgroup) = &cgroup {
            cgroup.attach(&mut command)?;
        }
        let mut child = command.spawn()?;
        #[cfg(target_os = "linux")]
        let probe_sequence = {
            static NEXT_PROBE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            NEXT_PROBE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        };
        #[cfg(target_os = "linux")]
        if diagnostic {
            eprintln!(
                "AIP_PROBE_DIAG event=start sequence={probe_sequence} condition={} pid={} lang={lang:?} isolation={:?} runtime={program:?} launch_program={probe_program:?} launch_args={probe_args:?}",
                std::env::var("AIP_EXPERIMENT_CONDITION").unwrap_or_else(|_| "unset".to_owned()),
                child.id().unwrap_or_default(),
                self.isolation,
            );
        }
        let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("worker runtime probe stdout pipe was not created"))?;
        let stderr = child.stderr.take().ok_or_else(|| std::io::Error::other("worker runtime probe stderr pipe was not created"))?;
        let stdout_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let stderr_bytes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let probe = async {
            let (status, stdout, stderr) =
                tokio::join!(child.wait(), capture_bounded(stdout, 4096, stdout_bytes.clone()), capture_bounded(stderr, 4096, stderr_bytes.clone()));
            let status = status?;
            let stdout = stdout?;
            let stderr = stderr?;
            #[cfg(target_os = "linux")]
            if diagnostic {
                eprintln!(
                    "AIP_PROBE_DIAG event=exit sequence={probe_sequence} elapsed_ms={} status={status} stdout={:?} stderr={:?}",
                    probe_started.elapsed().as_millis(),
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr)
                );
            }
            if !status.success() {
                return Err(std::io::Error::other(format!(
                    "worker runtime probe exited with {status}; stdout={:?}; stderr={:?}",
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr)
                )));
            }
            if stdout.is_empty() && stderr.is_empty() {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "worker runtime probe produced no version output"));
            }
            Ok(())
        };
        let result = match tokio::time::timeout(std::time::Duration::from_secs(2), probe).await {
            Ok(result) => result,
            Err(_) => {
                let pid = child.id().unwrap_or_default();
                let child_status = match child.try_wait() {
                    Ok(Some(status)) => format!("exited({status})"),
                    Ok(None) => "running".to_owned(),
                    Err(error) => format!("unknown({error})"),
                };
                #[cfg(target_os = "linux")]
                let process_sample = if diagnostic { linux_probe_process_sample(pid) } else { String::new() };
                #[cfg(not(target_os = "linux"))]
                let process_sample = String::new();
                #[cfg(target_os = "linux")]
                if diagnostic {
                    eprintln!(
                        "AIP_PROBE_DIAG event=timeout sequence={probe_sequence} elapsed_ms={} pid={pid} child_status={child_status} proc_sample={process_sample:?} stdout_bytes_read={} stderr_bytes_read={} action=start_kill",
                        probe_started.elapsed().as_millis(),
                        stdout_bytes.load(std::sync::atomic::Ordering::Relaxed),
                        stderr_bytes.load(std::sync::atomic::Ordering::Relaxed)
                    );
                }
                let _ = child.start_kill();
                let residual_pid_present = std::path::PathBuf::from("/proc").join(pid.to_string()).exists();
                #[cfg(target_os = "linux")]
                if diagnostic {
                    eprintln!(
                        "AIP_PROBE_DIAG event=post-kill sequence={probe_sequence} pid={pid} residual_pid_present={residual_pid_present} child_status={:?}",
                        child.try_wait()
                    );
                }
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "worker runtime probe exceeded 2 seconds; pid={pid}; child_status={child_status}; proc_sample={process_sample:?}; residual_pid_present={residual_pid_present}; stdout_bytes_read={}; stderr_bytes_read={}",
                        stdout_bytes.load(std::sync::atomic::Ordering::Relaxed),
                        stderr_bytes.load(std::sync::atomic::Ordering::Relaxed)
                    ),
                ))
            }
        };
        if let Some(cgroup) = cgroup {
            if let Err(error) = cgroup.terminate().await {
                return Err(std::io::Error::new(error.kind(), format!("worker cgroup cleanup failed: {error}")));
            }
        }
        result
    }
}

/// Read only a few capped `/proc` fields for timeout diagnostics; this never waits for the child.
#[cfg(target_os = "linux")]
fn linux_probe_process_sample(pid: u32) -> String {
    use std::io::Read;

    fn prefix(path: &std::path::Path, limit: u64) -> String {
        let Ok(file) = std::fs::File::open(path) else { return "<unavailable>".to_owned() };
        let mut bytes = Vec::new();
        if file.take(limit).read_to_end(&mut bytes).is_err() {
            return "<read-error>".to_owned();
        }
        String::from_utf8_lossy(&bytes).replace('\0', " ").replace('\n', " ")
    }

    let process = std::path::PathBuf::from("/proc").join(pid.to_string());
    format!(
        "status={:?} wchan={:?} cmdline={:?}",
        prefix(&process.join("status"), 512),
        prefix(&process.join("wchan"), 128),
        prefix(&process.join("cmdline"), 256)
    )
}

pub(crate) fn runtime_path(lang: crate::Lang, config: &WorkerLaunchConfig) -> std::io::Result<std::path::PathBuf> {
    let default_program = match lang {
        crate::Lang::Node => "node",
        crate::Lang::Python => "python3",
    };
    let requested = config.runtime_executable.as_deref().unwrap_or_else(|| std::path::Path::new(default_program));
    #[cfg(target_os = "linux")]
    {
        resolve_runtime(requested)
    }
    #[cfg(not(target_os = "linux"))]
    {
        if requested.is_absolute() || requested.components().count() > 1 {
            let parent =
                requested.parent().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "worker runtime path has no parent"))?;
            Ok(parent.canonicalize()?.join(
                requested
                    .file_name()
                    .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "worker runtime path has no executable name"))?,
            ))
        } else {
            Ok(requested.to_path_buf())
        }
    }
}

pub(crate) fn extension_path(isolation: Isolation, ext_dir: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    match isolation {
        Isolation::None | Isolation::MacNetDeny => Ok(ext_dir.to_path_buf()),
        Isolation::Auto | Isolation::MacSandbox | Isolation::LinuxBubblewrap => ext_dir.canonicalize(),
    }
}

/// The macOS sandbox hides the server's working directory, and Node aborts when `cwd()` is unreadable.
pub(crate) fn set_working_directory(command: &mut tokio::process::Command, isolation: Isolation, ext_dir: &std::path::Path) {
    if cfg!(target_os = "macos") && matches!(isolation, Isolation::Auto | Isolation::MacSandbox) {
        command.current_dir(ext_dir);
    }
}

pub(crate) fn configure_environment(command: &mut tokio::process::Command, config: &WorkerLaunchConfig) -> std::io::Result<()> {
    command.env_clear().env("PATH", std::env::var("PATH").unwrap_or_default());
    for (key, value) in &config.environment {
        if key.is_empty() || key.contains('=') || key.as_bytes().contains(&0) || value.as_bytes().contains(&0) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "worker environment contains an invalid key or value"));
        }
        command.env(key, value);
    }
    Ok(())
}

async fn capture_bounded<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
    bytes_read: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut output = Vec::with_capacity(limit.min(1024));
    let mut buffer = [0u8; 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(output);
        }
        bytes_read.fetch_add(read, std::sync::atomic::Ordering::Relaxed);
        let remaining = limit.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..read.min(remaining)]);
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
                mac_sandbox_spec(program, args, _ext_dir, _read_only_paths)
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
        Isolation::MacSandbox => {
            #[cfg(target_os = "macos")]
            {
                mac_sandbox_spec(program, args, _ext_dir, _read_only_paths)
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(unsupported("MacSandbox requires macOS"))
            }
        }
        Isolation::MacNetDeny => {
            #[cfg(target_os = "macos")]
            {
                mac_net_deny_spec(program, args)
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
fn mac_net_deny_spec(program: &OsStr, args: &[OsString]) -> std::io::Result<CommandSpec> {
    let mut sandbox_args = vec!["-p".into(), "(version 1)(allow default)(deny network*)".into(), program.to_owned()];
    sandbox_args.extend_from_slice(args);
    Ok(CommandSpec { program: "/usr/bin/sandbox-exec".into(), args: sandbox_args })
}

/// Quotes a path as an SBPL string literal. Seatbelt reads the profile as text, so a quote or
/// backslash in a path must not be able to close the literal and add rules.
#[cfg(target_os = "macos")]
fn sbpl_string(path: &std::path::Path) -> std::io::Result<String> {
    let text = path.to_str().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "sandbox path is not valid UTF-8"))?;
    if text.chars().any(char::is_control) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "sandbox path contains a control character"));
    }
    Ok(format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"")))
}

/// Directories the runtime itself needs, so a Node or Python under a denied root (`~/.nvm`, a venv) still starts.
#[cfg(target_os = "macos")]
fn runtime_roots(program: &OsStr) -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();
    let program = std::path::Path::new(program);
    if program.components().count() == 1 {
        if let Some(path) = std::env::var_os("PATH") {
            candidates.extend(std::env::split_paths(&path).map(|dir| dir.join(program)).filter(|path| path.exists()));
        }
    } else {
        candidates.push(program.to_path_buf());
    }
    let mut roots = Vec::new();
    for candidate in candidates {
        let real = candidate.canonicalize().unwrap_or_else(|_| candidate.clone());
        for path in [candidate, real] {
            let Some(parent) = path.parent() else { continue };
            // `<prefix>/bin/node` needs `<prefix>/lib`; anything else only gets its own directory.
            let root = if parent.file_name().is_some_and(|name| name == "bin") { parent.parent().unwrap_or(parent) } else { parent };
            roots.push(root.to_path_buf());
        }
    }
    roots
}

#[cfg(target_os = "macos")]
fn mac_sandbox_profile(program: &OsStr, ext_dir: &std::path::Path, read_only_paths: &[std::path::PathBuf]) -> std::io::Result<String> {
    // Reads stay open for system libraries and the runtime. Only places that hold user or other
    // processes' data are closed, then the extension and trusted dependencies are opened again.
    // Seatbelt prefers the more specific operation, so the allow rules name `file-read-data` too.
    let mut profile = String::from(
        "(version 1)(allow default)(deny network*)(deny process-fork)(deny file-write*)\n\
         (allow file-write* (literal \"/dev/null\") (literal \"/dev/dtracehelper\"))\n\
         (deny file-read-data (subpath \"/Users\") (subpath \"/Volumes\") (subpath \"/private/var/folders\") (subpath \"/private/tmp\") (subpath \"/private/var/tmp\") (subpath \"/private/var/root\"))\n",
    );
    let mut allowed = vec![ext_dir.to_path_buf()];
    allowed.extend(read_only_paths.iter().cloned());
    allowed.extend(runtime_roots(program));
    for path in allowed {
        let path = path.canonicalize().unwrap_or(path);
        profile.push_str(&format!("(allow file-read-data (subpath {}))\n", sbpl_string(&path)?));
    }
    Ok(profile)
}

#[cfg(target_os = "macos")]
fn mac_sandbox_spec(
    program: &OsStr,
    args: &[OsString],
    ext_dir: &std::path::Path,
    read_only_paths: &[std::path::PathBuf],
) -> std::io::Result<CommandSpec> {
    let profile = mac_sandbox_profile(program, ext_dir, read_only_paths)?;
    let mut sandbox_args = vec!["-p".into(), profile.into(), program.to_owned()];
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

    #[cfg(target_os = "linux")]
    struct LinuxStartupSample {
        passed: bool,
        details: String,
    }

    #[cfg(target_os = "linux")]
    async fn collect_startup_pipe(mut task: tokio::task::JoinHandle<std::io::Result<Vec<u8>>>) -> (usize, bool) {
        match tokio::time::timeout(std::time::Duration::from_millis(250), &mut task).await {
            Ok(Ok(Ok(bytes))) => (bytes.len(), true),
            _ => {
                task.abort();
                (0, false)
            }
        }
    }

    #[cfg(target_os = "linux")]
    async fn sample_linux_startup(label: String, program: OsString, args: Vec<OsString>) -> LinuxStartupSample {
        use tokio::io::AsyncReadExt;

        let mut command = tokio::process::Command::new(&program);
        command.args(&args).env_clear().env("PATH", std::env::var("PATH").unwrap_or_default());
        command.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                return LinuxStartupSample { passed: false, details: format!("{label}: spawn {program:?} failed: {error}") };
            }
        };
        let pid = child.id().expect("startup child has a PID");
        let mut child_stdout = child.stdout.take().unwrap();
        let mut child_stderr = child.stderr.take().unwrap();
        let stdout = tokio::spawn(async move {
            let mut bytes = Vec::new();
            let read = child_stdout.read_to_end(&mut bytes).await;
            read.map(|_| bytes)
        });
        let stderr = tokio::spawn(async move {
            let mut bytes = Vec::new();
            let read = child_stderr.read_to_end(&mut bytes).await;
            read.map(|_| bytes)
        });
        let started = tokio::time::Instant::now();
        let mut snapshots = Vec::new();
        let mut snapshot_after_ms = [0, 50, 250, 750, 1250, 1750].into_iter().peekable();
        let mut timed_out = false;
        let mut wait_error = None;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {
                    if snapshot_after_ms.peek().is_some_and(|checkpoint| started.elapsed().as_millis() >= *checkpoint) {
                        snapshots.push(linux_process_tree(pid));
                        snapshot_after_ms.next();
                    }
                    if started.elapsed() >= std::time::Duration::from_secs(2) {
                        timed_out = true;
                        let _ = child.start_kill();
                        break match tokio::time::timeout(std::time::Duration::from_millis(250), child.wait()).await {
                            Ok(Ok(status)) => Some(status),
                            Ok(Err(error)) => {
                                wait_error = Some(error.to_string());
                                let _ = child.start_kill();
                                None
                            }
                            Err(_) => {
                                wait_error = Some("child wait exceeded 250ms after kill".to_owned());
                                let _ = child.start_kill();
                                None
                            }
                        };
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
                Err(error) => {
                    wait_error = Some(format!("try_wait failed: {error}"));
                    let _ = child.start_kill();
                    break match tokio::time::timeout(std::time::Duration::from_millis(250), child.wait()).await {
                        Ok(Ok(status)) => Some(status),
                        Ok(Err(error)) => {
                            wait_error = Some(format!("try_wait failed; wait after kill failed: {error}"));
                            let _ = child.start_kill();
                            None
                        }
                        Err(_) => {
                            wait_error = Some("try_wait failed; child wait exceeded 250ms after kill".to_owned());
                            let _ = child.start_kill();
                            None
                        }
                    };
                }
            }
        };
        let (stdout, stdout_eof) = collect_startup_pipe(stdout).await;
        let (stderr, stderr_eof) = collect_startup_pipe(stderr).await;
        let observed_pids = snapshots
            .iter()
            .flat_map(|snapshot| snapshot.split("pid=").skip(1).filter_map(|entry| entry.split_whitespace().next()?.parse::<u32>().ok()))
            .chain(std::iter::once(pid))
            .collect::<std::collections::HashSet<_>>();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let residual_pids =
            observed_pids.into_iter().filter(|observed| std::path::PathBuf::from("/proc").join(observed.to_string()).exists()).collect::<Vec<_>>();
        let status_success = status.as_ref().is_some_and(|status| status.success());
        let passed = !timed_out && status_success && stdout > 0 && stdout_eof && stderr_eof && residual_pids.is_empty() && wait_error.is_none();
        LinuxStartupSample {
            passed,
            details: format!(
                "{label}: pid={pid} program={program:?} elapsed_ms={} status={status:?} timed_out={timed_out} wait_error={wait_error:?} stdout_bytes={stdout} stdout_eof={stdout_eof} stderr_bytes={stderr} stderr_eof={stderr_eof} residual_pids={residual_pids:?} proc_samples={snapshots:?}",
                started.elapsed().as_millis()
            ),
        }
    }

    #[cfg(target_os = "linux")]
    fn linux_process_tree(root: u32) -> String {
        fn visit(pid: u32, depth: usize, result: &mut Vec<String>) {
            if depth > 4 || result.len() >= 24 {
                return;
            }
            let base = std::path::PathBuf::from("/proc").join(pid.to_string());
            let read = |name: &str| std::fs::read_to_string(base.join(name)).unwrap_or_default().replace('\n', " ");
            let cmdline = std::fs::read(base.join("cmdline"))
                .unwrap_or_default()
                .split(|byte| *byte == 0)
                .filter(|part| !part.is_empty())
                .map(|part| String::from_utf8_lossy(part).into_owned())
                .collect::<Vec<_>>()
                .join(" ");
            let status = read("status").lines().find(|line| line.starts_with("State:")).unwrap_or_default().to_owned();
            let children =
                read(format!("task/{pid}/children").as_str()).split_whitespace().filter_map(|item| item.parse::<u32>().ok()).collect::<Vec<_>>();
            result.push(format!(
                "pid={pid} comm={} {status} wchan={} syscall={} cmd={cmdline:?} children={children:?}",
                read("comm").trim(),
                read("wchan").trim(),
                read("syscall").trim()
            ));
            for child in children {
                visit(child, depth + 1, result);
            }
        }
        let mut result = Vec::new();
        visit(root, 0, &mut result);
        result.join(" | ")
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_bwrap_startup_diagnostics_compare_direct_sequential_and_concurrent() {
        if std::env::var("AIP_TEST_BWRAP_DIAGNOSTICS").as_deref() != Ok("1") {
            return;
        }
        let ext = TempDir::new("startup-diagnostics");
        let config = WorkerLaunchConfig::default();
        let runtime = super::runtime_path(Lang::Node, &config).unwrap();
        let bwrap = super::command_spec(Isolation::LinuxBubblewrap, runtime.as_os_str(), &[OsString::from("--version")], ext.path(), &[]).unwrap();
        eprintln!(
            "startup diagnostics: runtime={} bwrap={:?} args={:?} loadavg={} mem_available={} cpu_pressure={} process_count={}",
            runtime.display(),
            bwrap.program,
            bwrap.args,
            std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim(),
            std::fs::read_to_string("/proc/meminfo").unwrap_or_default().lines().find(|line| line.starts_with("MemAvailable:")).unwrap_or_default(),
            std::fs::read_to_string("/proc/pressure/cpu").unwrap_or_default().trim(),
            std::fs::read_dir("/proc")
                .map(|entries| entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_name().to_string_lossy().bytes().all(|byte| byte.is_ascii_digit()))
                    .count())
                .unwrap_or_default()
        );
        let mut results = Vec::new();
        for index in 0..5 {
            results.push(sample_linux_startup(format!("direct-{index}"), runtime.as_os_str().to_owned(), vec!["--version".into()]).await);
        }
        for index in 0..5 {
            results.push(sample_linux_startup(format!("bwrap-sequential-{index}"), bwrap.program.clone(), bwrap.args.clone()).await);
        }
        let concurrent = (0..4).map(|index| sample_linux_startup(format!("bwrap-concurrent-{index}"), bwrap.program.clone(), bwrap.args.clone()));
        results.extend(futures_util::future::join_all(concurrent).await);
        let failures = results.iter().filter(|result| !result.passed).map(|result| result.details.as_str()).collect::<Vec<_>>();
        for result in &results {
            eprintln!("{}", result.details);
        }
        assert!(failures.is_empty(), "all startup samples collected before failure: {failures:#?}");
    }

    #[cfg(unix)]
    fn probe_runtime(fixture: &str) -> WorkerLaunchConfig {
        // Linux CI reported ETXTBSY for the freshly written hang fixture. Use checked-in scripts,
        // as with the empty-output probe, so these tests do not write executables during startup.
        let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(fixture);
        WorkerLaunchConfig { isolation: Isolation::None, runtime_executable: Some(runtime), ..WorkerLaunchConfig::default() }
    }

    async fn invoke_worker(
        lang: Lang,
        ext_dir: &std::path::Path,
        environment: std::collections::HashMap<String, String>,
        read_only_paths: Vec<PathBuf>,
    ) -> Value {
        let config = WorkerLaunchConfig { environment, read_only_paths, ..WorkerLaunchConfig::default() };
        config.probe(lang, ext_dir).await.expect("isolated worker runtime probe succeeds");
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
        assert!(config.cgroup_root.is_none());
        assert!(config.resource_limits.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_probe_rejects_success_without_version_output() {
        let dir = TempDir::new("probe-empty");
        let config = probe_runtime("empty-runtime.sh");
        let error = config.probe(Lang::Node, dir.path()).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
        assert!(error.to_string().contains("no version output"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_probe_reports_nonzero_status_and_bounded_diagnostics() {
        let dir = TempDir::new("probe-nonzero");
        let config = probe_runtime("failed-runtime.sh");
        let error = config.probe(Lang::Node, dir.path()).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        assert!(error.to_string().contains("42"));
        assert!(error.to_string().contains("probe-failed"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_probe_drains_output_larger_than_capture_limit() {
        let dir = TempDir::new("probe-large-output");
        let config = probe_runtime("large-output-runtime.sh");
        config.probe(Lang::Node, dir.path()).await.unwrap();
    }

    #[tokio::test]
    async fn capture_bounded_caps_stored_bytes_and_drains_to_eof() {
        use tokio::io::AsyncWriteExt;

        let (mut writer, reader) = tokio::io::duplex(8192);
        let writer = tokio::spawn(async move {
            writer.write_all(&vec![b'x'; 8192]).await.unwrap();
        });
        let bytes_read = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let captured = super::capture_bounded(reader, 4096, bytes_read.clone()).await.unwrap();
        writer.await.unwrap();

        assert_eq!(captured.len(), 4096);
        assert_eq!(bytes_read.load(Ordering::Relaxed), 8192);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_probe_deadline_kills_a_hung_runtime() {
        let dir = TempDir::new("probe-hang");
        let config = probe_runtime("hung-runtime.sh");
        let started = tokio::time::Instant::now();
        let error = config.probe(Lang::Node, dir.path()).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(error.to_string().contains("child_status=running"));
        assert!(error.to_string().contains("stdout_bytes_read=0"));
        assert!(error.to_string().contains("stderr_bytes_read=0"));
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
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
    fn auto_uses_macos_sandbox_that_denies_network_writes_and_spawn() {
        let ext = TempDir::new("auto-ext");
        let spec = command_spec(Isolation::Auto, "python3".as_ref(), &["-c".into()], ext.path(), &[]).unwrap();
        assert_eq!(spec.program, OsString::from("/usr/bin/sandbox-exec"));
        let profile = spec.args[1].to_string_lossy().into_owned();
        for rule in ["(deny network*)", "(deny process-fork)", "(deny file-write*)", "(deny file-read-data (subpath \"/Users\")"] {
            assert!(profile.contains(rule), "profile lacks {rule}: {profile}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_net_deny_keeps_its_network_only_profile() {
        let ext = TempDir::new("net-ext");
        let spec = command_spec(Isolation::MacNetDeny, "python3".as_ref(), &["-c".into()], ext.path(), &[]).unwrap();
        assert!(spec.args.iter().any(|arg| arg == "(version 1)(allow default)(deny network*)"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_sandbox_profile_escapes_paths_so_they_cannot_add_rules() {
        let path = std::path::Path::new("/tmp/x\") (allow file-write* (subpath \"/\"))");
        let quoted = super::sbpl_string(path).unwrap();
        assert!(quoted.starts_with('"') && quoted.ends_with('"'));
        assert!(!quoted[1..quoted.len() - 1].replace("\\\"", "").contains('"'), "unescaped quote in {quoted}");
        assert!(super::sbpl_string(std::path::Path::new("/tmp/a\nb")).is_err());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_sandbox_blocks_file_reads_writes_network_and_spawn() {
        let dir = TempDir::new("mac-boundary");
        let host_secret = TempDir::new("mac-host-secret");
        let host_secret_path = host_secret.path().join("credential.txt");
        std::fs::write(&host_secret_path, "host-secret-must-not-be-visible").unwrap();
        std::fs::write(
            dir.path().join("probe.py"),
            "import os, socket, subprocess\ndef run(_input, _ctx):\n    def attempt(fn):\n        try:\n            fn(); return True\n        except Exception:\n            return False\n    here = os.path.dirname(os.path.abspath(__file__))\n    return {\n        'host_file': attempt(lambda: open(os.environ['AIP_HOST_SECRET_PATH']).read()),\n        'home_listing': attempt(lambda: os.listdir('/Users')),\n        'own_source': attempt(lambda: open(__file__).read()),\n        'write_extension_dir': attempt(lambda: open(os.path.join(here, 'escape'), 'w').write('bad')),\n        'write_tmp': attempt(lambda: open('/private/tmp/aip-escape', 'w').write('bad')),\n        'fork': attempt(lambda: os.fork()),\n        'spawn': attempt(lambda: subprocess.run(['/bin/echo', 'x'])),\n        'network': attempt(lambda: socket.create_connection(('1.1.1.1', 53), 0.2)),\n    }\n",
        )
        .unwrap();
        let response = invoke_worker(
            Lang::Python,
            dir.path(),
            [("AIP_HOST_SECRET_PATH".into(), host_secret_path.to_string_lossy().into_owned())].into(),
            vec![],
        )
        .await;
        assert_eq!(
            response["output"],
            json!({"host_file":false,"home_listing":false,"own_source":true,"write_extension_dir":false,"write_tmp":false,"fork":false,"spawn":false,"network":false}),
            "{response}"
        );
        assert!(!std::path::Path::new("/private/tmp/aip-escape").exists());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_sandbox_runs_node_extensions() {
        let dir = TempDir::new("mac-node");
        std::fs::write(
            dir.path().join("probe.mjs"),
            "import { readFileSync } from 'node:fs';\nexport function run() {\n  let secret = false;\n  try { readFileSync(process.env.AIP_HOST_SECRET_PATH); secret = true; } catch {}\n  return { secret };\n}\n",
        )
        .unwrap();
        let host_secret = TempDir::new("mac-node-secret");
        let host_secret_path = host_secret.path().join("credential.txt");
        std::fs::write(&host_secret_path, "x").unwrap();
        let response =
            invoke_worker(Lang::Node, dir.path(), [("AIP_HOST_SECRET_PATH".into(), host_secret_path.to_string_lossy().into_owned())].into(), vec![])
                .await;
        assert_eq!(response["output"], json!({"secret":false}), "{response}");
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
