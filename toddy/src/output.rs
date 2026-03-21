//! Native output writer backed by a background thread.
//!
//! [`ChannelWriter`] buffers bytes and sends them through a bounded
//! channel to a background writer thread. This decouples the iced
//! event loop from potentially blocking pipe I/O.

use std::io::{self, Write};
use std::sync::mpsc;
use std::thread;

/// Channel capacity for the background writer thread.
const WRITER_CHANNEL_CAPACITY: usize = 256;

/// A [`Write`] adapter that buffers bytes and sends them through a
/// bounded channel on flush. The background writer thread receives
/// the chunks and performs the actual (potentially blocking) I/O.
pub(crate) struct ChannelWriter {
    tx: mpsc::SyncSender<Vec<u8>>,
    buffer: Vec<u8>,
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let bytes = std::mem::take(&mut self.buffer);
        self.tx
            .send(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "writer thread exited"))
    }
}

/// Spawn a background writer thread and return a [`ChannelWriter`]
/// that sends encoded bytes to it. The thread owns the transport
/// writer and performs blocking I/O without stalling the caller.
pub(crate) fn spawn_writer_thread(writer: Box<dyn Write + Send>) -> ChannelWriter {
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(WRITER_CHANNEL_CAPACITY);

    thread::Builder::new()
        .name("toddy-writer".into())
        .spawn(move || {
            let mut writer = writer;
            for bytes in rx {
                if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                    break;
                }
            }
        })
        .expect("failed to spawn writer thread");

    ChannelWriter {
        tx,
        buffer: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_writer_sends_on_flush() {
        let (tx, rx) = mpsc::sync_channel(16);
        let mut w = ChannelWriter {
            tx,
            buffer: Vec::new(),
        };
        w.write_all(b"hello").unwrap();
        assert!(rx.try_recv().is_err(), "nothing sent before flush");
        w.flush().unwrap();
        let received = rx.recv().unwrap();
        assert_eq!(received, b"hello");
    }

    #[test]
    fn channel_writer_empty_flush_is_noop() {
        let (tx, rx) = mpsc::sync_channel(16);
        let mut w = ChannelWriter {
            tx,
            buffer: Vec::new(),
        };
        w.flush().unwrap();
        assert!(rx.try_recv().is_err(), "empty flush should send nothing");
    }

    #[test]
    fn channel_writer_broken_pipe_on_closed_receiver() {
        let (tx, rx) = mpsc::sync_channel(16);
        drop(rx);
        let mut w = ChannelWriter {
            tx,
            buffer: Vec::new(),
        };
        w.write_all(b"data").unwrap();
        let result = w.flush();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn spawn_writer_thread_delivers_bytes() {
        let (inner_tx, inner_rx) = mpsc::sync_channel::<Vec<u8>>(16);
        struct TestWriter(mpsc::SyncSender<Vec<u8>>);
        impl Write for TestWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0
                    .send(buf.to_vec())
                    .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "closed"))?;
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut cw = spawn_writer_thread(Box::new(TestWriter(inner_tx)));
        cw.write_all(b"test data").unwrap();
        cw.flush().unwrap();
        let received = inner_rx.recv().unwrap();
        assert_eq!(received, b"test data");
    }
}
