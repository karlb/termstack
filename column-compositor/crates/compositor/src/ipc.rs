//! IPC socket for shell integration
//!
//! Handles requests from the `column-term` CLI tool.
//! The compositor listens on a Unix socket and accepts JSON messages
//! to spawn terminals or resize the focused terminal.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Resize mode for terminals
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResizeMode {
    /// Full viewport height (for TUI apps)
    Full,
    /// Content-based height (normal mode)
    Content,
}

/// Message from column-term to compositor
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcMessage {
    /// Spawn a new terminal with a specific command
    #[serde(rename = "spawn")]
    Spawn {
        /// Command to execute (passed to /bin/sh -c)
        command: String,
        /// Working directory for the command
        cwd: String,
        /// Environment variables to inherit
        env: HashMap<String, String>,
        /// Whether this is a TUI app (pre-resize optimization).
        ///
        /// When true:
        /// - Terminal starts at full viewport height
        /// - Command is run without echo prefix
        ///
        /// Note: Even if false, terminals auto-resize to full height when
        /// alternate screen mode is detected (see main.rs auto-resize logic).
        /// This flag provides proactive pre-sizing for known TUI apps.
        #[serde(default)]
        is_tui: bool,
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
    /// Spawn a new terminal
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
    /// Whether this is a TUI app (pre-resize optimization).
    /// See IpcMessage::Spawn::is_tui for details.
    pub is_tui: bool,
}

/// Read a request from a Unix stream
///
/// Returns the parsed request and the stream for sending acknowledgement.
/// For Resize requests, caller MUST send ACK to avoid race conditions.
pub fn read_ipc_request(stream: UnixStream) -> Option<(IpcRequest, UnixStream)> {
    // Set a short timeout to avoid blocking the compositor
    stream.set_read_timeout(Some(std::time::Duration::from_millis(100))).ok()?;

    let mut reader = BufReader::new(stream);

    // Read first line (JSON message)
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;

    tracing::debug!(message = %line.trim(), "received IPC message");

    // Parse JSON
    let message: IpcMessage = serde_json::from_str(&line).ok()?;

    // Get the stream back from the reader for ACK
    let stream = reader.into_inner();

    match message {
        IpcMessage::Spawn { command, cwd, env, is_tui } => {
            tracing::info!(command = %command, cwd = %cwd, is_tui, "spawn request received");
            Some((IpcRequest::Spawn(SpawnRequest {
                command,
                cwd: PathBuf::from(cwd),
                env,
                is_tui,
            }), stream))
        }
        IpcMessage::Resize { mode } => {
            tracing::info!(?mode, "resize request received");
            Some((IpcRequest::Resize(mode), stream))
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
    PathBuf::from(format!("/run/user/{}/column-compositor.sock", uid))
}
