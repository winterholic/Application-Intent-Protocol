use std::{
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinSet,
    time::{sleep, timeout},
};
// Authentication is relayed to the real local PostgreSQL. Only post-auth SQL is held.
async fn hold_sql(socket: TcpStream, held: Arc<AtomicUsize>, closed: Arc<AtomicUsize>) {
    let db = TcpStream::connect("127.0.0.1:5432").await.unwrap();
    let (mut input, mut output) = socket.into_split();
    let (mut db_input, mut db_output) = db.into_split();
    let ready = Arc::new(AtomicBool::new(false));
    let ready_out = ready.clone();
    let backend = async move {
        loop {
            let kind = db_input.read_u8().await?;
            let length = db_input.read_u32().await?;
            if !(4..=1_048_576).contains(&length) {
                return Err(std::io::Error::other("invalid PostgreSQL packet"));
            }
            let mut bytes = vec![0; (length - 4) as usize];
            db_input.read_exact(&mut bytes).await?;
            if kind == b'Z' {
                ready_out.store(true, Ordering::SeqCst);
            }
            output.write_u8(kind).await?;
            output.write_u32(length).await?;
            output.write_all(&bytes).await?;
        }
        #[allow(unreachable_code)]
        Ok::<(), std::io::Error>(())
    };
    let frontend = async move {
        let mut seen = false;
        let mut bytes = [0; 8192];
        loop {
            let n = input.read(&mut bytes).await?;
            if n == 0 {
                closed.fetch_add(1, Ordering::SeqCst);
                return Ok::<(), std::io::Error>(());
            }
            if ready.load(Ordering::SeqCst) {
                if !seen {
                    held.fetch_add(1, Ordering::SeqCst);
                    seen = true;
                }
            } else {
                db_output.write_all(&bytes[..n]).await?;
            }
        }
    };
    tokio::select! { _=frontend=>{}, _=backend=>{} }
}

async fn reaches(value: &AtomicUsize, count: usize) -> bool {
    timeout(Duration::from_secs(2), async {
        while value.load(Ordering::SeqCst) < count {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok()
}

#[tokio::test]
async fn cli_sql_startup_wait_is_bounded_after_real_database_authentication() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let held = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(AtomicUsize::new(0));
    let (h, c) = (held.clone(), closed.clone());
    let proxy = tokio::spawn(async move {
        let mut peers = JoinSet::new();
        loop {
            tokio::select! {
                socket=listener.accept()=>{let(socket,_)=socket.unwrap();peers.spawn(hold_sql(socket,h.clone(),c.clone()));}
                _=peers.join_next(),if !peers.is_empty()=>{}
            }
        }
    });
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("example/app.aip");
    let schema = format!("aip_startup_p{}", std::process::id());
    let db_url = format!("host=127.0.0.1 port={} dbname=postgres", address.port());
    let mut failures = vec![];
    for (index, (operation, expected)) in [("init", "INIT_UNSETTLED"), ("serve", "DB_PREFLIGHT")].into_iter().enumerate() {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aip-prototype"));
        command.arg(operation).arg(&source).args(["--schema", &schema, "--db-url", &db_url]);
        if operation == "serve" {
            command.args(["--wire", "decimal", "--listen", "127.0.0.1:0"]);
        }
        let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        if !reaches(&held, index + 1).await {
            failures.push(format!("{operation} did not authenticate and send SQL"));
        }
        let exited = timeout(Duration::from_secs(6), async {
            loop {
                if child.try_wait().unwrap().is_some() {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();
        if !exited {
            let _ = child.kill();
            failures.push(format!("{operation} SQL stage remained blocked"));
        }
        let output = child.wait_with_output().unwrap();
        if exited {
            let result = serde_json::from_slice::<serde_json::Value>(&output.stderr);
            if !matches!(&result,Ok(v) if v["code"]==expected) {
                failures.push(format!("{operation}: {result:?}"));
            }
            if output.status.success() || !output.stdout.is_empty() {
                failures.push(format!("{operation} unexpectedly initialized or started"));
            }
        }
        if !reaches(&closed, index + 1).await {
            failures.push(format!("{operation} left a DB socket open"));
        }
    }
    proxy.abort();
    let _ = proxy.await;
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
