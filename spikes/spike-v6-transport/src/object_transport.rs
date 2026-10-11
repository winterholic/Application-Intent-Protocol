//! Authenticated object transfer endpoints. Local object bytes stream through
//! bounded channels to blocking filesystem workers; S3 transfers use short
//! lived URLs and small JSON control requests.

use crate::{auth::Session, cors, durable, infrastructure::ObjectStorage, server::Runtime};
use aip_execution::objects::{
    CompletedPart, Error as ObjectError, LocalStore, MultipartUpload, ObjectKey, S3Store, MAX_S3_OBJECT_BYTES, MAX_S3_PART_BYTES,
};
use serde_json::{json, Value};
use spike_v2_read::id_wire::IdWire;
use std::{io, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
    task::JoinHandle,
};

const IO_TIMEOUT: Duration = Duration::from_secs(30);
const PIPE_DEPTH: usize = 2;
const STREAM_CHUNK: usize = 64 * 1024;
const PRESIGN_SECONDS: u64 = 300;

/// Handles raw object paths and streams request/response bytes without loading
/// the full object into memory. Returns false only for paths owned by another
/// route family.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve<R, W>(
    reader: &mut R,
    writer: &mut W,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    content_length: Option<usize>,
    session: Option<&Session>,
    rt: &Runtime,
    facts: &Value,
    wire: IdWire,
    origin: Option<&str>,
) -> bool
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let head_only = method.eq_ignore_ascii_case("HEAD");
    if path != "/objects/put" && !path.starts_with("/objects/") {
        return false;
    }
    if path == "/objects/sign" || path.starts_with("/objects/multipart/") {
        return false;
    }
    let cors_headers = cors::actual_response_headers(origin);
    let _slot = match rt.object_slots.try_acquire() {
        Ok(permit) => permit,
        Err(_) => return send_json(writer, 503, "OBJECT_CAPACITY", "동시 객체 전송 상한에 도달함", &cors_headers, head_only).await,
    };
    let Some(session) = session else {
        return send_json(writer, 401, "UNAUTHENTICATED", "검증된 principal 세션이 필요함", &cors_headers, head_only).await;
    };
    if session.remaining_ms() == 0 {
        return send_json(writer, 401, "TOKEN_EXPIRED", "세션 만료", &cors_headers, head_only).await;
    }
    let owner = match validate_owner(rt, facts, wire, session).await {
        Ok(owner) => owner,
        Err(code) => return send_json(writer, status_for(code), code, "principal 또는 배포 상태 확인 실패", &cors_headers, head_only).await,
    };
    let storage = match &rt.infrastructure.objects {
        Some(storage) => storage.clone(),
        None => return send_json(writer, 503, "NOT_AVAILABLE", "object storage 설정이 필요함", &cors_headers, head_only).await,
    };
    let store = match storage {
        ObjectStorage::Local(base) => match tokio::task::spawn_blocking(move || base.for_owner(&owner)).await {
            Ok(Ok(store)) => Arc::new(store),
            _ => return send_json(writer, 503, "STORAGE_UNAVAILABLE", "object storage 준비 실패", &cors_headers, head_only).await,
        },
        ObjectStorage::S3(_) => {
            return send_json(writer, 405, "METHOD_NOT_ALLOWED", "S3 객체는 presigned URL로 전송해야 함", &cors_headers, head_only).await;
        }
    };

    if path == "/objects/put" {
        if method != "POST" {
            return send_json(writer, 405, "METHOD_NOT_ALLOWED", "POST만 지원함", &cors_headers, head_only).await;
        }
        let Some(length) = content_length else {
            return send_json(writer, 411, "LENGTH_REQUIRED", "content-length가 필요함", &cors_headers, head_only).await;
        };
        let Some(content_type) = header(headers, "content-type").filter(|value| valid_content_type(value)) else {
            return send_json(writer, 400, "BAD_REQUEST", "유효한 content-type이 필요함", &cors_headers, head_only).await;
        };
        if length as u64 > store.max_bytes() {
            return send_json(writer, 413, "PAYLOAD_TOO_LARGE", "저장소 객체 크기 상한 초과", &cors_headers, head_only).await;
        }
        return receive_object(reader, writer, length, content_type.to_owned(), store, session, rt, facts, wire, &cors_headers).await;
    }

    let Some(key) = path.strip_prefix("/objects/").and_then(|value| ObjectKey::parse(value).ok()) else {
        return send_json(writer, 404, "NOT_FOUND", "객체를 찾을 수 없음", &cors_headers, head_only).await;
    };
    match method {
        "GET" | "HEAD" => {
            // HEAD avoids opening object bytes in the filesystem backend.
            if method == "HEAD" {
                let key_for_worker = key.clone();
                let store_for_worker = store.clone();
                let metadata = match tokio::task::spawn_blocking(move || store_for_worker.stat(&key_for_worker)).await {
                    Ok(Ok(Some(metadata))) => metadata,
                    Ok(Ok(None)) => return send_json(writer, 404, "NOT_FOUND", "객체를 찾을 수 없음", &cors_headers, head_only).await,
                    _ => return send_json(writer, 503, "STORAGE_UNAVAILABLE", "object storage 조회 실패", &cors_headers, head_only).await,
                };
                if let Err(code) = validate_owner(rt, facts, wire, session).await {
                    return send_json(writer, status_for(code), code, "principal 상태 변경", &cors_headers, head_only).await;
                }
                return send_head(writer, metadata.size, &metadata.content_type, &metadata.etag, &cors_headers).await;
            }
            let key_for_worker = key.clone();
            let store_for_worker = store.clone();
            let opened = match tokio::task::spawn_blocking(move || store_for_worker.open(&key_for_worker)).await {
                Ok(Ok(opened)) => opened,
                Ok(Err(ObjectError::NotFound)) => return send_json(writer, 404, "NOT_FOUND", "객체를 찾을 수 없음", &cors_headers, head_only).await,
                _ => return send_json(writer, 503, "STORAGE_UNAVAILABLE", "object storage 읽기 실패", &cors_headers, head_only).await,
            };
            if let Err(code) = validate_owner(rt, facts, wire, session).await {
                return send_json(writer, status_for(code), code, "principal 상태 변경", &cors_headers, head_only).await;
            }
            return send_object(writer, opened.0, opened.1.size, &opened.1.content_type, &opened.1.etag, rt, facts, wire, session, &cors_headers)
                .await;
        }
        "DELETE" => {
            if let Err(code) = validate_owner(rt, facts, wire, session).await {
                return send_json(writer, status_for(code), code, "principal 상태 변경", &cors_headers, head_only).await;
            }
            let deleted = match tokio::task::spawn_blocking(move || store.delete(&key)).await {
                Ok(Ok(deleted)) => deleted,
                _ => return send_json(writer, 503, "STORAGE_UNAVAILABLE", "object storage 삭제 실패", &cors_headers, head_only).await,
            };
            if deleted {
                send_empty(writer, 204, &cors_headers).await
            } else {
                send_json(writer, 404, "NOT_FOUND", "객체를 찾을 수 없음", &cors_headers, head_only).await
            }
        }
        _ => send_json(writer, 405, "METHOD_NOT_ALLOWED", "GET, HEAD, DELETE만 지원함", &cors_headers, head_only).await,
    }
}

#[allow(clippy::too_many_arguments)]
async fn receive_object<R, W>(
    reader: &mut R,
    writer: &mut W,
    length: usize,
    content_type: String,
    store: Arc<LocalStore>,
    session: &Session,
    rt: &Runtime,
    facts: &Value,
    wire: IdWire,
    cors_headers: &str,
) -> bool
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (tx, mut rx) = mpsc::channel::<io::Result<Vec<u8>>>(PIPE_DEPTH);
    let store_for_worker = store.clone();
    let content_type_for_worker = content_type.clone();
    let worker: JoinHandle<Result<_, ObjectError>> = tokio::task::spawn_blocking(move || {
        let mut upload = store_for_worker.begin_upload(&content_type_for_worker)?;
        let mut received = 0usize;
        while let Some(chunk) = rx.blocking_recv() {
            let bytes = chunk?;
            received = received.checked_add(bytes.len()).ok_or(ObjectError::TooLarge)?;
            upload.write_chunk(&bytes)?;
        }
        if received != length {
            return Err(ObjectError::Protocol("request body length mismatch".into()));
        }
        Ok(upload)
    });
    let mut remaining = length;
    let mut buffer = vec![0u8; STREAM_CHUNK];
    while remaining > 0 {
        let take = remaining.min(buffer.len());
        let read = match tokio::time::timeout(IO_TIMEOUT, reader.read(&mut buffer[..take])).await {
            Ok(Ok(read)) if read > 0 => read,
            _ => {
                let _ = tx.send(Err(io::Error::new(io::ErrorKind::UnexpectedEof, "request body ended early"))).await;
                drop(tx);
                let _ = worker.await;
                return send_json(writer, 400, "BAD_REQUEST", "요청 본문이 content-length보다 짧음", cors_headers, false).await;
            }
        };
        remaining -= read;
        if !tokio::time::timeout(IO_TIMEOUT, tx.send(Ok(buffer[..read].to_vec()))).await.is_ok_and(|result| result.is_ok()) {
            drop(tx);
            let error = match worker.await {
                Ok(Err(error)) => error,
                _ => ObjectError::TooLarge,
            };
            return send_object_error(writer, error, cors_headers).await;
        }
    }
    drop(tx);
    let upload = match worker.await {
        Ok(Ok(upload)) => upload,
        Ok(Err(error)) => return send_object_error(writer, error, cors_headers).await,
        Err(_) => return send_json(writer, 503, "STORAGE_UNAVAILABLE", "object storage worker 실패", cors_headers, false).await,
    };
    if session.remaining_ms() == 0 {
        return send_json(writer, 401, "TOKEN_EXPIRED", "업로드 중 세션 만료", cors_headers, false).await;
    }
    if let Err(code) = validate_owner(rt, facts, wire, session).await {
        return send_json(writer, status_for(code), code, "업로드 중 principal 상태 변경", cors_headers, false).await;
    }
    match tokio::task::spawn_blocking(move || upload.finish()).await {
        Ok(Ok((key, metadata))) => {
            send_json_value(
                writer,
                201,
                &json!({
                    "ok": true,
                    "id": key.to_string(),
                    "size": metadata.size,
                    "etag": metadata.etag,
                    "contentType": metadata.content_type,
                }),
                cors_headers,
                false,
            )
            .await
        }
        Ok(Err(error)) => send_object_error(writer, error, cors_headers).await,
        Err(_) => send_json(writer, 503, "STORAGE_UNAVAILABLE", "object storage 저장 실패", cors_headers, false).await,
    }
}

/// S3 JSON control endpoints. Object bytes never pass through this handler.
pub(crate) async fn control(rt: &Runtime, facts: &Value, wire: IdWire, session: Option<&Session>, body: &Value, path: &str) -> Value {
    let _slot = match rt.object_slots.try_acquire() {
        Ok(permit) => permit,
        Err(_) => return failure("OBJECT_CAPACITY"),
    };
    let Some(session) = session else {
        return failure("UNAUTHENTICATED");
    };
    if session.remaining_ms() == 0 {
        return failure("TOKEN_EXPIRED");
    }
    let owner = match validate_owner(rt, facts, wire, session).await {
        Ok(owner) => owner,
        Err(code) => return failure(code),
    };
    let Some(ObjectStorage::S3(base)) = &rt.infrastructure.objects else {
        return failure("NOT_AVAILABLE");
    };
    let store = match base.for_owner(&owner) {
        Ok(store) => store,
        Err(_) => return failure("BAD_REQUEST"),
    };
    let response = match path {
        "/objects/sign" => sign(&store, body),
        "/objects/multipart/start" => multipart_start(&store, body).await,
        "/objects/multipart/part" => multipart_part(&store, body),
        "/objects/multipart/complete" => multipart_complete(&store, body).await,
        "/objects/multipart/abort" => multipart_abort(&store, body).await,
        _ => failure("NOT_FOUND"),
    };
    let postflight = if session.remaining_ms() == 0 { Err("TOKEN_EXPIRED") } else { validate_owner(rt, facts, wire, session).await };
    if let Err(code) = postflight {
        if path == "/objects/multipart/start" && response["ok"] == true {
            if let Some(upload) = parse_upload(&json!({"id":response["id"],"uploadId":response["uploadId"]})) {
                let _ = store.abort_multipart(&upload).await;
            }
        }
        return failure(code);
    }
    response
}

fn sign(store: &S3Store, body: &Value) -> Value {
    let Some(method) = body.get("method").and_then(Value::as_str) else {
        return failure("BAD_REQUEST");
    };
    let has_id = body.get("id").is_some();
    let has_type = body.get("contentType").is_some();
    let valid_keys = match method {
        "PUT" => (exact(body, &["method", "contentType", "size"]) && !has_id) || (exact(body, &["method", "id", "contentType", "size"]) && has_id),
        "GET" | "HEAD" | "DELETE" => exact(body, &["method", "id"]) && !has_type,
        _ => false,
    };
    if !valid_keys {
        return failure("BAD_REQUEST");
    }
    if method == "PUT" && !body["size"].as_u64().is_some_and(|size| size <= MAX_S3_OBJECT_BYTES) {
        return failure("PAYLOAD_TOO_LARGE");
    }
    let key = if let Some(id) = body["id"].as_str() {
        match ObjectKey::parse(id) {
            Ok(key) => key,
            Err(_) => return failure("BAD_REQUEST"),
        }
    } else {
        ObjectKey::generate()
    };
    let url = match method {
        "PUT" => body["contentType"]
            .as_str()
            .filter(|value| valid_content_type(value))
            .ok_or(ObjectError::InvalidMetadata)
            .and_then(|content_type| store.presign_put_sized(&key, content_type, body["size"].as_u64().unwrap_or(u64::MAX), PRESIGN_SECONDS)),
        "GET" => store.presign_get(&key, PRESIGN_SECONDS),
        "HEAD" => store.presign_head(&key, PRESIGN_SECONDS),
        "DELETE" => store.presign_delete(&key, PRESIGN_SECONDS),
        _ => unreachable!(),
    };
    match url {
        Ok(url) => {
            if method == "PUT" {
                json!({"ok":true,"id":key.to_string(),"method":method,"url":url,"headers":{"content-type":body["contentType"],"content-length":body["size"].as_u64().unwrap().to_string()},"expiresIn":PRESIGN_SECONDS})
            } else {
                json!({"ok":true,"id":key.to_string(),"method":method,"url":url,"expiresIn":PRESIGN_SECONDS})
            }
        }
        Err(_) => failure("BAD_REQUEST"),
    }
}

async fn multipart_start(store: &S3Store, body: &Value) -> Value {
    if !exact(body, &["contentType"]) {
        return failure("BAD_REQUEST");
    }
    let Some(content_type) = body["contentType"].as_str().filter(|value| valid_content_type(value)) else {
        return failure("BAD_REQUEST");
    };
    match store.create_multipart(content_type).await {
        Ok(upload) => json!({"ok":true,"id":upload.key.to_string(),"uploadId":upload.upload_id}),
        Err(_) => failure("OBJECT_STORE_ERROR"),
    }
}

fn multipart_part(store: &S3Store, body: &Value) -> Value {
    if !exact(body, &["id", "uploadId", "partNumber", "size"]) {
        return failure("BAD_REQUEST");
    }
    let Some(key) = parse_key(body) else {
        return failure("BAD_REQUEST");
    };
    let Some(upload_id) = body["uploadId"].as_str() else {
        return failure("BAD_REQUEST");
    };
    let Some(part_number) = body["partNumber"].as_u64().and_then(|n| u16::try_from(n).ok()) else {
        return failure("BAD_REQUEST");
    };
    if !body["size"].as_u64().is_some_and(|size| size > 0 && size <= MAX_S3_PART_BYTES) {
        return failure("PAYLOAD_TOO_LARGE");
    }
    let size = body["size"].as_u64().unwrap();
    match store.presign_part_sized(&key, upload_id, part_number, size, PRESIGN_SECONDS) {
        Ok(url) => {
            json!({"ok":true,"id":key.to_string(),"partNumber":part_number,"method":"PUT","url":url,"headers":{"content-length":size.to_string()},"expiresIn":PRESIGN_SECONDS})
        }
        Err(_) => failure("BAD_REQUEST"),
    }
}

async fn multipart_complete(store: &S3Store, body: &Value) -> Value {
    if !exact(body, &["id", "uploadId", "parts"]) {
        return failure("BAD_REQUEST");
    }
    let Some(upload) = parse_upload(body) else {
        return failure("BAD_REQUEST");
    };
    let Some(parts) = body["parts"].as_array().filter(|parts| !parts.is_empty() && parts.len() <= 10_000) else {
        return failure("BAD_REQUEST");
    };
    let mut completed = Vec::with_capacity(parts.len());
    for part in parts {
        if !exact(part, &["number", "etag"]) {
            return failure("BAD_REQUEST");
        }
        let Some(number) = part["number"].as_u64().and_then(|n| u16::try_from(n).ok()) else {
            return failure("BAD_REQUEST");
        };
        let Some(etag) = part["etag"].as_str().filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)) else {
            return failure("BAD_REQUEST");
        };
        completed.push(CompletedPart { number, etag: etag.to_owned() });
    }
    match store.complete_multipart(&upload, &completed).await {
        Ok(()) => json!({"ok":true,"id":upload.key.to_string(),"completed":true}),
        Err(ObjectError::InvalidMultipart) => failure("BAD_REQUEST"),
        Err(_) => failure("OBJECT_STORE_ERROR"),
    }
}

async fn multipart_abort(store: &S3Store, body: &Value) -> Value {
    if !exact(body, &["id", "uploadId"]) {
        return failure("BAD_REQUEST");
    }
    let Some(upload) = parse_upload(body) else {
        return failure("BAD_REQUEST");
    };
    match store.abort_multipart(&upload).await {
        Ok(()) => json!({"ok":true,"id":upload.key.to_string()}),
        Err(ObjectError::InvalidMultipart) => failure("BAD_REQUEST"),
        Err(_) => failure("OBJECT_STORE_ERROR"),
    }
}

fn parse_upload(body: &Value) -> Option<MultipartUpload> {
    Some(MultipartUpload { key: ObjectKey::parse(body["id"].as_str()?).ok()?, upload_id: body["uploadId"].as_str()?.to_owned() })
}
fn parse_key(body: &Value) -> Option<ObjectKey> {
    ObjectKey::parse(body["id"].as_str()?).ok()
}
fn exact(body: &Value, keys: &[&str]) -> bool {
    body.as_object().is_some_and(|object| object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key)))
}
fn valid_content_type(value: &str) -> bool {
    let Some((kind, subtype)) = value.split_once('/') else {
        return false;
    };
    !kind.is_empty()
        && !subtype.is_empty()
        && value.len() <= 127
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'+' | b'.' | b'-'))
}
fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
}
async fn validate_owner(rt: &Runtime, facts: &Value, wire: IdWire, session: &Session) -> Result<String, &'static str> {
    tokio::time::timeout(crate::server::DB_PREFLIGHT_TIMEOUT, durable::validate_principal(rt, facts, wire, session))
        .await
        .map_err(|_| "DB_UNAVAILABLE")?
}
fn status_for(code: &str) -> u16 {
    match code {
        "UNAUTHENTICATED" | "TOKEN_EXPIRED" => 401,
        "NOT_AVAILABLE" | "DB_UNAVAILABLE" | "DEPLOYMENT_CHANGED" => 503,
        _ => 403,
    }
}
fn failure(code: &str) -> Value {
    json!({"ok":false,"code":code})
}
fn object_error_code(error: &ObjectError) -> &'static str {
    match error {
        ObjectError::TooLarge | ObjectError::QuotaExceeded => "PAYLOAD_TOO_LARGE",
        ObjectError::Capacity => "OBJECT_CAPACITY",
        ObjectError::InvalidKey | ObjectError::InvalidMetadata | ObjectError::InvalidMultipart => "BAD_REQUEST",
        ObjectError::NotFound => "NOT_FOUND",
        _ => "STORAGE_UNAVAILABLE",
    }
}
async fn send_object_error<W: AsyncWrite + Unpin>(writer: &mut W, error: ObjectError, cors_headers: &str) -> bool {
    let code = object_error_code(&error);
    let status = match code {
        "PAYLOAD_TOO_LARGE" => 413,
        "OBJECT_CAPACITY" => 503,
        "BAD_REQUEST" => 400,
        "NOT_FOUND" => 404,
        _ => 503,
    };
    send_json(writer, status, code, "object storage 처리 실패", cors_headers, false).await
}
async fn send_json<W: AsyncWrite + Unpin>(writer: &mut W, status: u16, code: &str, message: &str, cors_headers: &str, head_only: bool) -> bool {
    send_json_value(writer, status, &json!({"ok":false,"code":code,"msg":message}), cors_headers, head_only).await
}
async fn send_json_value<W: AsyncWrite + Unpin>(writer: &mut W, status: u16, value: &Value, cors_headers: &str, head_only: bool) -> bool {
    let body = value.to_string();
    let content_length = body.len();
    let body = if head_only { "" } else { &body };
    let response = format!(
        "HTTP/1.1 {status} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\ncache-control: private, no-store\r\nx-content-type-options: nosniff\r\n{cors_headers}connection: close\r\n\r\n{body}",
        reason(status),
        content_length
    );
    timed_write(writer, response.as_bytes()).await
}
async fn send_head<W: AsyncWrite + Unpin>(writer: &mut W, size: u64, content_type: &str, etag: &str, cors_headers: &str) -> bool {
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {size}\r\ncontent-type: {content_type}\r\netag: \"{etag}\"\r\ncontent-disposition: attachment\r\ncache-control: private, no-store\r\nx-content-type-options: nosniff\r\n{cors_headers}connection: close\r\n\r\n"
    );
    timed_write(writer, response.as_bytes()).await
}
async fn send_empty<W: AsyncWrite + Unpin>(writer: &mut W, status: u16, cors_headers: &str) -> bool {
    let response = format!(
        "HTTP/1.1 {status} {}\r\ncontent-length: 0\r\ncache-control: private, no-store\r\nx-content-type-options: nosniff\r\n{cors_headers}connection: close\r\n\r\n",
        reason(status)
    );
    timed_write(writer, response.as_bytes()).await
}
#[allow(clippy::too_many_arguments)]
async fn send_object<W: AsyncWrite + Unpin>(
    writer: &mut W,
    file: std::fs::File,
    size: u64,
    content_type: &str,
    etag: &str,
    rt: &Runtime,
    facts: &Value,
    wire: IdWire,
    session: &Session,
    cors_headers: &str,
) -> bool {
    if session.remaining_ms() == 0 {
        return send_json(writer, 401, "TOKEN_EXPIRED", "세션 만료", cors_headers, false).await;
    }
    if let Err(code) = validate_owner(rt, facts, wire, session).await {
        return send_json(writer, status_for(code), code, "principal 상태 변경", cors_headers, false).await;
    }
    let response = format!(
        "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\ncontent-type: {content_type}\r\netag: \"{etag}\"\r\ncontent-disposition: attachment\r\ncache-control: private, no-store\r\nx-content-type-options: nosniff\r\n{cors_headers}connection: close\r\n\r\n"
    );
    if !timed_write(writer, response.as_bytes()).await {
        return true;
    }
    let (tx, mut rx) = mpsc::channel::<io::Result<Vec<u8>>>(PIPE_DEPTH);
    let reader = tokio::task::spawn_blocking(move || -> io::Result<()> {
        let mut file = file;
        let mut remaining = size;
        let mut buffer = vec![0u8; STREAM_CHUNK];
        while remaining > 0 {
            let take = remaining.min(buffer.len() as u64) as usize;
            let n = std::io::Read::read(&mut file, &mut buffer[..take])?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "stored object ended early"));
            }
            remaining -= n as u64;
            if tx.blocking_send(Ok(buffer[..n].to_vec())).is_err() {
                return Ok(());
            }
        }
        Ok(())
    });
    let mut next_revalidation = tokio::time::Instant::now() + Duration::from_secs(1);
    while let Some(chunk) = rx.recv().await {
        if tokio::time::Instant::now() >= next_revalidation || session.remaining_ms() == 0 {
            if session.remaining_ms() == 0 {
                return true;
            }
            if validate_owner(rt, facts, wire, session).await.is_err() {
                return true;
            }
            next_revalidation = tokio::time::Instant::now() + Duration::from_secs(1);
        }
        let bytes = match chunk {
            Ok(bytes) => bytes,
            Err(_) => return true,
        };
        let prefix = format!("{:X}\r\n", bytes.len());
        if !timed_write(writer, prefix.as_bytes()).await || !timed_write(writer, &bytes).await || !timed_write(writer, b"\r\n").await {
            return true;
        }
    }
    if !matches!(reader.await, Ok(Ok(()))) {
        return true;
    }
    if session.remaining_ms() == 0 || validate_owner(rt, facts, wire, session).await.is_err() {
        return true;
    }
    let _ = timed_write(writer, b"0\r\n\r\n").await;
    true
}
async fn timed_write<W: AsyncWrite + Unpin>(writer: &mut W, bytes: &[u8]) -> bool {
    tokio::time::timeout(IO_TIMEOUT, writer.write_all(bytes)).await.is_ok_and(|result| result.is_ok())
}
fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn head_json_errors_preserve_get_length_without_sending_body() {
        let mut response = Vec::new();
        send_json(&mut response, 404, "NOT_FOUND", "missing", "", true).await;
        let split = response.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap();
        let headers = std::str::from_utf8(&response[..split]).unwrap();
        let body = &response[split + 4..];
        let expected = json!({"ok":false,"code":"NOT_FOUND","msg":"missing"}).to_string().len();
        assert!(headers.lines().any(|line| line.eq_ignore_ascii_case(&format!("content-length: {expected}"))));
        assert!(body.is_empty());
    }

    #[test]
    fn object_control_put_requires_bounded_signed_size_and_exact_schema() {
        let store = S3Store::new(aip_execution::objects::S3Config::new("https://objects.example.test", "bucket", "us-east-1", "ak", "sk")).unwrap();
        let signed = sign(&store, &json!({"method":"PUT","size":64,"contentType":"image/png"}));
        assert_eq!(signed["ok"], true);
        assert!(signed["url"].as_str().unwrap().contains("X-Amz-SignedHeaders=content-length%3Bcontent-type%3Bhost"));
        assert_eq!(signed["headers"]["content-length"].as_str(), Some("64"));
        assert_eq!(sign(&store, &json!({"method":"PUT","size":u64::MAX,"contentType":"image/png"}))["code"], "PAYLOAD_TOO_LARGE");
        assert_eq!(sign(&store, &json!({"method":"PUT","size":64,"contentType":"image/png","path":"/tmp/x"}))["code"], "BAD_REQUEST");
        assert!(exact(&json!({"method":"PUT","contentType":"image/png","size":64}), &["method", "contentType", "size"]));
        assert!(valid_content_type("application/octet-stream"));
        assert!(!valid_content_type("text/plain\r\nX-Evil: yes"));
    }

    #[test]
    fn multipart_part_signs_its_declared_length_and_rejects_invalid_fields() {
        let store = S3Store::new(aip_execution::objects::S3Config::new("https://objects.example.test", "bucket", "us-east-1", "ak", "sk")).unwrap();
        let key = ObjectKey::generate().to_string();
        let body = json!({"id":key,"uploadId":"opaque+id","partNumber":2,"size":128});
        let signed = multipart_part(&store, &body);
        assert_eq!(signed["ok"], true);
        assert!(signed["url"].as_str().unwrap().contains("X-Amz-SignedHeaders=content-length%3Bhost"));
        assert_eq!(signed["headers"]["content-length"].as_str(), Some("128"));
        let mut invalid = body.clone();
        invalid["extra"] = json!(true);
        assert_eq!(multipart_part(&store, &invalid)["code"], "BAD_REQUEST");
    }
}
