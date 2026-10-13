//! O-13: `Worker::stop` and failure paths must not leave descendant processes behind,
//! including grandchildren that left the worker's process group with `setsid`.

use crate::{Isolation, Lang, Worker, WorkerLaunchConfig};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("aip-cleanup-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn alive(pid: i32) -> bool {
    // A zombie still answers kill(0); treat it as gone once it is not running.
    (unsafe { libc::kill(pid, 0) } == 0) && !is_zombie(pid)
}

fn is_zombie(pid: i32) -> bool {
    std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim_start().starts_with('Z'))
        .unwrap_or(false)
}

fn wait_for_pid_file(path: &Path) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(pid) = text.trim().parse() {
                return pid;
            }
        }
        assert!(Instant::now() < deadline, "grandchild pid file never appeared");
        std::thread::sleep(Duration::from_millis(20));
    }
}

enum End {
    Stop,
    Drop,
}

async fn assert_gone(spawn_body: &str, end: End) {
    let dir = Dir::new();
    let pid_file = dir.0.join("pid");
    let source = format!(
        "import os, subprocess, sys, time\ndef run(_input, _ctx):\n    pid_file = {pid_file:?}\n{spawn_body}\n    return {{}}\n",
        pid_file = pid_file.to_string_lossy(),
        spawn_body = spawn_body,
    );
    std::fs::write(dir.0.join("probe.py"), source).unwrap();
    let config = WorkerLaunchConfig { isolation: Isolation::None, ..WorkerLaunchConfig::default() };
    let mut worker = Worker::try_start_configured(Lang::Python, &dir.0.to_string_lossy(), config).await.expect("worker starts");
    worker.send(&json!({"type":"invoke","invoke":1,"impl":"probe.run","input":{},"token":"fixture"})).await.unwrap();
    let pid = wait_for_pid_file(&pid_file);
    assert!(alive(pid), "descendant should be running before the worker ends");
    match end {
        End::Stop => worker.stop().await,
        End::Drop => drop(worker),
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let survived = alive(pid);
    if survived {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    assert!(!survived, "descendant {pid} survived the worker");
}

const SLEEPER: &str = "import time; time.sleep(300)";

fn plain_child() -> String {
    format!("    p = subprocess.Popen([sys.executable, '-c', {SLEEPER:?}])\n    open(pid_file, 'w').write(str(p.pid))")
}

fn setsid_child() -> String {
    format!("    p = subprocess.Popen([sys.executable, '-c', {SLEEPER:?}], start_new_session=True)\n    open(pid_file, 'w').write(str(p.pid))")
}

// The intermediate parent exits at once, so the daemon is re-parented to init.
fn daemon() -> String {
    "    if os.fork() == 0:\n        os.setsid()\n        if os.fork() == 0:\n            open(pid_file, 'w').write(str(os.getpid()))\n            time.sleep(300)\n        os._exit(0)\n    time.sleep(10)".to_string()
}

#[tokio::test]
async fn stop_kills_grandchild_in_same_process_group() {
    assert_gone(&plain_child(), End::Stop).await;
}

#[tokio::test]
async fn stop_kills_grandchild_that_called_setsid() {
    assert_gone(&setsid_child(), End::Stop).await;
}

// Known limit of the tree walk (see reap.rs): a daemon already re-parented to init has no parent
// link. MacSandbox denies fork and bwrap/cgroup cover Linux, so only `Isolation::None` is exposed.
#[tokio::test]
#[ignore = "documented limit: orphaned double-fork daemons need cgroup or the macOS sandbox"]
async fn stop_kills_double_forked_daemon() {
    assert_gone(&daemon(), End::Stop).await;
}

#[tokio::test]
async fn drop_kills_grandchild_that_called_setsid() {
    assert_gone(&setsid_child(), End::Drop).await;
}

fn marker_processes(marker: &str) -> Vec<String> {
    let output = std::process::Command::new("ps").args(["-A", "-o", "pid=,stat=,command="]).output().unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains(marker) && !line.contains("ps -A"))
        .filter(|line| !line.split_whitespace().nth(1).is_some_and(|stat| stat.starts_with('Z')))
        .map(str::to_owned)
        .collect()
}

/// Inside Bubblewrap the extension directory is read-only, so the grandchild is found by a unique
/// command-line marker instead of a pid file. `--unshare-pid` plus `--die-with-parent` must remove it.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn bwrap_stop_leaves_no_setsid_grandchild() {
    if std::env::var("AIP_TEST_BWRAP").as_deref() != Ok("1") {
        return;
    }
    let dir = Dir::new();
    let marker = format!("aip-cleanup-marker-{}", std::process::id());
    let source = format!(
        "import subprocess, sys\ndef run(_input, _ctx):\n    subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(300)  # {marker}'], start_new_session=True)\n    return {{}}\n"
    );
    std::fs::write(dir.0.join("probe.py"), source).unwrap();
    let config = WorkerLaunchConfig { isolation: Isolation::LinuxBubblewrap, ..WorkerLaunchConfig::default() };
    let mut worker = Worker::try_start_configured(Lang::Python, &dir.0.to_string_lossy(), config).await.expect("worker starts");
    worker.send(&json!({"type":"invoke","invoke":1,"impl":"probe.run","input":{},"token":"fixture"})).await.unwrap();
    let reply = worker.read_line().await.unwrap().expect("worker answers");
    assert!(reply.contains("done"), "unexpected reply {reply}");
    assert!(!marker_processes(&marker).is_empty(), "grandchild should exist before stop");
    worker.stop().await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker_processes(&marker).is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(marker_processes(&marker), Vec::<String>::new(), "grandchild survived stop under bwrap");
}
