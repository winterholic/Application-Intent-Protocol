//! 테스트 DB 앞에 두는 TCP proxy. 지정한 지점에서 서버 응답을 막고 연결을 끊거나 응답을 바꾼다.
//! 장애는 한 번만 일으키고, 이후 연결(같은 키 재시도 등)은 그대로 통과시킨다.
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, oneshot},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// 서버가 COMMIT 성공 태그를 보낸 뒤, 응답을 막고 FIN으로 닫는다.
    AckDropFin,
    /// 같은 시점에 RST로 끊는다.
    AckDropReset,
    /// 문장에 이 문자열이 든 요청의 CommandComplete를 서버가 보낸 직후 응답을 막고 닫는다. COMMIT은 아직 보내지지 않았다.
    CutAfterStatement(&'static str),
    /// 클라이언트가 COMMIT을 보내면 서버에 전달하지 않고 닫는다.
    CutOnCommitSent,
    /// 서버의 COMMIT 성공 태그를 주어진 태그의 빈 메시지로 바꿔 전달한다.
    ReplaceCommitAck(u8),
}

enum Frontend {
    Commit,
    Statement,
}

struct Armed {
    fault: Fault,
    /// 지정하면 COMMIT 단계 장애는 이 문자열이 든 문장을 보낸 연결에서만 일어난다. 배경 작업의 COMMIT이 장애를 먼저 쓰지 못하게 한다.
    after_statement: Option<&'static str>,
    armed: AtomicBool,
    fired: tokio::sync::Mutex<Option<oneshot::Sender<String>>>,
}

impl Armed {
    fn take(&self) -> bool {
        self.armed.swap(false, Ordering::SeqCst)
    }

    async fn fire(&self, phase: &str) {
        if let Some(sender) = self.fired.lock().await.take() {
            let _ = sender.send(phase.to_owned());
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|window| window == needle)
}

/// 클라이언트가 보낸 바이트에서 완결된 메시지를 읽어 COMMIT과 장애 지점 문장을 찾는다.
fn scan_frontend(pending: &mut Vec<u8>, startup: &mut bool, needle: Option<&str>) -> Vec<Frontend> {
    let mut events = Vec::new();
    let mut offset = 0;
    while pending.len().saturating_sub(offset) >= if *startup { 5 } else { 4 } {
        if !*startup {
            let len = u32::from_be_bytes(pending[offset..offset + 4].try_into().unwrap()) as usize;
            if len < 8 || pending.len() - offset < len {
                break;
            }
            offset += len;
            *startup = true;
            continue;
        }
        let len = u32::from_be_bytes(pending[offset + 1..offset + 5].try_into().unwrap()) as usize;
        if len < 4 || pending.len() - offset < len + 1 {
            break;
        }
        let body = &pending[offset + 5..offset + 1 + len];
        match pending[offset] {
            b'Q' if body.iter().copied().take_while(|byte| *byte != 0).collect::<Vec<_>>().eq_ignore_ascii_case(b"COMMIT") => {
                events.push(Frontend::Commit)
            }
            b'P' | b'Q' if needle.is_some_and(|needle| contains(body, needle.as_bytes())) => events.push(Frontend::Statement),
            _ => {}
        }
        offset += len + 1;
    }
    if offset > 0 {
        pending.drain(..offset);
    }
    events
}

async fn proxy_connection(caller: TcpStream, backend_address: SocketAddr, state: Arc<Armed>) {
    let Ok(backend) = TcpStream::connect(backend_address).await else {
        return;
    };
    let (mut caller_read, mut caller_write) = caller.into_split();
    let (mut backend_read, mut backend_write) = backend.into_split();
    let statement_sent = Arc::new(AtomicBool::new(false));
    let cut = Arc::new(Notify::new());
    let front = tokio::spawn({
        let (state, statement_sent, cut) = (state.clone(), statement_sent.clone(), cut.clone());
        async move {
            let (mut pending, mut startup) = (Vec::new(), false);
            let mut buf = [0u8; 8192];
            let needle = match state.fault {
                Fault::CutAfterStatement(needle) => Some(needle),
                _ => state.after_statement,
            };
            loop {
                let Ok(n) = caller_read.read(&mut buf).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                pending.extend_from_slice(&buf[..n]);
                let events = scan_frontend(&mut pending, &mut startup, needle);
                if events.iter().any(|event| matches!(event, Frontend::Statement)) {
                    statement_sent.store(true, Ordering::SeqCst);
                }
                // COMMIT은 따로 한 번의 쓰기로 온다. 가로챈 COMMIT은 서버에 전달하지 않는다.
                let primed = state.after_statement.is_none() || statement_sent.load(Ordering::SeqCst);
                if primed && events.iter().any(|event| matches!(event, Frontend::Commit)) && state.fault == Fault::CutOnCommitSent && state.take() {
                    state.fire("COMMIT was sent by the client and withheld from PostgreSQL; connection closed").await;
                    cut.notify_one();
                    return;
                }
                if backend_write.write_all(&buf[..n]).await.is_err() {
                    return;
                }
            }
        }
    });
    loop {
        let mut header = [0u8; 5];
        let read = tokio::select! {
            read = backend_read.read_exact(&mut header) => read,
            _ = cut.notified() => break,
        };
        if read.is_err() {
            break;
        }
        let len = u32::from_be_bytes(header[1..5].try_into().unwrap()) as usize;
        if !(4..=16 * 1024 * 1024).contains(&len) {
            break;
        }
        let mut body = vec![0u8; len - 4];
        if backend_read.read_exact(&mut body).await.is_err() {
            break;
        }
        let command_complete = header[0] == b'C';
        let primed = state.after_statement.is_none() || statement_sent.load(Ordering::SeqCst);
        let is_commit_complete = primed && command_complete && body.strip_suffix(&[0]).is_some_and(|tag| tag == b"COMMIT");
        match state.fault {
            Fault::AckDropFin | Fault::AckDropReset if is_commit_complete && state.take() => {
                state.fire("CommandComplete(COMMIT) observed from PostgreSQL; response withheld").await;
                front.abort();
                if state.fault == Fault::AckDropReset {
                    // 닫을 때 RST를 보내려면 SO_LINGER 0이 필요하다. 테스트 proxy라 닫기 지연은 문제가 아니다.
                    #[allow(deprecated)]
                    let _ = caller_write.as_ref().set_linger(Some(Duration::ZERO));
                } else {
                    let _ = caller_write.shutdown().await;
                }
                return;
            }
            Fault::CutAfterStatement(_) if command_complete && statement_sent.load(Ordering::SeqCst) && state.take() => {
                state.fire("CommandComplete of the targeted statement observed from PostgreSQL; response withheld before COMMIT").await;
                front.abort();
                let _ = caller_write.shutdown().await;
                return;
            }
            Fault::ReplaceCommitAck(tag) if is_commit_complete && state.take() => {
                state.fire("CommandComplete(COMMIT) observed from PostgreSQL; replaced with an unexpected message").await;
                header = [tag, 0, 0, 0, 4];
                body.clear();
            }
            _ => {}
        }
        if caller_write.write_all(&header).await.is_err() || caller_write.write_all(&body).await.is_err() {
            break;
        }
    }
    front.abort();
}

/// 설정된 테스트 DB 앞에 proxy를 세우고 주소와 "장애가 일어났다"는 신호를 돌려준다.
/// 연결 문자열은 `database_url_via_proxy(address.port())`로 만든다.
pub async fn start(fault: Fault) -> (SocketAddr, oneshot::Receiver<String>) {
    start_inner(fault, None).await
}

/// COMMIT 단계 장애를, 문장에 `needle`이 든 요청을 보낸 연결에서만 일으킨다. 서버의 배경 작업이 같은 proxy를 거칠 때 쓴다.
pub async fn start_after(needle: &'static str, fault: Fault) -> (SocketAddr, oneshot::Receiver<String>) {
    start_inner(fault, Some(needle)).await
}

async fn start_inner(fault: Fault, after_statement: Option<&'static str>) -> (SocketAddr, oneshot::Receiver<String>) {
    let (backend_host, backend_port) = aip_test_support::endpoint(aip_test_support::database_url()).unwrap();
    let backend_address = std::net::ToSocketAddrs::to_socket_addrs(&(backend_host.as_str(), backend_port))
        .ok()
        .and_then(|mut addresses| addresses.next())
        .expect("resolve the test database endpoint");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (sender, receiver) = oneshot::channel();
    let state = Arc::new(Armed { fault, after_statement, armed: AtomicBool::new(true), fired: tokio::sync::Mutex::new(Some(sender)) });
    tokio::spawn(async move {
        loop {
            let Ok((caller, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(proxy_connection(caller, backend_address, state.clone()));
        }
    });
    (address, receiver)
}
