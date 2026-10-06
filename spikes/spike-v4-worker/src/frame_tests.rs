use super::BoundedLines;
use std::io::{self, Cursor, ErrorKind};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader};

fn assert_invalid_data<T: std::fmt::Debug>(result: io::Result<T>) {
    let err = result.expect_err("expected InvalidData");
    assert_eq!(err.kind(), ErrorKind::InvalidData);
}

#[tokio::test]
async fn newline_and_crlf_count_toward_the_exact_frame_limit() {
    let input = Cursor::new(b"abc\nab\r\n".to_vec());
    let mut lines = BoundedLines::new(BufReader::with_capacity(2, input), 4);

    assert_eq!(lines.next_line().await.unwrap().as_deref(), Some("abc"));
    assert_eq!(lines.next_line().await.unwrap().as_deref(), Some("ab"));
    assert_eq!(lines.next_line().await.unwrap(), None);
}

#[tokio::test]
async fn one_byte_over_limit_permanently_rejects_the_stream() {
    let input = Cursor::new(b"abcd\nnext\n".to_vec());
    let mut lines = BoundedLines::new(BufReader::with_capacity(2, input), 4);

    assert_invalid_data(lines.next_line().await);
    assert_invalid_data(lines.next_line().await);
}

#[tokio::test]
async fn eof_returns_a_valid_trailing_frame_and_rejects_an_oversized_one() {
    let input = Cursor::new(b"abc".to_vec());
    let mut lines = BoundedLines::new(BufReader::with_capacity(2, input), 3);
    assert_eq!(lines.next_line().await.unwrap().as_deref(), Some("abc"));
    assert_eq!(lines.next_line().await.unwrap(), None);

    let input = Cursor::new(b"abcd".to_vec());
    let mut lines = BoundedLines::new(BufReader::with_capacity(2, input), 3);
    assert_invalid_data(lines.next_line().await);
    assert_invalid_data(lines.next_line().await);
}

#[tokio::test]
async fn invalid_utf8_is_invalid_data() {
    let input = Cursor::new(b"a\xff\n".to_vec());
    let mut lines = BoundedLines::new(BufReader::with_capacity(2, input), 4);
    assert_invalid_data(lines.next_line().await);
}

#[tokio::test]
async fn timeout_cancellation_preserves_the_partial_frame() {
    let (mut tx, rx) = tokio::io::duplex(64);
    let mut lines = BoundedLines::new(BufReader::with_capacity(2, rx), 16);
    tx.write_all(b"abc").await.unwrap();

    assert!(tokio::time::timeout(Duration::from_millis(25), lines.next_line()).await.is_err());

    tx.write_all(b"def\n").await.unwrap();
    assert_eq!(lines.next_line().await.unwrap().as_deref(), Some("abcdef"));
}

#[tokio::test]
async fn utf8_frame_limit_counts_bytes_including_the_delimiter() {
    let mut exact = BoundedLines::new(BufReader::with_capacity(2, Cursor::new("한\n".as_bytes())), 4);
    assert_eq!(exact.next_line().await.unwrap().as_deref(), Some("한"));
    let mut too_large = BoundedLines::new(BufReader::with_capacity(2, Cursor::new("한\n".as_bytes())), 3);
    assert_invalid_data(too_large.next_line().await);
}

struct BrokenReader;

impl tokio::io::AsyncRead for BrokenReader {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        _buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Err(io::Error::new(ErrorKind::BrokenPipe, "test read failure")))
    }
}

#[tokio::test]
async fn io_failure_is_reported_once_then_the_stream_is_discarded() {
    let mut lines = BoundedLines::new(BufReader::new(BrokenReader), 4);
    assert_eq!(lines.next_line().await.unwrap_err().kind(), ErrorKind::BrokenPipe);
    assert_invalid_data(lines.next_line().await);
}
