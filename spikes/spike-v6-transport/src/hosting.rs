//! Explicitly configured, read-only directory mounts for the HTTP transport.
use std::{
    ffi::CString,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    os::fd::{AsRawFd, FromRawFd},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    time::Instant,
};

const CHUNK_SIZE: usize = 64 * 1024;
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_MAX_LIFETIME: Duration = Duration::from_secs(15 * 60);
const MAX_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_MAX_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountAccess {
    Public,
    Authenticated,
}

/// Exposes only one managed PostgreSQL file bucket beneath a URL prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedFileMount {
    pub prefix: String,
    pub bucket: String,
}

impl ManagedFileMount {
    pub fn new(prefix: String, bucket: String) -> io::Result<Self> {
        if !valid_prefix(&prefix) || !valid_bucket(&bucket) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid managed file mount"));
        }
        Ok(Self { prefix, bucket })
    }

    pub fn validate(&self) -> io::Result<()> {
        if !valid_prefix(&self.prefix) || !valid_bucket(&self.bucket) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid managed file mount"));
        }
        Ok(())
    }

    pub fn matches(&self, target: &str) -> bool {
        let path = target.split_once('?').map_or(target, |(path, _)| path);
        path == self.prefix || path.strip_prefix(&self.prefix).is_some_and(|rest| rest.starts_with('/'))
    }
}

fn valid_bucket(bucket: &str) -> bool {
    !bucket.is_empty()
        && bucket.len() <= 128
        && bucket != "."
        && bucket != ".."
        && bucket.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[derive(Clone, Debug)]
pub struct StaticMount {
    pub prefix: String,
    pub directory: PathBuf,
    pub access: MountAccess,
    idle_timeout: Duration,
    max_lifetime: Duration,
}

impl StaticMount {
    pub fn new(prefix: String, directory: PathBuf, access: MountAccess) -> io::Result<Self> {
        if !valid_prefix(&prefix) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "mount prefix must be an absolute URL path without query or fragment"));
        }
        let directory = std::fs::canonicalize(&directory)?;
        if !directory.is_dir() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "mount root must be a directory"));
        }
        Ok(Self { prefix, directory, access, idle_timeout: DEFAULT_IDLE_TIMEOUT, max_lifetime: DEFAULT_MAX_LIFETIME })
    }

    pub fn with_timeouts(mut self, idle_timeout: Duration, max_lifetime: Duration) -> io::Result<Self> {
        self.idle_timeout = idle_timeout;
        self.max_lifetime = max_lifetime;
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> io::Result<()> {
        if !valid_prefix(&self.prefix)
            || self.idle_timeout.is_zero()
            || self.idle_timeout > MAX_IDLE_TIMEOUT
            || self.max_lifetime.is_zero()
            || self.max_lifetime > MAX_MAX_LIFETIME
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid static mount limits"));
        }
        Ok(())
    }

    /// Matches only a whole mount prefix, ignoring the query string.
    pub fn matches(&self, target: &str) -> bool {
        let path = target.split_once('?').map_or(target, |(path, _)| path);
        path == self.prefix || path.strip_prefix(&self.prefix).is_some_and(|rest| rest.starts_with('/'))
    }
}

fn valid_prefix(prefix: &str) -> bool {
    prefix.starts_with('/')
        && prefix.len() > 1
        && !prefix.ends_with('/')
        && !prefix.contains(['?', '#', '%', '\\'])
        && !prefix.contains("//")
        && prefix.split('/').skip(1).all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Serves a request if its target belongs to this mount. Authentication is deliberately
/// enforced by the transport before this function is called.
pub async fn serve<W: AsyncWrite + Unpin>(
    writer: &mut W,
    mount: &StaticMount,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
) -> io::Result<bool> {
    serve_with_origin(writer, mount, method, target, headers, None).await
}

/// Like `serve`, with the already validated browser origin supplied by the HTTP transport.
/// Callers must never pass an origin before applying their configured origin policy.
pub async fn serve_with_origin<W: AsyncWrite + Unpin>(
    writer: &mut W,
    mount: &StaticMount,
    method: &str,
    target: &str,
    headers: &[(&str, &str)],
    origin: Option<&str>,
) -> io::Result<bool> {
    if !mount.matches(target) {
        return Ok(false);
    }
    mount.validate()?;
    let deadline = Instant::now() + mount.max_lifetime;
    let cache_control = match mount.access {
        MountAccess::Authenticated => "private, no-store",
        MountAccess::Public => "no-cache",
    };
    let allow_origin = origin.map(|origin| format!("access-control-allow-origin: {origin}\r\n")).unwrap_or_default();
    let cors_headers = format!("{allow_origin}vary: Origin\r\n");
    let path = target.split_once('?').map_or(target, |(path, _)| path);
    let Some(relative) = path.strip_prefix(&mount.prefix) else { return Ok(false) };
    if relative.is_empty() {
        return send_empty(writer, 404, "Not Found", mount, deadline, &cors_headers).await.map(|_| true);
    }
    if method != "GET" && method != "HEAD" {
        return send_empty(writer, 405, "Method Not Allowed", mount, deadline, &cors_headers).await.map(|_| true);
    }
    let components = match decode_relative(relative) {
        Ok(parts) => parts,
        Err(()) => return send_empty(writer, 400, "Bad Request", mount, deadline, &cors_headers).await.map(|_| true),
    };
    let file = match open_beneath(&mount.directory, &components) {
        Ok(file) => file,
        Err(_) => return send_empty(writer, 404, "Not Found", mount, deadline, &cors_headers).await.map(|_| true),
    };
    let metadata = match file.metadata() {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return send_empty(writer, 404, "Not Found", mount, deadline, &cors_headers).await.map(|_| true),
    };
    let length = metadata.len();
    let etag = make_etag(&metadata);
    let if_none_match = match single_header(headers, "if-none-match") {
        Ok(value) => value,
        Err(()) => return send_empty(writer, 400, "Bad Request", mount, deadline, &cors_headers).await.map(|_| true),
    };
    if if_none_match.is_some_and(|value| etag_matches(value, &etag)) {
        write_limited(
            writer,
            format!("HTTP/1.1 304 Not Modified\r\netag: {etag}\r\ncache-control: {cache_control}\r\nx-content-type-options: nosniff\r\n{cors_headers}connection: close\r\n\r\n").as_bytes(),
            mount,
            deadline,
        )
        .await?;
        return Ok(true);
    }
    let range_header = match single_header(headers, "range") {
        Ok(value) => value,
        Err(()) => return send_empty(writer, 400, "Bad Request", mount, deadline, &cors_headers).await.map(|_| true),
    };
    let range = match range_header.map(|value| parse_range(value, length)).transpose() {
        Ok(range) => range,
        Err(()) => {
            write_limited(
                writer,
                format!("HTTP/1.1 416 Range Not Satisfiable\r\ncontent-range: bytes */{length}\r\ncontent-length: 0\r\ncache-control: {cache_control}\r\nx-content-type-options: nosniff\r\n{cors_headers}connection: close\r\n\r\n").as_bytes(),
                mount,
                deadline,
            )
            .await?;
            return Ok(true);
        }
    };
    let (status, reason, start, count) = match range {
        Some((start, end)) => (206, "Partial Content", start, end - start + 1),
        None => (200, "OK", 0, length),
    };
    let content_type = content_type(components.last().map(String::as_str).unwrap_or(""));
    write_limited(
        writer,
        format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-type: {content_type}\r\ncontent-length: {count}\r\netag: {etag}\r\naccept-ranges: bytes\r\ncache-control: {cache_control}\r\nx-content-type-options: nosniff\r\n{cors_headers}"
        )
        .as_bytes(),
        mount,
        deadline,
    )
    .await?;
    if status == 206 {
        write_limited(writer, format!("content-range: bytes {start}-{}/{length}\r\n", start + count - 1).as_bytes(), mount, deadline).await?;
    }
    write_limited(writer, b"connection: close\r\n\r\n", mount, deadline).await?;
    if method == "HEAD" || count == 0 {
        return Ok(true);
    }
    let mut remaining = count;
    let mut file = file;
    let seeked = tokio::time::timeout_at(
        deadline,
        tokio::task::spawn_blocking(move || {
            file.seek(SeekFrom::Start(start))?;
            Ok::<_, io::Error>(file)
        }),
    )
    .await
    .map_err(|_| timed_out("static mount transfer lifetime exceeded"))?
    .map_err(io::Error::other)??;
    file = seeked;
    while remaining > 0 {
        let limit = remaining.min(CHUNK_SIZE as u64) as usize;
        let (next_file, chunk) = tokio::time::timeout_at(
            deadline,
            tokio::task::spawn_blocking(move || {
                let mut file = file;
                let mut bytes = vec![0; limit];
                let read = file.read(&mut bytes)?;
                bytes.truncate(read);
                Ok::<_, io::Error>((file, bytes))
            }),
        )
        .await
        .map_err(|_| timed_out("static mount transfer lifetime exceeded"))?
        .map_err(io::Error::other)??;
        file = next_file;
        if chunk.is_empty() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "mounted file changed while being served"));
        }
        write_limited(writer, &chunk, mount, deadline).await?;
        remaining -= chunk.len() as u64;
    }
    Ok(true)
}

async fn send_empty<W: AsyncWrite + Unpin>(
    writer: &mut W,
    status: u16,
    reason: &str,
    mount: &StaticMount,
    deadline: Instant,
    cors_headers: &str,
) -> io::Result<()> {
    let allow = if status == 405 { "allow: GET, HEAD\r\n" } else { "" };
    write_limited(
        writer,
        format!(
            "HTTP/1.1 {status} {reason}\r\n{allow}content-length: 0\r\nx-content-type-options: nosniff\r\n{cors_headers}connection: close\r\n\r\n"
        )
        .as_bytes(),
        mount,
        deadline,
    )
    .await
}

async fn write_limited<W: AsyncWrite + Unpin>(writer: &mut W, bytes: &[u8], mount: &StaticMount, deadline: Instant) -> io::Result<()> {
    let mut remaining = bytes;
    while !remaining.is_empty() {
        let written = tokio::time::timeout_at(deadline, tokio::time::timeout(mount.idle_timeout, writer.write(remaining)))
            .await
            .map_err(|_| timed_out("static mount transfer lifetime exceeded"))?
            .map_err(|_| timed_out("static mount write idle timeout exceeded"))??;
        if written == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "static mount response write returned zero bytes"));
        }
        remaining = &remaining[written..];
    }
    Ok(())
}

fn timed_out(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, message)
}

fn single_header<'a>(headers: &'a [(&str, &str)], name: &str) -> Result<Option<&'a str>, ()> {
    let mut values = headers.iter().filter(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| *value);
    let first = values.next();
    if values.next().is_some() {
        Err(())
    } else {
        Ok(first)
    }
}

fn decode_relative(path: &str) -> Result<Vec<String>, ()> {
    if !path.starts_with('/') || path.contains('\\') {
        return Err(());
    }
    let mut components = Vec::new();
    for raw in path[1..].split('/') {
        if raw.is_empty() {
            return Err(());
        }
        let decoded = percent_decode(raw)?;
        if decoded.is_empty() || decoded == "." || decoded == ".." || decoded.contains(['/', '\\', '%']) || decoded.chars().any(char::is_control) {
            return Err(());
        }
        components.push(decoded);
    }
    Ok(components)
}

fn percent_decode(raw: &str) -> Result<String, ()> {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3).ok_or(())?;
            let high = hex_value(hex[0]).ok_or(())?;
            let low = hex_value(hex[1]).ok_or(())?;
            decoded.push((high << 4) | low);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| ())
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
const O_DIRECTORY: i32 = 0x10000;
#[cfg(target_os = "linux")]
const O_NOFOLLOW: i32 = 0x20000;
#[cfg(target_os = "linux")]
const O_CLOEXEC: i32 = 0x80000;
#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0x800;

#[cfg(target_os = "macos")]
const O_DIRECTORY: i32 = 0x100000;
#[cfg(target_os = "macos")]
const O_NOFOLLOW: i32 = 0x100;
#[cfg(target_os = "macos")]
const O_CLOEXEC: i32 = 0x1000000;
#[cfg(target_os = "macos")]
const O_NONBLOCK: i32 = 0x4;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("secure static mounts currently support Linux and macOS only");

unsafe extern "C" {
    fn openat(dirfd: i32, pathname: *const i8, flags: i32, ...) -> i32;
}

fn open_beneath(root: &Path, components: &[String]) -> io::Result<File> {
    let mut directory = File::open(root)?;
    for (index, component) in components.iter().enumerate() {
        let name = CString::new(component.as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "nul in path"))?;
        let is_last = index + 1 == components.len();
        let flags = O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK | if is_last { 0 } else { O_DIRECTORY };
        // SAFETY: `name` is NUL-terminated and stays alive through openat. The returned
        // descriptor is owned exactly once by File; intermediate descriptors close on reassignment.
        let fd = unsafe { openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `openat` returned a fresh descriptor owned by this function.
        directory = unsafe { File::from_raw_fd(fd) };
    }
    Ok(directory)
}

fn make_etag(metadata: &std::fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!(
            "W/\"{:x}-{:x}-{:x}-{:x}-{:x}\"",
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime() as u64,
            metadata.mtime_nsec() as u64
        )
    }
    #[cfg(not(unix))]
    {
        format!("W/\"{:x}-{:?}\"", metadata.len(), metadata.modified().ok())
    }
}

fn etag_matches(header: &str, etag: &str) -> bool {
    header
        .split(',')
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag.strip_prefix("W/").unwrap_or(etag))
}

fn parse_range(value: &str, length: u64) -> Result<(u64, u64), ()> {
    let Some(value) = value.strip_prefix("bytes=") else { return Err(()) };
    if value.contains(',') || length == 0 {
        return Err(());
    }
    let (first, last) = value.split_once('-').ok_or(())?;
    let (start, end) = if first.is_empty() {
        let suffix = parse_ascii_u64(last)?;
        if suffix == 0 {
            return Err(());
        }
        (length.saturating_sub(suffix), length - 1)
    } else {
        let start = parse_ascii_u64(first)?;
        let end = if last.is_empty() { length - 1 } else { parse_ascii_u64(last)?.min(length - 1) };
        (start, end)
    };
    if start >= length || end < start {
        return Err(());
    }
    Ok((start, end))
}

fn parse_ascii_u64(value: &str) -> Result<u64, ()> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(());
    }
    value.parse().map_err(|_| ())
}

fn content_type(filename: &str) -> &'static str {
    match filename.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase()).as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("txt" | "md" | "csv") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("pdf") => "application/pdf",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}
