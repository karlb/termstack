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
use std::io::Write;
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
        /// The shell prompt at command entry time (e.g., "karl@host ~/code> ")
        #[serde(default)]
        prompt: String,
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
    /// Shell builtin executed (creates persistent entry in stack)
    #[serde(rename = "builtin")]
    Builtin {
        /// The shell prompt at command entry time (e.g., "karl@host ~/code> ")
        prompt: String,
        /// The builtin command (e.g., "cd ..") - may be empty for Enter-only
        command: String,
        /// The output/result (may be empty for commands like cd)
        result: String,
        /// Whether the command succeeded
        success: bool,
    },
    /// Query current window state (for testing/debugging)
    #[serde(rename = "query_windows")]
    QueryWindows,
}

/// Information about a window in the compositor (for IPC responses)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Index in layout_nodes
    pub index: usize,
    /// Actual window width from geometry (may differ from output width if app didn't resize)
    pub width: i32,
    /// Window height including title bar
    pub height: i32,
    /// Whether this is an external (Wayland client) window
    pub is_external: bool,
    /// Command that spawned this window (for external windows)
    pub command: String,
}

/// Request ready for processing by the compositor
#[derive(Debug)]
pub enum IpcRequest {
    /// Spawn a new terminal or GUI window
    Spawn(SpawnRequest),
    /// Resize the focused terminal
    Resize(ResizeMode),
    /// Shell builtin executed (creates persistent entry in stack)
    Builtin(BuiltinRequest),
    /// Query current window state (for testing/debugging)
    QueryWindows,
}

/// Builtin command request ready for processing by the compositor
#[derive(Debug)]
pub struct BuiltinRequest {
    /// The shell prompt at command entry time (e.g., "karl@host ~/code> ")
    pub prompt: String,
    /// The builtin command (e.g., "cd ..") - may be empty for Enter-only
    pub command: String,
    /// The output/result (may be empty for commands like cd)
    pub result: String,
    /// Whether the command succeeded
    pub success: bool,
}

/// Spawn request ready for processing by the compositor
#[derive(Debug)]
pub struct SpawnRequest {
    /// The shell prompt at command entry time (e.g., "karl@host ~/code> ")
    pub prompt: String,
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
        IpcMessage::Spawn { prompt, command, cwd, env, foreground } => {
            let spawn_type = match foreground {
                None => "terminal",
                Some(true) => "gui (foreground)",
                Some(false) => "gui (background)",
            };
            tracing::info!(command = %command, cwd = %cwd, spawn_type, "spawn request received");
            Ok((IpcRequest::Spawn(SpawnRequest {
                prompt,
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
        IpcMessage::Builtin { prompt, command, result, success } => {
            tracing::info!(command = %command, success, has_result = !result.is_empty(), "builtin request received");
            Ok((IpcRequest::Builtin(BuiltinRequest {
                prompt,
                command,
                result,
                success,
            }), stream))
        }
        IpcMessage::QueryWindows => {
            tracing::info!("query_windows request received");
            Ok((IpcRequest::QueryWindows, stream))
        }
    }
}

/// Send acknowledgement on a stream (for synchronous operations like resize)
pub fn send_ack(mut stream: UnixStream) {
    let _ = writeln!(stream, "ok");
    let _ = stream.flush();
}

/// Send a JSON response on a stream (for query operations)
pub fn send_json_response<T: Serialize>(mut stream: UnixStream, data: &T) {
    if let Ok(json) = serde_json::to_string(data) {
        let _ = writeln!(stream, "{}", json);
        let _ = stream.flush();
    }
}

/// Generate the IPC socket path for the current user
///
/// Checks TERMSTACK_IPC_SOCKET environment variable first (for testing),
/// otherwise uses the default path.
pub fn socket_path() -> PathBuf {
    if let Ok(path) = std::env::var("TERMSTACK_IPC_SOCKET") {
        return PathBuf::from(path);
    }
    let uid = rustix::process::getuid().as_raw();
    PathBuf::from(format!("/run/user/{}/termstack.sock", uid))
}
