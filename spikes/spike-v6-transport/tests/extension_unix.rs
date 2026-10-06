#![cfg(target_os = "macos")]

use serde_json::{json, Value};
use spike_v1_fixture::{load_str, Form};
use spike_v2_read::{connect_with_url, id_wire::IdWire, plan::Caller};
use spike_v4_worker::{invoke_with_wire, Isolation, Lang, Worker, WorkerLimits};
use spike_v5_sdk::contract_fingerprint_with_extensions;
use spike_v6_transport::server::{listen_with_extensions, ReadExtensions, WorkerLang};
use std::{
    fs, io,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpStream, UnixListener},
    time::timeout,
};

const FIXTURE: &str = include_str!("../../spike-v1-fixture/fixture/recruitment.aip");
const OLD_STATS: &str = "  extension read stats {\n    input { clubId: Club.Id }\n    output { approvedApplicants: Int }\n    access Apply.approvedCount\n    effect none\n    deadline 2s\n    implementation \"recruitment.stats\"\n  }";
const NOW: &str = "2026-10-04T00:00:00Z";
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct OwnedTempDir(PathBuf);

impl Drop for OwnedTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn owned_temp_dir() -> io::Result<OwnedTempDir> {
    let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("aip-unix-probe-{}-{id}", std::process::id()));
    fs::create_dir(&path)?;
    Ok(OwnedTempDir(path))
}

fn facts() -> Value {
    let replacement = "  extension read stats {\n    input { clubId: Club.Id }\n    output { approvedApplicants: Int }\n    access Apply.approvedCount\n    effect none\n    deadline 2s\n    implementation \"probe.unix\"\n  }";
    assert_eq!(FIXTURE.matches(OLD_STATS).count(), 1, "Recruitment.stats fixture changed");
    load_str(&FIXTURE.replacen(OLD_STATS, replacement, 1), Form::A)
        .unwrap_or_else(|error| panic!("invalid V1 Recruitment fixture: {error:?}"))
        .execution
}

fn socket_probe_sources(path: &Path) -> (String, String) {
    let socket_literal = serde_json::to_string(&path.to_string_lossy().as_ref()).expect("socket path JSON string");
    let node = format!(
        r#"import net from "node:net";
const socketPath = {socket_literal};
async function probe() {{
  return await new Promise((resolve) => {{
    let settled = false;
    const socket = net.createConnection(socketPath);
    const finish = (available) => {{
      if (settled) return;
      settled = true;
      socket.destroy();
      resolve(available ? 1 : 0);
    }};
    socket.once("connect", () => finish(true));
    socket.once("error", () => finish(false));
    socket.setTimeout(500, () => finish(false));
  }});
}}
export async function unix(_input, _ctx) {{
  return {{ approvedApplicants: await probe() }};
}}
"#
    );
    let python = format!(
        "import socket\n\nSOCKET_PATH = {socket_literal}\n\ndef probe():\n    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)\n    connection.settimeout(0.5)\n    try:\n        connection.connect(SOCKET_PATH)\n        return 1\n    except OSError:\n        return 0\n    finally:\n        connection.close()\n\nasync def unix(input, ctx):\n    return {{\"approvedApplicants\": probe()}}\n"
    );
    (node, python)
}

fn worker_lang(lang: Lang) -> WorkerLang {
    match lang {
        Lang::Node => WorkerLang::Node,
        Lang::Python => WorkerLang::Python,
    }
}

async fn post_extension(address: SocketAddr, token: &str, fingerprint: &str) -> Result<Value, String> {
    let mut socket = TcpStream::connect(address).await.map_err(|error| format!("connect: {:?}", error.kind()))?;
    let body = json!({ "extension": "Recruitment.stats", "input": { "clubId": 10 } }).to_string();
    let request = format!(
        "POST /extension HTTP/1.1\r\nhost: 127.0.0.1\r\nauthorization: Bearer {token}\r\nx-aip-contract: {fingerprint}\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(request.as_bytes()).await.map_err(|error| format!("write: {:?}", error.kind()))?;
    let mut response = Vec::new();
    timeout(Duration::from_secs(6), socket.read_to_end(&mut response))
        .await
        .map_err(|_| "HTTP response exceeded six seconds".to_string())?
        .map_err(|error| format!("read: {:?}", error.kind()))?;
    let response = String::from_utf8(response).map_err(|_| "HTTP response is not UTF-8".to_string())?;
    let (_, body) = response.split_once("\r\n\r\n").ok_or_else(|| format!("HTTP response has no body: {response}"))?;
    serde_json::from_str(body).map_err(|error| format!("invalid JSON response: {error}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mac_net_deny_blocks_a_live_unix_socket_for_node_and_python() {
    let facts = facts();
    let mut failures = Vec::new();
    let db_url = spike_v2_read::DB_URL;
    let mut db = match connect_with_url(db_url).await {
        Ok(db) => db,
        Err(error) => panic!("local PostgreSQL connection failed: {error}"),
    };
    let temp_dir = match owned_temp_dir() {
        Ok(dir) => dir,
        Err(error) => panic!("temporary directory creation failed: {:?}", error.kind()),
    };
    let socket_path = temp_dir.0.join("probe.sock");
    let unix_listener = match UnixListener::bind(&socket_path) {
        Ok(listener) => listener,
        Err(error) => panic!("Unix listener bind failed: {:?}", error.kind()),
    };
    let (node_source, python_source) = socket_probe_sources(&socket_path);
    if let Err(error) = fs::write(temp_dir.0.join("probe.mjs"), node_source) {
        failures.push(format!("Node probe fixture write failed: {:?}", error.kind()));
    }
    if let Err(error) = fs::write(temp_dir.0.join("probe.py"), python_source) {
        failures.push(format!("Python probe fixture write failed: {:?}", error.kind()));
    }
    let caller = Caller { actor_id: None, now: NOW.into() };
    let input = json!({ "clubId": 10 });
    let fingerprint = contract_fingerprint_with_extensions(&facts, IdWire::SafeNumber);

    for lang in [Lang::Node, Lang::Python] {
        let mut baseline = match Worker::try_start_with(lang, temp_dir.0.to_string_lossy().as_ref(), Isolation::None, WorkerLimits::default()).await {
            Ok(worker) => worker,
            Err(error) => {
                failures.push(format!("{lang:?} unisolated worker startup failed: {:?}", error.kind()));
                continue;
            }
        };
        let baseline_result = timeout(
            Duration::from_secs(6),
            invoke_with_wire(&mut db, &mut baseline, &facts, "Recruitment.stats", &input, &caller, IdWire::SafeNumber),
        )
        .await;
        match baseline_result {
            Ok(Ok(output)) if output == json!({ "approvedApplicants": 1 }) => {}
            Ok(Ok(output)) => failures.push(format!("{lang:?} unisolated Unix socket positive baseline expected 1, got {output}")),
            Ok(Err(error)) => failures.push(format!("{lang:?} unisolated baseline failed: {}", error.code)),
            Err(_) => failures.push(format!("{lang:?} unisolated baseline exceeded six seconds")),
        }
        baseline.stop().await;

        let server = listen_with_extensions(
            facts.clone(),
            IdWire::SafeNumber,
            "127.0.0.1:0".parse().unwrap(),
            db_url.into(),
            Some(ReadExtensions { lang: worker_lang(lang), dir: temp_dir.0.clone() }),
        )
        .await;
        match server {
            Ok(server) => {
                let token = server.keys.issue(1, 60);
                match post_extension(server.address, &token, &fingerprint).await {
                    Ok(response) if response["output"] == json!({ "approvedApplicants": 0 }) => {}
                    Ok(response) => failures.push(format!("{lang:?} MacNetDeny Unix socket probe expected 0, got {response}")),
                    Err(error) => failures.push(format!("{lang:?} MacNetDeny Unix socket probe failed: {error}")),
                }
                if let Err(error) = server.shutdown().await {
                    failures.push(format!("{lang:?} configured server shutdown failed: {:?}", error.kind()));
                }
            }
            Err(error) => failures.push(format!("{lang:?} configured server startup failed: {:?}", error.kind())),
        }
    }

    drop(unix_listener);
    drop(temp_dir);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
