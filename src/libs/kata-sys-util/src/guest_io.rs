// SPDX-License-Identifier: Apache-2.0
//! Resource bounds for streams received from a guest or VMM.
use std::io;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// Read a record without ever buffering more than `limit` bytes (including newline).
/// An oversized or invalid UTF-8 record terminates this stream, not the host process.
pub async fn read_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            break;
        }
        let newline = available.iter().position(|b| *b == b'\n');
        let count = newline.map_or(available.len(), |p| p + 1);
        if count > limit.saturating_sub(line.len()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "guest record exceeds byte limit",
            ));
        }
        line.extend_from_slice(&available[..count]);
        reader.consume(count);
        if newline.is_some() {
            break;
        }
    }
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid UTF-8 guest record"))
}

/// Fixed-window record and byte budget. Reject the offending stream on overflow.
pub struct RateLimit {
    start: Instant,
    records: usize,
    bytes: usize,
    max_records: usize,
    max_bytes: usize,
}
impl RateLimit {
    pub fn new(max_records: usize, max_bytes: usize) -> Self {
        Self {
            start: Instant::now(),
            records: 0,
            bytes: 0,
            max_records,
            max_bytes,
        }
    }
    pub fn check(&mut self, bytes: usize) -> io::Result<()> {
        if self.start.elapsed() >= Duration::from_secs(1) {
            self.start = Instant::now();
            self.records = 0;
            self.bytes = 0;
        }
        if self.records >= self.max_records || bytes > self.max_bytes.saturating_sub(self.bytes) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "guest stream rate limit exceeded",
            ));
        }
        self.records += 1;
        self.bytes += bytes;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn records_are_bounded_and_eof_is_preserved() {
        let mut input = &b"abc\nxy"[..];
        assert_eq!(read_line(&mut input, 4).await.unwrap().unwrap(), "abc");
        assert_eq!(read_line(&mut input, 4).await.unwrap().unwrap(), "xy");
        assert!(read_line(&mut input, 4).await.unwrap().is_none());
        assert!(read_line(&mut &b"abcde"[..], 4).await.is_err());
        assert!(read_line(&mut &[255, 10][..], 4).await.is_err());
    }
    #[test]
    fn rate_budget_counts_both_records_and_bytes() {
        let mut budget = RateLimit::new(2, 4);
        budget.check(4).unwrap();
        assert!(budget.check(1).is_err());
        budget.check(0).unwrap();
        assert!(budget.check(0).is_err());
    }
}
