#[cfg(unix)]
mod worker_bootstrap {
    use spike_v4_worker::{Isolation, Lang, Worker, WorkerLimits};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command as TokioCommand;
    use tokio::time::{sleep, timeout};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct OwnedDir(PathBuf);

    impl Drop for OwnedDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir() -> OwnedDir {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("aip-worker-bootstrap-{}-{id}", std::process::id()));
        fs::create_dir(&path).expect("create fresh owned temporary directory");
        OwnedDir(path)
    }

    fn shell_quote(value: &Path) -> String {
        format!("'{}'", value.to_string_lossy().replace('\'', "'\\''"))
    }

    fn fake_runtime(dir: &Path, command_name: &str, log: &Path) {
        let path = dir.join(command_name);
        // Capture completion needs a marker: the argv file exists before printf finishes.
        let source = format!(
            "#!/bin/sh\nexec > {}\nfor arg do\n  printf '%s\\0' \"$arg\"\n  /bin/sleep 0.02\ndone\n/usr/bin/touch {}\nexec /bin/cat >/dev/null\n",
            shell_quote(log),
            shell_quote(&log.with_extension("ready"))
        );
        fs::write(&path, source).expect("write fake runtime command");
        let mut permissions = fs::metadata(&path).expect("stat fake runtime").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("make fake runtime executable");
    }

    #[tokio::test]
    async fn capture_worker_args_child_entry() {
        let Ok(log) = std::env::var("AIP_BOOTSTRAP_CAPTURE_LOG") else {
            return;
        };
        let Ok(extension_dir) = std::env::var("AIP_BOOTSTRAP_EXTENSION_DIR") else {
            panic!("child harness missing extension directory");
        };
        let lang = match std::env::var("AIP_BOOTSTRAP_CAPTURE_LANG").as_deref() {
            Ok("node") => Lang::Node,
            Ok("python") => Lang::Python,
            other => panic!("unknown child harness language: {other:?}"),
        };
        let worker = Worker::try_start_with(lang, &extension_dir, Isolation::None, WorkerLimits::default())
            .await
            .expect("launch through captured runtime command");
        timeout(Duration::from_secs(2), async {
            while !Path::new(&log).with_extension("ready").is_file() {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("fake runtime finishes recording all argv");
        worker.stop().await;
    }

    fn captured_args(language: &str, command_name: &str) -> (OwnedDir, Vec<String>, String) {
        let dir = temp_dir();
        let bin = dir.0.join("bin");
        let extensions = dir.0.join("extensions");
        fs::create_dir_all(&bin).expect("create fake runtime bin directory");
        fs::create_dir(&extensions).expect("create extension directory");
        let log = dir.0.join("args.bin");
        fake_runtime(&bin, command_name, &log);
        let existing_path = std::env::var("PATH").unwrap_or_default();
        let path = format!("{}:{existing_path}", bin.display());
        let output = Command::new(std::env::current_exe().expect("test harness executable"))
            .args(["--exact", "worker_bootstrap::capture_worker_args_child_entry", "--nocapture"])
            .env("AIP_BOOTSTRAP_CAPTURE_LOG", &log)
            .env("AIP_BOOTSTRAP_EXTENSION_DIR", &extensions)
            .env("AIP_BOOTSTRAP_CAPTURE_LANG", language)
            .env("PATH", path)
            .output()
            .expect("run isolated test harness child");
        assert!(output.status.success(), "capture child failed: {}", String::from_utf8_lossy(&output.stderr));
        let bytes = fs::read(&log).expect("read captured runtime argv");
        let args = bytes
            .split(|byte| *byte == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8(arg.to_vec()).expect("captured argv is UTF-8"))
            .collect();
        (dir, args, extensions.to_string_lossy().into_owned())
    }

    #[test]
    fn node_bootstrap_is_embedded_and_takes_extension_directory_as_an_argument() {
        let (_owned, args, extension_dir) = captured_args("node", "node");
        assert_eq!(args.first().map(String::as_str), Some("--input-type=module"));
        assert_eq!(args.get(1).map(String::as_str), Some("-e"));
        assert_eq!(args.get(2).map(String::as_str), Some(include_str!("../workers/worker.mjs")));
        assert_eq!(args.get(3).map(String::as_str), Some("--"));
        assert_eq!(args.last().map(String::as_str), Some(extension_dir.as_str()));
        assert!(args.iter().all(|arg| !arg.ends_with("/workers/worker.mjs")));
        assert!(include_str!("../workers/worker.mjs").contains("process.argv[1]"));
    }

    #[test]
    fn python_bootstrap_is_embedded_and_takes_extension_directory_as_an_argument() {
        let (_owned, args, extension_dir) = captured_args("python", "python3");
        assert_eq!(args.first().map(String::as_str), Some("-P"));
        assert_eq!(args.get(1).map(String::as_str), Some("-c"));
        let captured_source = args.get(2).expect("Python -c source argument");
        let expected_source = include_str!("../workers/worker.py");
        assert!(
            captured_source == expected_source,
            "captured Python source differs (actual {} bytes, expected {} bytes; actual tail {:?}; expected tail {:?})",
            captured_source.len(),
            expected_source.len(),
            captured_source.as_bytes().get(captured_source.len().saturating_sub(12)..),
            expected_source.as_bytes().get(expected_source.len().saturating_sub(12)..)
        );
        assert_eq!(args.last().map(String::as_str), Some(extension_dir.as_str()));
        assert!(args.iter().all(|arg| !arg.ends_with("/workers/worker.py")));
        assert!(include_str!("../workers/worker.py").contains("sys.argv[1]"));
    }

    #[tokio::test]
    async fn python_bootstrap_imports_are_not_shadowed_by_cwd_files() {
        for shadow_module in ["asyncio", "json"] {
            let dir = temp_dir();
            let cwd = dir.0.join("cwd");
            let extensions = dir.0.join("extensions");
            fs::create_dir(&cwd).expect("create isolated worker cwd");
            fs::create_dir(&extensions).expect("create extension directory");
            fs::write(cwd.join(format!("{shadow_module}.py")), "raise RuntimeError('cwd shadow loaded')\n")
                .expect("create hostile cwd shadow module");
            fs::write(extensions.join("probe.py"), "async def run(input, ctx):\n    return {'approvedApplicants': 0}\n")
                .expect("create extension module");

            let mut child = TokioCommand::new("python3")
                .args(["-P", "-c", include_str!("../workers/worker.py")])
                .arg(&extensions)
                .current_dir(&cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .expect("launch embedded Python worker with safe-path mode");
            let input = serde_json::json!({"type":"invoke","invoke":1,"impl":"probe.run","input":{},"token":"test"}).to_string() + "\n";
            child.stdin.take().expect("worker stdin").write_all(input.as_bytes()).await.expect("send invocation");
            let output = timeout(Duration::from_secs(3), child.wait_with_output())
                .await
                .expect("worker should exit after stdin EOF")
                .expect("collect worker output");
            assert!(output.status.success(), "Python worker failed with cwd {shadow_module}.py: {}", String::from_utf8_lossy(&output.stderr));
            let line = String::from_utf8(output.stdout).expect("worker stdout is UTF-8");
            let response: serde_json::Value = serde_json::from_str(line.trim()).expect("worker returned protocol JSON");
            assert_eq!(response["type"], "done");
            assert_eq!(response["output"], serde_json::json!({"approvedApplicants":0}));
        }
    }
}
