use std::path::Path;

/// Per-worker hard resource limits applied through a delegated Linux cgroup v2 subtree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerResourceLimits {
    /// Hard `memory.max` limit in bytes.
    pub memory_bytes: u64,
    /// Hard `pids.max` limit; the kernel counts tasks, including threads.
    pub pids_max: u32,
    /// `cpu.max` quota in microseconds.
    pub cpu_quota_micros: u64,
    /// `cpu.max` period in microseconds.
    pub cpu_period_micros: u64,
}

impl WorkerResourceLimits {
    pub fn validate(self) -> std::io::Result<()> {
        if self.memory_bytes == 0 || self.pids_max == 0 || self.cpu_quota_micros == 0 || self.cpu_period_micros == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "worker cgroup limits must be positive"));
        }
        Ok(())
    }

    #[cfg(any(target_os = "linux", test))]
    fn cpu_max_value(self) -> String {
        format!("{} {}", self.cpu_quota_micros, self.cpu_period_micros)
    }

    #[cfg(any(target_os = "linux", test))]
    fn memory_max_value(self) -> String {
        self.memory_bytes.to_string()
    }

    #[cfg(any(target_os = "linux", test))]
    fn pids_max_value(self) -> String {
        self.pids_max.to_string()
    }
}

pub(crate) fn validate_config(root: Option<&Path>, limits: Option<WorkerResourceLimits>) -> std::io::Result<()> {
    match (root, limits) {
        (None, None) => Ok(()),
        (Some(_), Some(limits)) => {
            limits.validate()?;
            #[cfg(not(target_os = "linux"))]
            {
                Err(unsupported("worker cgroup resource limits require Linux cgroup v2"))
            }
            #[cfg(target_os = "linux")]
            {
                Ok(())
            }
        }
        (None, Some(_)) => {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "worker cgroup_root is required when resource_limits are configured"))
        }
        (Some(_), None) => {
            Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "worker resource_limits are required when cgroup_root is configured"))
        }
    }
}

pub(crate) fn create(root: Option<&Path>, limits: Option<WorkerResourceLimits>) -> std::io::Result<Option<CgroupGuard>> {
    validate_config(root, limits)?;
    match (root, limits) {
        (Some(_root), Some(_limits)) => {
            #[cfg(target_os = "linux")]
            {
                CgroupGuard::create(_root, _limits).map(Some)
            }
            #[cfg(not(target_os = "linux"))]
            {
                Err(unsupported("worker cgroup resource limits require Linux cgroup v2"))
            }
        }
        (None, None) => Ok(None),
        _ => unreachable!("validated cgroup configuration must contain both fields or neither"),
    }
}

#[cfg(not(target_os = "linux"))]
#[derive(Debug)]
pub(crate) struct CgroupGuard;

#[cfg(not(target_os = "linux"))]
impl CgroupGuard {
    pub(crate) fn attach(&self, _command: &mut tokio::process::Command) -> std::io::Result<()> {
        Err(unsupported("worker cgroup resource limits require Linux cgroup v2"))
    }

    pub(crate) fn kill(&self) -> std::io::Result<()> {
        Ok(())
    }

    pub(crate) async fn terminate(self) -> std::io::Result<()> {
        Err(unsupported("worker cgroup resource limits require Linux cgroup v2"))
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::WorkerResourceLimits;
    use std::fs::{self, File, OpenOptions};
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    static NEXT_CGROUP: AtomicU64 = AtomicU64::new(0);
    const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

    pub(crate) struct CgroupGuard {
        path: PathBuf,
        removed: bool,
    }

    impl CgroupGuard {
        pub(crate) fn create(root: &Path, limits: WorkerResourceLimits) -> io::Result<Self> {
            limits.validate()?;
            let root = root.canonicalize()?;
            if !root.is_dir() || !root.join("cgroup.controllers").is_file() || !root.join("cgroup.procs").is_file() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "cgroup_root is not a delegated cgroup v2 directory"));
            }
            let controllers = fs::read_to_string(root.join("cgroup.controllers"))?;
            for required in ["cpu", "memory", "pids"] {
                if !controllers.split_ascii_whitespace().any(|controller| controller == required) {
                    return Err(io::Error::new(io::ErrorKind::Unsupported, format!("cgroup_root does not provide the `{required}` controller")));
                }
            }

            let unique = NEXT_CGROUP.fetch_add(1, Ordering::Relaxed);
            let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
            let path = root.join(format!("aip-worker-{}-{timestamp}-{unique}", std::process::id()));
            fs::create_dir(&path)?;
            let mut guard = Self { path, removed: false };
            if let Err(error) = guard.apply_limits(&limits) {
                guard.removed = true;
                if let Err(cleanup_error) = fs::remove_dir(&guard.path) {
                    eprintln!("empty worker cgroup cleanup failed for {}: {cleanup_error}", guard.path.display());
                }
                return Err(error);
            }
            Ok(guard)
        }

        fn apply_limits(&self, limits: &WorkerResourceLimits) -> io::Result<()> {
            for name in ["cgroup.procs", "cgroup.kill", "cgroup.events", "memory.max", "memory.oom.group", "pids.max", "cpu.max"] {
                if !self.path.join(name).is_file() {
                    return Err(io::Error::new(io::ErrorKind::Unsupported, format!("delegated cgroup is missing required `{name}` interface")));
                }
            }
            write_control(&self.path.join("memory.max"), &limits.memory_max_value())?;
            write_control(&self.path.join("memory.oom.group"), "1")?;
            write_control(&self.path.join("pids.max"), &limits.pids_max_value())?;
            write_control(&self.path.join("cpu.max"), &limits.cpu_max_value())?;
            Ok(())
        }

        pub(crate) fn attach(&self, command: &mut tokio::process::Command) -> io::Result<()> {
            self.attach_std(command.as_std_mut())
        }

        fn attach_std(&self, command: &mut std::process::Command) -> io::Result<()> {
            let mut procs = OpenOptions::new().write(true).open(self.path.join("cgroup.procs"))?;
            let fd = procs.as_raw_fd();
            if fd <= 2 {
                let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
                if duplicate < 0 {
                    return Err(io::Error::last_os_error());
                }
                procs = unsafe { File::from_raw_fd(duplicate) };
            }
            let fd = procs.as_raw_fd();
            // SAFETY: the hook only calls getpid and write on a pre-opened descriptor before exec.
            // It does not allocate, lock, or access environment state after fork.
            unsafe {
                command.pre_exec(move || {
                    let _keep_descriptor_open = procs.as_raw_fd();
                    let mut digits = [0_u8; 12];
                    let mut cursor = digits.len() - 1;
                    digits[cursor] = b'\n';
                    let mut pid = libc::getpid() as u32;
                    loop {
                        cursor -= 1;
                        digits[cursor] = b'0' + (pid % 10) as u8;
                        pid /= 10;
                        if pid == 0 {
                            break;
                        }
                    }
                    let size = digits.len() - cursor;
                    let written = libc::write(fd, digits[cursor..].as_ptr().cast(), size);
                    if written == size as isize {
                        Ok(())
                    } else if written < 0 {
                        Err(io::Error::last_os_error())
                    } else {
                        Err(io::Error::from_raw_os_error(libc::EIO))
                    }
                });
            }
            Ok(())
        }

        pub(crate) fn kill(&self) -> io::Result<()> {
            write_control(&self.path.join("cgroup.kill"), "1")
        }

        #[cfg(test)]
        pub(crate) fn path(&self) -> &Path {
            &self.path
        }

        pub(crate) async fn terminate(mut self) -> io::Result<()> {
            self.kill()?;
            let deadline = tokio::time::Instant::now() + CLEANUP_TIMEOUT;
            loop {
                if !self.is_populated()? {
                    fs::remove_dir(&self.path)?;
                    self.removed = true;
                    return Ok(());
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "worker cgroup remained populated after kill"));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }

        fn is_populated(&self) -> io::Result<bool> {
            let events = fs::read_to_string(self.path.join("cgroup.events"))?;
            let populated = events
                .lines()
                .find_map(|line| line.strip_prefix("populated "))
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "cgroup.events has no populated field"))?;
            match populated {
                "0" => Ok(false),
                "1" => Ok(true),
                _ => Err(io::Error::new(io::ErrorKind::InvalidData, "cgroup.events populated field is invalid")),
            }
        }

        fn remove_if_empty(&mut self) -> io::Result<bool> {
            if !self.is_populated()? {
                fs::remove_dir(&self.path)?;
                self.removed = true;
                return Ok(true);
            }
            Ok(false)
        }
    }

    impl Drop for CgroupGuard {
        fn drop(&mut self) {
            if self.removed {
                return;
            }
            if let Err(error) = self.kill() {
                eprintln!("worker cgroup kill failed for {}: {error}", self.path.display());
            }
            match self.remove_if_empty() {
                Ok(true) => return,
                Err(error) => eprintln!("worker cgroup immediate cleanup failed for {}: {error}", self.path.display()),
                Ok(false) => {}
            }
            let path = self.path.clone();
            let cleanup = std::thread::Builder::new().name("aip-cgroup-cleanup".into()).spawn(move || {
                let deadline = std::time::Instant::now() + CLEANUP_TIMEOUT;
                loop {
                    let events = match fs::read_to_string(path.join("cgroup.events")) {
                        Ok(events) => events,
                        Err(error) => {
                            eprintln!("worker cgroup cleanup status failed for {}: {error}", path.display());
                            return;
                        }
                    };
                    if events.lines().any(|line| line == "populated 0") {
                        if let Err(error) = fs::remove_dir(&path) {
                            eprintln!("worker cgroup removal failed for {}: {error}", path.display());
                        }
                        return;
                    }
                    if std::time::Instant::now() >= deadline {
                        eprintln!("worker cgroup cleanup timed out for {}", path.display());
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            });
            if let Err(error) = cleanup {
                eprintln!("worker cgroup cleanup thread could not start: {error}");
            }
        }
    }

    #[cfg(test)]
    impl CgroupGuard {
        pub(crate) fn from_existing(path: PathBuf) -> io::Result<Self> {
            Ok(Self { path, removed: true })
        }

        pub(crate) fn apply_limits_for_test(&self, limits: &WorkerResourceLimits) -> io::Result<()> {
            self.apply_limits(limits)
        }

        pub(crate) fn attach_std_for_test(&self, command: &mut std::process::Command) -> io::Result<()> {
            CgroupGuard::attach_std(self, command)
        }
    }

    fn write_control(path: &Path, value: &str) -> io::Result<()> {
        use std::io::Write;
        let content = format!("{value}\n");
        let written = OpenOptions::new().write(true).open(path)?.write(content.as_bytes())?;
        if written == content.len() {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::WriteZero, "partial cgroup control write"))
        }
    }

    pub(crate) use CgroupGuard as Guard;
}

#[cfg(target_os = "linux")]
pub(crate) use linux::Guard as CgroupGuard;

#[cfg(not(target_os = "linux"))]
fn unsupported(message: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Unsupported, message)
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use super::CgroupGuard;
    use super::WorkerResourceLimits;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("aip-cgroup-test-{}-{}", std::process::id(), NEXT_DIR.fetch_add(1, Ordering::Relaxed)));
            std::fs::create_dir(&path).unwrap();
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

    fn limits() -> WorkerResourceLimits {
        WorkerResourceLimits { memory_bytes: 64 * 1024 * 1024, pids_max: 12, cpu_quota_micros: 50_000, cpu_period_micros: 100_000 }
    }

    #[cfg(target_os = "linux")]
    fn delegated_root() -> Option<PathBuf> {
        match std::env::var_os("AIP_TEST_CGROUP_V2_ROOT") {
            Some(root) => Some(PathBuf::from(root)),
            None => {
                eprintln!("skipped real cgroup v2 test; configure AIP_TEST_CGROUP_V2_ROOT to a delegated cgroup v2 directory");
                None
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn counter(path: &std::path::Path, file: &str, name: &str) -> u64 {
        std::fs::read_to_string(path.join(file))
            .unwrap()
            .lines()
            .find_map(|line| {
                let mut fields = line.split_ascii_whitespace();
                if fields.next()? != name {
                    return None;
                }
                fields.next()?.parse::<u64>().ok()
            })
            .unwrap_or_else(|| panic!("{name} counter was missing from {}", path.join(file).display()))
    }

    #[test]
    fn validates_positive_limits_and_formats_cgroup_values() {
        let limits = limits();
        limits.validate().unwrap();
        assert_eq!(limits.cpu_max_value(), "50000 100000");
        assert_eq!(limits.memory_max_value(), "67108864");
        assert_eq!(limits.pids_max_value(), "12");
        assert!(WorkerResourceLimits { memory_bytes: 0, ..limits }.validate().is_err());
        assert!(WorkerResourceLimits { pids_max: 0, ..limits }.validate().is_err());
        assert!(WorkerResourceLimits { cpu_quota_micros: 0, ..limits }.validate().is_err());
        assert!(WorkerResourceLimits { cpu_period_micros: 0, ..limits }.validate().is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn process_is_placed_before_exec_and_fake_limits_are_written() {
        use std::process::Command;

        let root = TempDir::new();
        let group = root.path().join("worker-1");
        std::fs::create_dir(&group).unwrap();
        for (name, value) in [
            ("cgroup.procs", ""),
            ("cgroup.kill", ""),
            ("cgroup.events", "populated 0\n"),
            ("memory.max", ""),
            ("memory.oom.group", ""),
            ("pids.max", ""),
            ("cpu.max", ""),
        ] {
            std::fs::write(group.join(name), value).unwrap();
        }

        let guard = CgroupGuard::from_existing(group.clone()).unwrap();
        guard.apply_limits_for_test(&limits()).unwrap();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", &format!("test \"$(cat {}/cgroup.procs)\" = \"$$\" && printf attached", group.display())]);
        guard.attach_std_for_test(&mut command).unwrap();
        let output = command.output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(output.stdout, b"attached");
        assert_eq!(std::fs::read_to_string(group.join("memory.max")).unwrap().trim(), "67108864");
        assert_eq!(std::fs::read_to_string(group.join("memory.oom.group")).unwrap().trim(), "1");
        assert_eq!(std::fs::read_to_string(group.join("pids.max")).unwrap().trim(), "12");
        assert_eq!(std::fs::read_to_string(group.join("cpu.max")).unwrap().trim(), "50000 100000");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn delegated_cgroup_v2_applies_limits_and_contains_a_worker_when_configured() {
        let Some(root) = delegated_root() else {
            return;
        };
        let group = super::create(Some(&root), Some(limits())).unwrap().unwrap();
        let group_path = group.path().to_path_buf();
        let mut command = tokio::process::Command::new("/bin/sleep");
        command.arg("30").kill_on_drop(true);
        group.attach(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        let pid = child.id().unwrap().to_string();
        let members = std::fs::read_to_string(group_path.join("cgroup.procs")).unwrap();
        assert!(members.lines().any(|member| member == pid), "pre-exec cgroup placement missing pid {pid}: {members:?}");
        assert_eq!(std::fs::read_to_string(group_path.join("memory.max")).unwrap().trim(), "67108864");
        assert_eq!(std::fs::read_to_string(group_path.join("memory.oom.group")).unwrap().trim(), "1");
        assert_eq!(std::fs::read_to_string(group_path.join("pids.max")).unwrap().trim(), "12");
        assert_eq!(std::fs::read_to_string(group_path.join("cpu.max")).unwrap().trim(), "50000 100000");
        group.terminate().await.unwrap();
        let _ = child.wait().await.unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn delegated_cgroup_v2_enforces_limits_and_drop_cleans_descendants() {
        use std::process::Stdio;
        use std::time::Duration;
        use tokio::process::Command;

        let Some(root) = delegated_root() else {
            return;
        };

        let cpu_group = super::create(Some(&root), Some(WorkerResourceLimits { cpu_quota_micros: 10_000, cpu_period_micros: 100_000, ..limits() }))
            .unwrap()
            .unwrap();
        let cpu_path = cpu_group.path().to_path_buf();
        let mut cpu_worker = Command::new("/bin/sh");
        cpu_worker.args(["-c", "while :; do :; done"]).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
        cpu_group.attach(&mut cpu_worker).unwrap();
        let mut cpu_worker = cpu_worker.spawn().unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while counter(&cpu_path, "cpu.stat", "nr_throttled") == 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(counter(&cpu_path, "cpu.stat", "nr_throttled") > 0, "cpu.max did not throttle the busy worker");
        cpu_group.terminate().await.unwrap();
        let _ = cpu_worker.wait().await.unwrap();

        let pids_group = super::create(Some(&root), Some(WorkerResourceLimits { pids_max: 2, ..limits() })).unwrap().unwrap();
        let pids_path = pids_group.path().to_path_buf();
        let mut pids_worker = Command::new("/bin/sh");
        pids_worker.args(["-c", "sleep 30 & sleep 30 & wait"]).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
        pids_group.attach(&mut pids_worker).unwrap();
        let mut pids_worker = pids_worker.spawn().unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while counter(&pids_path, "pids.events", "max") == 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(counter(&pids_path, "pids.events", "max") > 0, "pids.max did not reject a child process");
        pids_group.terminate().await.unwrap();
        let _ = pids_worker.wait().await.unwrap();

        let memory_group = super::create(Some(&root), Some(limits())).unwrap().unwrap();
        let memory_path = memory_group.path().to_path_buf();
        let initial_oom_kills = counter(&memory_path, "memory.events", "oom_kill");
        let mut memory_worker = Command::new("/usr/bin/python3");
        memory_worker
            .args([
                "-c",
                "import time; data = bytearray(128 * 1024 * 1024); [data.__setitem__(i, 1) for i in range(0, len(data), 4096)]; time.sleep(30)",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        memory_group.attach(&mut memory_worker).unwrap();
        let mut memory_worker = memory_worker.spawn().unwrap();
        let status = tokio::time::timeout(Duration::from_secs(15), memory_worker.wait()).await.expect("memory-limited worker did not exit").unwrap();
        assert!(!status.success(), "memory-limited worker unexpectedly survived a 128 MiB allocation");
        assert!(counter(&memory_path, "memory.events", "oom_kill") > initial_oom_kills, "memory.max did not trigger a cgroup OOM kill");
        memory_group.terminate().await.unwrap();

        let descendants_group = super::create(Some(&root), Some(limits())).unwrap().unwrap();
        let descendants_path = descendants_group.path().to_path_buf();
        let pid_file =
            std::env::temp_dir().join(format!("aip-cgroup-descendant-{}-{}", std::process::id(), NEXT_DIR.fetch_add(1, Ordering::Relaxed)));
        let mut descendants_worker = Command::new("/bin/sh");
        descendants_worker
            .args(["-c", "sleep 30 & echo $! > \"$1\"; wait", "aip-cgroup-test", pid_file.to_str().unwrap()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        descendants_group.attach(&mut descendants_worker).unwrap();
        let mut descendants_worker = descendants_worker.spawn().unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while !pid_file.exists() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(pid_file.exists(), "worker did not create its descendant");
        drop(descendants_group);
        let _ = tokio::time::timeout(Duration::from_secs(5), descendants_worker.wait()).await.expect("worker survived cgroup drop").unwrap();
        let cleanup_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while descendants_path.exists() && tokio::time::Instant::now() < cleanup_deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!descendants_path.exists(), "dropping the guard left the worker cgroup and descendants behind");
        let _ = std::fs::remove_file(pid_file);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn requested_cgroup_limits_fail_closed_on_non_linux() {
        let root = TempDir::new();
        let error = super::create(Some(root.path()), Some(limits())).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
    }
}
