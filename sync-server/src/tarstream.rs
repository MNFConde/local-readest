//! Shared plumbing for handlers that stream a tar out of a response body:
//! chunks flow through an mpsc channel so the archive never sits in memory
//! whole (users: admin backup, per-user archive export).

use std::io::Write;

pub(crate) const STREAM_CHUNK_BYTES: usize = 64 * 1024;

/// Bridges the synchronous tar builder into the response body.
pub(crate) struct ChannelWriter {
    tx: tokio::sync::mpsc::Sender<Result<Vec<u8>, std::io::Error>>,
    buf: Vec<u8>,
}

impl ChannelWriter {
    pub fn new(tx: tokio::sync::mpsc::Sender<Result<Vec<u8>, std::io::Error>>) -> Self {
        Self {
            tx,
            buf: Vec::new(),
        }
    }

    fn flush_buf(&mut self) -> std::io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::take(&mut self.buf);
        // Receiver gone = client disconnected: tear the archive down.
        self.tx
            .blocking_send(Ok(chunk))
            .map_err(|_| std::io::Error::other("archive stream closed"))
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(data);
        if self.buf.len() >= STREAM_CHUNK_BYTES {
            self.flush_buf()?;
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.flush_buf()
    }
}
