use spike_v6_transport::hosting;

use hosting::{MountAccess, StaticMount};
use std::{fs, path::PathBuf};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream},
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("aip-hosting-{}-{}", std::process::id(), getrandom_id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn getrandom_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    // The clock alone has microsecond granularity on macOS; tests starting together then shared one directory and one test's Drop deleted the other's files.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64 ^ (NEXT.fetch_add(1, Ordering::SeqCst) << 48)
}

async fn get(mount: StaticMount, method: &str, target: &str, headers: &[(&str, &str)]) -> Vec<u8> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let method = method.to_string();
    let target = target.to_string();
    let headers = headers.iter().map(|(name, value)| ((*name).to_string(), (*value).to_string())).collect::<Vec<_>>();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let headers = headers.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect::<Vec<_>>();
        hosting::serve(&mut socket, &mount, &method, &target, &headers).await.unwrap();
    });
    let mut socket = TcpStream::connect(addr).await.unwrap();
    let mut response = Vec::new();
    socket.read_to_end(&mut response).await.unwrap();
    task.await.unwrap();
    response
}

fn mount(dir: &TempDir) -> StaticMount {
    StaticMount::new("/assets".into(), dir.0.clone(), MountAccess::Public).unwrap()
}

#[tokio::test]
async fn serves_file_with_type_etag_and_head_without_body() {
    let dir = TempDir::new();
    fs::write(dir.0.join("hello.txt"), b"hello").unwrap();
    let response = get(mount(&dir), "GET", "/assets/hello.txt", &[]).await;
    let response_text = String::from_utf8(response.clone()).unwrap();
    assert!(response_text.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response_text.to_ascii_lowercase().contains("content-type: text/plain"));
    assert!(response_text.to_ascii_lowercase().contains("cache-control: no-cache"));
    assert!(response_text.contains("etag: W/\""));
    assert!(response.ends_with(b"\r\n\r\nhello"));

    let head = get(mount(&dir), "HEAD", "/assets/hello.txt", &[]).await;
    assert!(head.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(head.ends_with(b"\r\n\r\n"));
}

#[test]
fn mount_matches_only_its_path_boundary_and_keeps_access_mode() {
    let dir = TempDir::new();
    let mount = StaticMount::new("/assets".into(), dir.0.clone(), MountAccess::Authenticated).unwrap();
    assert!(mount.matches("/assets"));
    assert!(mount.matches("/assets/a.js?v=1"));
    assert!(!mount.matches("/assets-private/a.js"));
    assert_eq!(mount.access, MountAccess::Authenticated);
}

#[test]
fn static_mount_rejects_invalid_timeouts() {
    let dir = TempDir::new();
    let mount = mount(&dir);
    assert!(mount.clone().with_timeouts(std::time::Duration::ZERO, std::time::Duration::from_secs(1)).is_err());
    assert!(mount.with_timeouts(std::time::Duration::from_secs(301), std::time::Duration::from_secs(1)).is_err());
}

#[tokio::test]
async fn streams_large_file_and_supports_single_byte_range_and_cache_validation() {
    let dir = TempDir::new();
    let bytes = (0..2 * 1024 * 1024).map(|i| (i % 251) as u8).collect::<Vec<_>>();
    fs::write(dir.0.join("large.bin"), &bytes).unwrap();
    let full = get(mount(&dir), "GET", "/assets/large.bin", &[]).await;
    assert!(full.starts_with(b"HTTP/1.1 200 OK\r\n"));
    let marker = full.windows(4).position(|part| part == b"\r\n\r\n").unwrap();
    let head = String::from_utf8(full[..marker].to_vec()).unwrap();
    assert_eq!(&full[marker + 4..], bytes.as_slice());
    let etag = head.lines().find_map(|line| line.to_ascii_lowercase().starts_with("etag: ").then(|| line[6..].trim())).unwrap().to_string();

    let range = get(mount(&dir), "GET", "/assets/large.bin", &[("range", "bytes=10-14")]).await;
    assert!(range.starts_with(b"HTTP/1.1 206 Partial Content\r\n"));
    assert!(range.ends_with(&bytes[10..15]));

    let cached = get(mount(&dir), "GET", "/assets/large.bin", &[("if-none-match", &etag)]).await;
    assert!(cached.starts_with(b"HTTP/1.1 304 Not Modified\r\n"));
    assert!(String::from_utf8_lossy(&cached).to_ascii_lowercase().contains("cache-control: no-cache"));
    assert!(cached.ends_with(b"\r\n\r\n"));
}

#[tokio::test]
async fn authenticated_mount_disables_caching_and_idle_write_timeout_bounds_slow_clients() {
    let dir = TempDir::new();
    fs::write(dir.0.join("large.bin"), vec![7u8; 1024 * 1024]).unwrap();
    let mount = StaticMount::new("/private".into(), dir.0.clone(), MountAccess::Authenticated).unwrap();
    let mut sink = Vec::new();
    hosting::serve(&mut sink, &mount, "GET", "/private/large.bin", &[]).await.unwrap();
    assert!(String::from_utf8_lossy(&sink).to_ascii_lowercase().contains("cache-control: private, no-store"));

    let mount = mount.with_timeouts(std::time::Duration::from_millis(25), std::time::Duration::from_secs(2)).unwrap();
    let (mut server, _client) = tokio::io::duplex(256);
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), hosting::serve(&mut server, &mount, "GET", "/private/large.bin", &[]))
        .await
        .expect("slow client must not hold the server forever");
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
}

#[tokio::test]
async fn absolute_transfer_lifetime_bounds_progressing_but_slow_clients() {
    let dir = TempDir::new();
    fs::write(dir.0.join("large.bin"), vec![7u8; 1024 * 1024]).unwrap();
    let mount = StaticMount::new("/assets".into(), dir.0.clone(), MountAccess::Public)
        .unwrap()
        .with_timeouts(std::time::Duration::from_secs(1), std::time::Duration::from_millis(40))
        .unwrap();
    let (mut server, mut client) = tokio::io::duplex(128);
    let drain = tokio::spawn(async move {
        let mut byte = [0; 1];
        loop {
            if client.read(&mut byte).await.unwrap_or(0) == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    });
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), hosting::serve(&mut server, &mount, "GET", "/assets/large.bin", &[]))
        .await
        .expect("absolute transfer lifetime must be enforced");
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
    drain.abort();
}

#[tokio::test]
async fn rejects_bad_ranges_and_path_traversal_and_never_follows_symlinks() {
    let dir = TempDir::new();
    fs::write(dir.0.join("inside.txt"), b"inside").unwrap();
    let outside = TempDir::new();
    fs::write(outside.0.join("secret.txt"), b"secret").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.0.join("secret.txt"), dir.0.join("link.txt")).unwrap();

    let bad_range = get(mount(&dir), "GET", "/assets/inside.txt", &[("range", "bytes=3-1")]).await;
    assert!(bad_range.starts_with(b"HTTP/1.1 416 Range Not Satisfiable\r\n"));
    for path in ["/assets/%2e%2e/secret.txt", "/assets/%252e%252e/secret.txt", "/assets/%2fetc/passwd"] {
        let response = get(mount(&dir), "GET", path, &[]).await;
        assert!(!response.windows(6).any(|part| part == b"secret"));
        assert!(response.starts_with(b"HTTP/1.1 400 Bad Request\r\n") || response.starts_with(b"HTTP/1.1 404 Not Found\r\n"));
    }
    #[cfg(unix)]
    assert!(get(mount(&dir), "GET", "/assets/link.txt", &[]).await.starts_with(b"HTTP/1.1 404 Not Found\r\n"));
}
