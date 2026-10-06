use std::io;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

pub(crate) struct BoundedLines<R> {
    reader: R,
    buffer: Vec<u8>,
    limit: usize,
    failed: bool,
}

impl<R: AsyncBufRead + Unpin> BoundedLines<R> {
    pub(crate) fn new(reader: R, limit: usize) -> Self {
        Self { reader, buffer: Vec::new(), limit, failed: false }
    }

    pub(crate) fn into_inner(self) -> R {
        self.reader
    }

    pub(crate) fn is_failed(&self) -> bool {
        self.failed
    }

    pub(crate) fn discard(&mut self) {
        self.failed = true;
        self.buffer.clear();
    }

    fn invalid(&mut self) -> io::Error {
        self.discard();
        io::Error::new(io::ErrorKind::InvalidData, "worker frame limit or encoding violation")
    }

    fn finish(&mut self) -> io::Result<Option<String>> {
        let mut frame = std::mem::take(&mut self.buffer);
        if frame.last() == Some(&b'\n') {
            frame.pop();
            if frame.last() == Some(&b'\r') {
                frame.pop();
            }
        }
        String::from_utf8(frame).map(Some).map_err(|_| self.invalid())
    }

    pub(crate) async fn next_line(&mut self) -> io::Result<Option<String>> {
        if self.failed {
            return Err(self.invalid());
        }
        loop {
            let available = match self.reader.fill_buf().await {
                Ok(bytes) => bytes,
                Err(error) => {
                    self.discard();
                    return Err(error);
                }
            };
            if available.is_empty() {
                return if self.buffer.is_empty() { Ok(None) } else { self.finish() };
            }
            let end = available.iter().position(|byte| *byte == b'\n');
            let consumed = end.map_or(available.len(), |index| index + 1);
            if consumed > self.limit.saturating_sub(self.buffer.len()) {
                return Err(self.invalid());
            }
            if self.buffer.len() + consumed > self.buffer.capacity() && self.buffer.try_reserve_exact(consumed).is_err() {
                self.discard();
                return Err(io::Error::other("worker frame allocation failed"));
            }
            self.buffer.extend_from_slice(&available[..consumed]);
            self.reader.consume(consumed);
            // Partial bytes live on the reader, so cancelling fill_buf loses no frame state.
            if end.is_some() {
                return self.finish();
            }
        }
    }
}

#[cfg(test)]
#[path = "frame_tests.rs"]
mod tests;
