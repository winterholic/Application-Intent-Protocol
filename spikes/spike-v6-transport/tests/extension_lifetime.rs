#[cfg(target_os = "macos")]
mod macos {
    use serde_json::{json, Value};
    use spike_v1_fixture::{load_str, Form};
    use spike_v2_read::id_wire::IdWire;
    use spike_v5_sdk::contract_fingerprint_with_extensions;
    use spike_v6_transport::server::{listen_with_extensions, ReadExtensions, WorkerLang};
    use std::fs;
    use std::net::SocketAddr;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio::time::{sleep, timeout};

    const FIXTURE: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct OwnedDir(PathBuf);

    impl Drop for OwnedDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn unique_dir() -> std::io::Result<OwnedDir> {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("aip-extension-lifetime-{}-{id}", std::process::id()));
        fs::create_dir(&path)?;
        Ok(OwnedDir(path))
    }

    fn facts(deadline_ms: u64) -> Value {
        facts_with(deadline_ms, "observer.wait")
    }

    fn facts_with(deadline_ms: u64, implementation: &str) -> Value {
        let mut facts = load_str(FIXTURE, Form::A).unwrap_or_else(|error| panic!("Recruitment fixture invalid: {error:?}")).execution;
        facts["resources"]["Recruitment"]["extensions"]["stats"]["implementation"] = json!(implementation);
        facts["resources"]["Recruitment"]["extensions"]["stats"]["deadlineMs"] = json!(deadline_ms);
        facts
    }

    fn worker_source(lang: WorkerLang, pid_file: &Path) -> std::io::Result<(OwnedDir, ReadExtensions)> {
        let dir = unique_dir()?;
        let path = pid_file.to_string_lossy();
        // A JSON string literal is valid for both the JS and Python path literal used here.
        let quoted_path = serde_json::to_string(path.as_ref()).expect("serialize test-owned path");
        let (module, source) = match lang {
            WorkerLang::Node => (
                "observer.mjs",
                format!(
                    "import {{ writeFileSync }} from 'node:fs';\nexport async function wait() {{ writeFileSync({quoted_path}, String(process.pid)); while (true) {{}} }}\n"
                ),
            ),
            WorkerLang::Python => (
                "observer.py",
                format!(
                    "import os\nfrom pathlib import Path\nasync def wait(input, ctx):\n    Path({quoted_path}).write_text(str(os.getpid()))\n    while True: pass\n"
                ),
            ),
        };
        fs::write(dir.0.join(module), source)?;
        let config = ReadExtensions { lang, dir: dir.0.clone() };
        Ok((dir, config))
    }

    fn concurrent_worker_source(lang: WorkerLang, pid_log: &Path) -> std::io::Result<(OwnedDir, ReadExtensions)> {
        let dir = unique_dir()?;
        let path = pid_log.to_string_lossy();
        let quoted_path = serde_json::to_string(path.as_ref()).expect("serialize test-owned path");
        let (module, source) = match lang {
            WorkerLang::Node => (
                "observer.mjs",
                format!(
                    "import {{ appendFileSync }} from 'node:fs';\nexport async function drain() {{ appendFileSync({quoted_path}, String(process.pid) + '\\n'); await new Promise((resolve) => setTimeout(resolve, 400)); return {{ approvedApplicants: 0 }}; }}\n"
                ),
            ),
            WorkerLang::Python => (
                "observer.py",
                format!(
                    "import asyncio, os\nasync def drain(input, ctx):\n    with open({quoted_path}, 'a') as pid_log:\n        pid_log.write(str(os.getpid()) + '\\n')\n    await asyncio.sleep(0.4)\n    return {{\"approvedApplicants\": 0}}\n"
                ),
            ),
        };
        fs::write(dir.0.join(module), source)?;
        let config = ReadExtensions { lang, dir: dir.0.clone() };
        Ok((dir, config))
    }

    async fn start_server(deadline_ms: u64, config: ReadExtensions) -> Result<spike_v6_transport::server::Server, std::io::Error> {
        listen_with_extensions(
            facts(deadline_ms),
            IdWire::SafeNumber,
            "127.0.0.1:0".parse().expect("loopback address"),
            spike_v2_read::DB_URL.to_string(),
            Some(config),
        )
        .await
    }

    fn request(token: &str, fingerprint: &str) -> String {
        let body = json!({ "extension": "Recruitment.stats", "input": { "clubId": 10 } }).to_string();
        format!(
            "POST /extension HTTP/1.1\r\nhost: 127.0.0.1\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fingerprint}\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    async fn post(address: SocketAddr, token: &str, fingerprint: &str) -> Result<Value, String> {
        let mut socket = TcpStream::connect(address).await.map_err(|error| format!("connect: {:?}", error.kind()))?;
        socket.write_all(request(token, fingerprint).as_bytes()).await.map_err(|error| format!("write: {:?}", error.kind()))?;
        let mut response = Vec::new();
        timeout(Duration::from_secs(5), socket.read_to_end(&mut response))
            .await
            .map_err(|_| "HTTP response timed out".to_string())?
            .map_err(|error| format!("read: {:?}", error.kind()))?;
        response_json(response)
    }

    fn response_json(response: Vec<u8>) -> Result<Value, String> {
        let text = String::from_utf8(response).map_err(|_| "HTTP response is not UTF-8".to_string())?;
        let (_, body) = text.split_once("\r\n\r\n").ok_or_else(|| format!("HTTP response has no body: {text}"))?;
        serde_json::from_str(body).map_err(|error| format!("invalid JSON response: {error}"))
    }

    async fn spawn_pending(address: SocketAddr, token: String, fingerprint: String) -> tokio::task::JoinHandle<Result<Vec<u8>, std::io::Error>> {
        tokio::spawn(async move {
            let mut socket = TcpStream::connect(address).await?;
            socket.write_all(request(&token, &fingerprint).as_bytes()).await?;
            let mut response = Vec::new();
            socket.read_to_end(&mut response).await?;
            Ok(response)
        })
    }

    async fn wait_pid(path: &Path, limit: Duration) -> Result<i32, String> {
        timeout(limit, async {
            loop {
                if let Ok(text) = fs::read_to_string(path) {
                    if let Ok(pid) = text.trim().parse::<i32>() {
                        return pid;
                    }
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| "worker did not write its PID before timeout".into())
    }

    async fn wait_pids(path: &Path, expected: usize, limit: Duration) -> Result<Vec<i32>, String> {
        timeout(limit, async {
            loop {
                let mut pids =
                    fs::read_to_string(path).unwrap_or_default().lines().filter_map(|line| line.trim().parse::<i32>().ok()).collect::<Vec<_>>();
                pids.sort_unstable();
                pids.dedup();
                if pids.len() >= expected {
                    return pids;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| format!("worker PID log did not reach {expected} entries"))
    }

    fn process_alive(pid: i32) -> bool {
        Command::new("kill").args(["-0", &pid.to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|status| status.success())
    }

    async fn cleanup_child(pid: i32) -> bool {
        let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        wait_dead(pid).await
    }

    async fn wait_dead(pid: i32) -> bool {
        timeout(Duration::from_secs(1), async {
            loop {
                if !process_alive(pid) {
                    return true;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or(false)
    }

    async fn timeout_case(lang: WorkerLang, failures: &mut Vec<String>) {
        let dir = match unique_dir() {
            Ok(dir) => dir,
            Err(error) => {
                failures.push(format!("{lang:?} timeout temp dir: {:?}", error.kind()));
                return;
            }
        };
        let pid_file = dir.0.join("worker.pid");
        let (worker_dir, config) = match worker_source(lang, &pid_file) {
            Ok(pair) => pair,
            Err(error) => {
                failures.push(format!("{lang:?} timeout worker source: {:?}", error.kind()));
                return;
            }
        };
        let server = match start_server(100, config).await {
            Ok(server) => server,
            Err(error) => {
                failures.push(format!("{lang:?} timeout server startup: {:?}", error.kind()));
                return;
            }
        };
        let facts = facts(100);
        let fingerprint = contract_fingerprint_with_extensions(&facts, IdWire::SafeNumber);
        let token = server.keys.issue(1, 600);
        let result = post(server.address, &token, &fingerprint).await;
        match result {
            Ok(body) if body["code"] == "DEADLINE_EXCEEDED" => {}
            Ok(body) => failures.push(format!("{lang:?} deadline response: expected DEADLINE_EXCEEDED, got {body}")),
            Err(error) => failures.push(format!("{lang:?} deadline HTTP request failed: {error}")),
        }
        match wait_pid(&pid_file, Duration::from_secs(2)).await {
            Ok(pid) if wait_dead(pid).await => {}
            Ok(pid) => {
                failures.push(format!("{lang:?} deadline child {pid} survived for 1s"));
                if !cleanup_child(pid).await {
                    failures.push(format!("{lang:?} deadline child {pid} did not exit after test cleanup"));
                }
            }
            Err(error) => failures.push(format!("{lang:?} deadline child PID: {error}")),
        }
        if let Err(error) = server.shutdown().await {
            failures.push(format!("{lang:?} timeout server shutdown: {:?}", error.kind()));
        }
        drop(worker_dir);
        drop(dir);
    }

    async fn shutdown_case(lang: WorkerLang, failures: &mut Vec<String>) {
        let dir = match unique_dir() {
            Ok(dir) => dir,
            Err(error) => {
                failures.push(format!("{lang:?} shutdown temp dir: {:?}", error.kind()));
                return;
            }
        };
        let pid_file = dir.0.join("worker.pid");
        let (worker_dir, config) = match worker_source(lang, &pid_file) {
            Ok(pair) => pair,
            Err(error) => {
                failures.push(format!("{lang:?} shutdown worker source: {:?}", error.kind()));
                return;
            }
        };
        let server = match start_server(5000, config).await {
            Ok(server) => server,
            Err(error) => {
                failures.push(format!("{lang:?} shutdown server startup: {:?}", error.kind()));
                return;
            }
        };
        let facts = facts(5000);
        let fingerprint = contract_fingerprint_with_extensions(&facts, IdWire::SafeNumber);
        let token = server.keys.issue(1, 600);
        let pending = spawn_pending(server.address, token, fingerprint).await;
        let pid_result = wait_pid(&pid_file, Duration::from_secs(5)).await;
        match pid_result {
            Ok(pid) => {
                match timeout(Duration::from_secs(1), server.shutdown()).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => failures.push(format!("{lang:?} server shutdown failed: {:?}", error.kind())),
                    Err(_) => failures.push(format!("{lang:?} server shutdown exceeded 1s")),
                }
                if !wait_dead(pid).await {
                    failures.push(format!("{lang:?} shutdown child {pid} survived for 1s"));
                    if !cleanup_child(pid).await {
                        failures.push(format!("{lang:?} shutdown child {pid} did not exit after test cleanup"));
                    }
                }
            }
            Err(error) => {
                failures.push(format!("{lang:?} shutdown child PID: {error}"));
                if let Err(error) = server.shutdown().await {
                    failures.push(format!("{lang:?} cleanup shutdown failed: {:?}", error.kind()));
                }
            }
        }

        match timeout(Duration::from_secs(1), pending).await {
            Ok(Ok(Ok(response))) if !String::from_utf8_lossy(&response).contains("\"ok\":true") => {}
            Ok(Ok(Ok(response))) => failures.push(format!("{lang:?} shutdown returned successful HTTP: {}", String::from_utf8_lossy(&response))),
            Ok(Ok(Err(error))) => {
                if !matches!(error.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof) {
                    failures.push(format!("{lang:?} shutdown request error: {:?}", error.kind()));
                }
            }
            Ok(Err(error)) => failures.push(format!("{lang:?} pending request task failed: {error}")),
            Err(_) => failures.push(format!("{lang:?} pending HTTP request did not close within 1s")),
        }
        drop(worker_dir);
        drop(dir);
    }

    async fn concurrency_case(lang: WorkerLang, failures: &mut Vec<String>) {
        let dir = match unique_dir() {
            Ok(dir) => dir,
            Err(error) => {
                failures.push(format!("{lang:?} concurrency temp dir: {:?}", error.kind()));
                return;
            }
        };
        let pid_log = dir.0.join("workers.pid");
        let (worker_dir, config) = match concurrent_worker_source(lang, &pid_log) {
            Ok(pair) => pair,
            Err(error) => {
                failures.push(format!("{lang:?} concurrency worker source: {:?}", error.kind()));
                return;
            }
        };
        let server = match listen_with_extensions(
            facts_with(2000, "observer.drain"),
            IdWire::SafeNumber,
            "127.0.0.1:0".parse().expect("loopback address"),
            spike_v2_read::DB_URL.to_string(),
            Some(config),
        )
        .await
        {
            Ok(server) => server,
            Err(error) => {
                failures.push(format!("{lang:?} concurrency server startup: {:?}", error.kind()));
                return;
            }
        };
        let facts = facts_with(2000, "observer.drain");
        let fingerprint = contract_fingerprint_with_extensions(&facts, IdWire::SafeNumber);
        let token = server.keys.issue(1, 600);
        let mut requests = Vec::with_capacity(4);
        for _ in 0..4 {
            requests.push(spawn_pending(server.address, token.clone(), fingerprint.clone()).await);
        }

        let pids = match wait_pids(&pid_log, 4, Duration::from_secs(5)).await {
            Ok(pids) => pids,
            Err(error) => {
                failures.push(format!("{lang:?} four concurrent workers: {error}"));
                Vec::new()
            }
        };
        if pids.len() >= 4 {
            let alive = pids.iter().filter(|pid| process_alive(**pid)).count();
            if alive < 4 {
                failures.push(format!("{lang:?} expected four live children, observed {alive}"));
            }
            match post(server.address, &token, &fingerprint).await {
                Ok(body) if body["code"] == "WORKER_BUSY" => {}
                Ok(body) => failures.push(format!("{lang:?} fifth concurrent call: expected WORKER_BUSY, got {body}")),
                Err(error) => failures.push(format!("{lang:?} fifth concurrent call failed: {error}")),
            }
        }

        for request in requests {
            match timeout(Duration::from_secs(5), request).await {
                Ok(Ok(Ok(response))) => match response_json(response) {
                    Ok(body) if body["ok"] == true && body["output"] == json!({ "approvedApplicants": 0 }) => {}
                    Ok(body) => failures.push(format!("{lang:?} concurrent worker response: {body}")),
                    Err(error) => failures.push(format!("{lang:?} concurrent worker response parse: {error}")),
                },
                Ok(Ok(Err(error))) => failures.push(format!("{lang:?} concurrent worker HTTP error: {:?}", error.kind())),
                Ok(Err(error)) => failures.push(format!("{lang:?} concurrent request task failed: {error}")),
                Err(_) => failures.push(format!("{lang:?} concurrent request did not finish within 5s")),
            }
        }

        match post(server.address, &token, &fingerprint).await {
            Ok(body) if body["ok"] == true && body["output"] == json!({ "approvedApplicants": 0 }) => {}
            Ok(body) => failures.push(format!("{lang:?} call after worker slot release: {body}")),
            Err(error) => failures.push(format!("{lang:?} call after worker slot release failed: {error}")),
        }
        let all_pids = match wait_pids(&pid_log, 5, Duration::from_secs(2)).await {
            Ok(pids) => pids,
            Err(error) => {
                failures.push(format!("{lang:?} post-release worker PID: {error}"));
                pids
            }
        };
        if let Err(error) = server.shutdown().await {
            failures.push(format!("{lang:?} concurrency server shutdown: {:?}", error.kind()));
        }
        for pid in all_pids {
            if !wait_dead(pid).await {
                failures.push(format!("{lang:?} worker child {pid} survived server shutdown"));
                if !cleanup_child(pid).await {
                    failures.push(format!("{lang:?} worker child {pid} did not exit after test cleanup"));
                }
            }
        }
        drop(worker_dir);
        drop(dir);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn extension_timeout_and_server_shutdown_reap_real_worker_children() {
        let mut failures = Vec::new();
        for lang in [WorkerLang::Node, WorkerLang::Python] {
            timeout_case(lang, &mut failures).await;
            shutdown_case(lang, &mut failures).await;
            concurrency_case(lang, &mut failures).await;
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
