use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

async fn connect(port: u16) -> TcpStream {
    TcpStream::connect(("127.0.0.1", port)).await.unwrap()
}

async fn rejected_before_body(port: u16, headers: &[u8]) -> Result<String, String> {
    let mut stream = connect(port).await;
    stream.write_all(headers).await.map_err(|e| e.to_string())?;
    let mut response = Vec::new();
    match timeout(Duration::from_secs(2), stream.read_to_end(&mut response)).await {
        Ok(Ok(_)) => {
            let response = String::from_utf8_lossy(&response).into_owned();
            let body = response.split_once("\r\n\r\n").map(|(_, body)| body).unwrap_or("");
            let value: Value = serde_json::from_str(body).map_err(|e| format!("응답 JSON 파싱 실패: {e}; 응답={response:?}"))?;
            value["code"].as_str().map(str::to_owned).ok_or_else(|| format!("응답에 code가 없음: {response:?}"))
        }
        Ok(Err(e)) => Err(format!("응답 읽기 실패: {e}")),
        Err(_) => Err("2초 안에 서버가 연결을 종료하지 않음".into()),
    }
}

#[tokio::test]
async fn v8_rejects_oversized_headers_before_reading_the_body() {
    let (port, _) = spike_v6_transport::start(json!({})).await;
    let mut cases = Vec::new();

    let mut long_line = b"POST /read HTTP/1.1\r\nx-long: ".to_vec();
    long_line.extend(std::iter::repeat_n(b'a', 8193));
    cases.push(("8 KiB 초과 단일 헤더 라인", long_line));

    let mut large_headers = b"POST /read HTTP/1.1\r\n".to_vec();
    for name in ["x-first", "x-second", "x-third"] {
        large_headers.extend_from_slice(format!("{name}: ").as_bytes());
        large_headers.extend(std::iter::repeat_n(b'b', 6000));
        large_headers.extend_from_slice(b"\r\n");
    }
    cases.push(("16 KiB 초과 전체 헤더", large_headers));

    let mut long_bearer = b"POST /read HTTP/1.1\r\nauthorization: Bearer ".to_vec();
    long_bearer.extend(std::iter::repeat_n(b'c', spike_v6_transport::MAX_TOKEN + 1));
    long_bearer.extend_from_slice(b"\r\n");
    cases.push(("Bearer 토큰 상한 초과", long_bearer));

    let mut failures = Vec::new();
    for (name, headers) in cases {
        match rejected_before_body(port, &headers).await {
            Ok(code) if code == "BAD_REQUEST" => {}
            Ok(code) => failures.push(format!("{name}: BAD_REQUEST 기대, 실제 {code}")),
            Err(error) => failures.push(format!("{name}: {error}")),
        }
    }

    // 정상 크기 헤더는 본문을 받기 전까지 거부 응답을 보내지 않아야 한다.
    let mut ordinary = connect(port).await;
    ordinary.write_all(b"POST /read HTTP/1.1\r\ncontent-length: 1\r\n\r\n").await.unwrap();
    let mut response = [0u8; 1];
    match timeout(Duration::from_secs(2), ordinary.read(&mut response)).await {
        Err(_) => {}
        Ok(Ok(0)) => failures.push("정상 크기 헤더: 본문 전 연결이 닫힘".into()),
        Ok(Ok(_)) => failures.push("정상 크기 헤더: 본문 전 응답을 받음".into()),
        Ok(Err(e)) => failures.push(format!("정상 크기 헤더: 읽기 실패: {e}")),
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
