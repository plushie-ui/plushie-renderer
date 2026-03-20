//! Transport layer: configurable I/O source for the protocol channel.
//!
//! By default, toddy uses stdin/stdout (the host spawns toddy as a
//! subprocess). The `--exec` flag spawns a command and uses its
//! stdin/stdout instead, enabling remote rendering over SSH.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStderr, Command, Stdio};
use std::thread::{self, JoinHandle};

/// The I/O endpoints for protocol communication.
pub(crate) struct Transport {
    /// Reader for incoming messages from the host.
    pub reader: BufReader<Box<dyn Read + Send>>,
    /// Writer for outgoing messages to the host.
    pub writer: Box<dyn Write + Send>,
    /// Held to keep child process alive. Dropped on toddy exit.
    _child: Option<Child>,
    /// Reads child stderr and forwards to toddy's stderr with prefix.
    _stderr_thread: Option<JoinHandle<()>>,
}

impl Transport {
    /// Standard I/O transport (current default).
    pub fn stdio() -> Self {
        Self {
            reader: BufReader::with_capacity(64 * 1024, Box::new(io::stdin())),
            writer: Box::new(io::stdout()),
            _child: None,
            _stderr_thread: None,
        }
    }

    /// Spawn a command and use its stdin/stdout as the protocol channel.
    ///
    /// The command is run through the system shell (`sh -c` on Unix,
    /// `cmd /c` on Windows). The child's stderr is forwarded to toddy's
    /// stderr with a `[remote]` prefix.
    pub fn exec(command: &str) -> io::Result<Self> {
        let mut child = Command::new(if cfg!(windows) { "cmd" } else { "sh" })
            .args(if cfg!(windows) {
                vec!["/c", command]
            } else {
                vec!["-c", command]
            })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| io::Error::other(format!("failed to exec '{command}': {e}")))?;

        let child_stdout = child.stdout.take().expect("child stdout piped");
        let child_stdin = child.stdin.take().expect("child stdin piped");
        let child_stderr = child.stderr.take().expect("child stderr piped");

        let stderr_thread = spawn_stderr_forwarder(child_stderr);

        Ok(Self {
            reader: BufReader::with_capacity(64 * 1024, Box::new(child_stdout)),
            writer: Box::new(child_stdin),
            _child: Some(child),
            _stderr_thread: Some(stderr_thread),
        })
    }

    /// Name of this transport for the hello message.
    pub fn name(&self) -> &'static str {
        if self._child.is_some() {
            "exec"
        } else {
            "stdio"
        }
    }

    /// Consume the transport into its constituent parts.
    ///
    /// Returns the reader, writer, and a guard that holds the child
    /// process and stderr thread alive until dropped.
    pub fn into_parts(
        self,
    ) -> (
        BufReader<Box<dyn Read + Send>>,
        Box<dyn Write + Send>,
        TransportGuard,
    ) {
        (
            self.reader,
            self.writer,
            TransportGuard {
                _child: self._child,
                _stderr_thread: self._stderr_thread,
            },
        )
    }
}

/// Holds transport resources (child process, stderr thread) for
/// cleanup on drop.
pub(crate) struct TransportGuard {
    _child: Option<Child>,
    _stderr_thread: Option<JoinHandle<()>>,
}

/// Read lines from the child's stderr and forward them to toddy's
/// stderr with a `[remote]` prefix. Exits when the child closes stderr.
fn spawn_stderr_forwarder(stderr: ChildStderr) -> JoinHandle<()> {
    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        let mut lines = reader.lines();
        while let Some(Ok(line)) = lines.next() {
            eprintln!("[remote] {line}");
        }
    })
}
