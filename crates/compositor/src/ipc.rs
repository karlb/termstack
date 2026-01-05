//! IPC socket for shell integration
//!
//! Handles requests from the `termstack` CLI tool.
//! The compositor listens on a Unix socket and accepts JSON messages
//! to spawn terminals or resize the focused terminal.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// IPC errors
#[derive(Debug, Error)]
pub enum IpcError {
    /// Timeout reading from socket
    #[error("timeout reading from socket")]
    Timeout,

    /// IO error during read/write
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    /// JSON parse error
    #[error("failed to parse JSON: {0}")]
    ParseError(#[from] serde_json::Error),

    /// Empty message received
    #[error("empty message received")]
    EmptyMessage,
}

/// Resize mode for terminals
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResizeMode {
    /// Full viewport height (for TUI apps)
    Full,
    /// Content-based height (normal mode)
    Content,
}

/// Message from termstack to compositor
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcMessage {
    /// Spawn a new terminal or GUI window
    #[serde(rename = "spawn")]
    Spawn {
        /// Command to execute
        command: String,
        /// Working directory for the command
        cwd: String,
        /// Environment variables to inherit
        env: HashMap<String, String>,
        /// GUI mode: None = terminal spawn, Some(true) = foreground GUI, Some(false) = background GUI
        #[serde(skip_serializing_if = "Option::is_none")]
        foreground: Option<bool>,
    },
    /// Resize the focused terminal
    #[serde(rename = "resize")]
    Resize {
        /// Resize mode
        mode: ResizeMode,
    },
}

/// Request ready for processing by the compositor
#[derive(Debug)]
pub enum IpcRequest {
    /// Spawn a new terminal or GUI window
    Spawn(SpawnRequest),
    /// Resize the focused terminal
    Resize(ResizeMode),
}

/// Spawn request ready for processing by the compositor
#[derive(Debug)]
pub struct SpawnRequest {
    /// Command to execute
    pub command: String,
    /// Working directory
    pub cwd: PathBuf,
    /// Environment variables
    pub env: HashMap<String, String>,
    /// GUI mode: None = terminal spawn, Some(true) = foreground GUI, Some(false) = background GUI
    pub foreground: Option<bool>,
}

/// Read a request from a Unix stream
///
/// Returns the parsed request and the stream for sending acknowledgement.
/// For Resize requests, caller MUST send ACK to avoid race conditions.
///
/// # Errors
///
/// Returns `IpcError::Timeout` if the read times out.
/// Returns `IpcError::Io` for other IO errors.
/// Returns `IpcError::ParseError` if JSON parsing fails.
/// Returns `IpcError::EmptyMessage` if an empty line is received.
pub fn read_ipc_request(stream: UnixStream) -> Result<(IpcRequest, UnixStream), IpcError> {
    // Set a short timeout to avoid blocking the compositor
    stream.set_read_timeout(Some(std::time::Duration::from_millis(100)))
        .map_err(|e| {
            if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut {
                IpcError::Timeout
            } else {
                IpcError::Io(e)
            }
        })?;

    let mut reader = BufReader::new(stream);

    // Read first line (JSON message)
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => return Err(IpcError::EmptyMessage),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => {
            return Err(IpcError::Timeout);
        }
        Err(e) => return Err(IpcError::Io(e)),
    }

    if line.trim().is_empty() {
        return Err(IpcError::EmptyMessage);
    }

    tracing::debug!(message = %line.trim(), "received IPC message");

    // Parse JSON
    let message: IpcMessage = serde_json::from_str(&line)?;

    // Get the stream back from the reader for ACK
    let stream = reader.into_inner();

    match message {
        IpcMessage::Spawn { command, cwd, env, foreground } => {
            let spawn_type = match foreground {
                None => "terminal",
                Some(true) => "gui (foreground)",
                Some(false) => "gui (background)",
            };
            tracing::info!(command = %command, cwd = %cwd, spawn_type, "spawn request received");
            Ok((IpcRequest::Spawn(SpawnRequest {
                command,
                cwd: PathBuf::from(cwd),
                env,
                foreground,
            }), stream))
        }
        IpcMessage::Resize { mode } => {
            tracing::info!(?mode, "resize request received");
            Ok((IpcRequest::Resize(mode), stream))
        }
    }
}

/// Send acknowledgement on a stream (for synchronous operations like resize)
pub fn send_ack(mut stream: UnixStream) {
    use std::io::Write;
    let _ = writeln!(stream, "ok");
    let _ = stream.flush();
}

/// Generate the IPC socket path for the current user
pub fn socket_path() -> PathBuf {
    let uid = rustix::process::getuid().as_raw();
    PathBuf::from(format!("/run/user/{}/termstack.sock", uid))
}
