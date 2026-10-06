use serde_json::{json, Value};
use spike_v2_read::id_wire::IdWire;
use spike_v6_transport::server::{listen_with_apply_contract, Server};
use std::{io::ErrorKind, net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{sleep, timeout},
};

const BAD_DB: &str = "host=127.0.0.1 port=1 dbname=postgres connect_timeout=1";
const READ_DEADLINE: Duration = Duration::from_secs(6);
const HTTP: &str = "HTTP/1.1";

async fn server() -> Result<Server, String> {
    listen_with_apply_contract(json!({}), IdWire::SafeNumber, "127.0.0.1:0".parse().unwrap(), BAD_DB.into())
        .await
        .map_err(|error| format!("server startup: {:?}", error.kind()))
}

async fn connect_and_write(address: SocketAddr, bytes: &[u8]) -> Result<TcpStream, String> {
    let mut socket = TcpStream::connect(address).await.map_err(|error| format!("connect: {:?}", error.kind()))?;
    socket.write_all(bytes).await.map_err(|error| format!("write: {:?}", error.kind()))?;
    Ok(socket)
}

async fn read_json(socket: &mut TcpStream, deadline: Duration) -> Result<Value, String> {
    let mut response = Vec::new();
    timeout(deadline, socket.read_to_end(&mut response))
        .await
        .map_err(|_| format!("connection stayed open past {deadline:?}"))?
        .map_err(|error| format!("read: {:?}", error.kind()))?;
    let response = String::from_utf8(response).map_err(|_| "response is not UTF-8".to_string())?;
    let (_, body) = response.split_once("\r\n\r\n").ok_or_else(|| format!("HTTP response has no body: {response}"))?;
    serde_json::from_str(body).map_err(|error| format!("invalid JSON response: {error}"))
}

async fn read_session(address: SocketAddr, deadline: Duration) -> Result<Value, String> {
    let body = "{}";
    let request = format!("POST /session {HTTP}\r\nhost: 127.0.0.1\r\ncontent-length: {}\r\n\r\n{body}", body.len());
    let mut socket =
        timeout(deadline, connect_and_write(address, request.as_bytes())).await.map_err(|_| "session connect/write timeout".to_string())??;
    read_json(&mut socket, deadline).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn partial_headers_body_and_dripped_headers_share_one_five_second_deadline() {
    let server = match server().await {
        Ok(server) => server,
        Err(error) => panic!("{error}"),
    };
    let address = server.address;
    let mut failures = Vec::new();

    let partial_header = b"POST /session HTTP/1.1\r\n";
    match connect_and_write(address, partial_header).await {
        Ok(mut socket) => match read_json(&mut socket, READ_DEADLINE).await {
            Ok(response) if response["code"] == "REQUEST_TIMEOUT" => {}
            Ok(response) => failures.push(format!("partial header: expected REQUEST_TIMEOUT, got {response}")),
            Err(error) => failures.push(format!("partial header: {error}")),
        },
        Err(error) => failures.push(format!("partial header: {error}")),
    }

    let partial_body = b"POST /session HTTP/1.1\r\nhost: 127.0.0.1\r\ncontent-length: 1048576\r\n\r\n";
    match connect_and_write(address, partial_body).await {
        Ok(mut socket) => match read_json(&mut socket, READ_DEADLINE).await {
            Ok(response) if response["code"] == "REQUEST_TIMEOUT" => {}
            Ok(response) => failures.push(format!("partial 1 MiB body: expected REQUEST_TIMEOUT, got {response}")),
            Err(error) => failures.push(format!("partial 1 MiB body: {error}")),
        },
        Err(error) => failures.push(format!("partial 1 MiB body: {error}")),
    }

    let drip_start = tokio::time::Instant::now();
    match connect_and_write(address, b"POST /session HTTP/1.1\r\n").await {
        Ok(mut socket) => {
            sleep(Duration::from_secs(3)).await;
            if let Err(error) = socket.write_all(b"host: 127.0.0.1\r\n").await {
                failures.push(format!("dripped header write: {:?}", error.kind()));
            } else {
                let remaining = READ_DEADLINE.saturating_sub(drip_start.elapsed());
                match read_json(&mut socket, remaining).await {
                    Ok(response) if response["code"] == "REQUEST_TIMEOUT" => {}
                    Ok(response) => failures.push(format!("dripped header: expected REQUEST_TIMEOUT, got {response}")),
                    Err(error) => failures.push(format!("dripped header reset the absolute deadline: {error}")),
                }
            }
        }
        Err(error) => failures.push(format!("dripped header: {error}")),
    }

    if let Err(error) = server.shutdown().await {
        failures.push(format!("server shutdown: {:?}", error.kind()));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sixty_fifth_incomplete_connection_is_rejected_and_a_freed_slot_serves_session() {
    let server = match server().await {
        Ok(server) => server,
        Err(error) => panic!("{error}"),
    };
    let address = server.address;
    let partial = b"POST /session HTTP/1.1\r\n";
    let mut failures = Vec::new();
    let mut held = Vec::with_capacity(64);

    for index in 0..64 {
        match connect_and_write(address, partial).await {
            Ok(socket) => held.push(socket),
            Err(error) => failures.push(format!("partial connection {index}: {error}")),
        }
    }
    // Let the accept loop dispatch the connected partial requests before probing the next slot.
    sleep(Duration::from_millis(100)).await;

    match connect_and_write(address, partial).await {
        Ok(mut overflow) => {
            let mut response = Vec::new();
            match timeout(Duration::from_secs(1), overflow.read_to_end(&mut response)).await {
                Ok(Ok(_)) if response.is_empty() => {}
                Ok(Ok(_)) => failures.push(format!("65th connection returned unexpected bytes: {} bytes", response.len())),
                Ok(Err(error)) if error.kind() == ErrorKind::ConnectionReset => {}
                Ok(Err(error)) => failures.push(format!("65th connection read failed: {:?}", error.kind())),
                Err(_) => failures.push("65th incomplete connection remained open for more than one second".into()),
            }
        }
        Err(error) => failures.push(format!("65th connection: {error}")),
    }

    drop(held.pop());
    let mut session = None;
    for _ in 0..20 {
        match read_session(address, Duration::from_millis(300)).await {
            Ok(response) if response["ok"] == true => {
                session = Some(response);
                break;
            }
            Ok(response) if response["code"] == "WORKER_BUSY" => sleep(Duration::from_millis(25)).await,
            Ok(response) => {
                failures.push(format!("session after slot release: expected ok, got {response}"));
                break;
            }
            Err(_) => sleep(Duration::from_millis(25)).await,
        }
    }
    if session.is_none() {
        failures.push("session did not become available after closing one partial connection".into());
    }

    drop(held);
    if let Err(error) = server.shutdown().await {
        failures.push(format!("server shutdown: {:?}", error.kind()));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_post_route_returns_not_found_before_database_connection() {
    let server = match server().await {
        Ok(server) => server,
        Err(error) => panic!("{error}"),
    };
    let body = "{}";
    let request = format!("POST /unknown {HTTP}\r\nhost: 127.0.0.1\r\ncontent-length: {}\r\n\r\n{body}", body.len());
    let mut failures = Vec::new();
    match connect_and_write(server.address, request.as_bytes()).await {
        Ok(mut socket) => match read_json(&mut socket, Duration::from_secs(2)).await {
            Ok(response) if response["code"] == "NOT_FOUND" => {}
            Ok(response) => failures.push(format!("unknown POST route: expected NOT_FOUND, got {response}")),
            Err(error) => failures.push(format!("unknown POST route: {error}")),
        },
        Err(error) => failures.push(format!("unknown POST route: {error}")),
    }
    if let Err(error) = server.shutdown().await {
        failures.push(format!("server shutdown: {:?}", error.kind()));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
