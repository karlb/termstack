//! IPC socket for shell integration
//!
//! Handles spawn requests from the `column-term` CLI tool.
//! The compositor listens on a Unix socket and accepts JSON messages
//! to spawn terminals with specific commands and environments.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
    },
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
}

/// Read a spawn request from a Unix stream
///
/// Returns None if the message couldn't be parsed or wasn't a spawn request.
pub fn read_spawn_request(stream: UnixStream) -> Option<SpawnRequest> {
    // Set a short timeout to avoid blocking the compositor
    stream.set_read_timeout(Some(std::time::Duration::from_millis(100))).ok()?;

    let reader = BufReader::new(stream);

    // Read first line (JSON message)
    let mut lines = reader.lines();
    let line = lines.next()?.ok()?;

    tracing::debug!(message = %line, "received IPC message");

    // Parse JSON
    let message: IpcMessage = serde_json::from_str(&line).ok()?;

    match message {
        IpcMessage::Spawn { command, cwd, env } => {
            tracing::info!(command = %command, cwd = %cwd, "spawn request received");
            Some(SpawnRequest {
                command,
                cwd: PathBuf::from(cwd),
                env,
            })
        }
    }
}

/// Generate the IPC socket path for the current user
pub fn socket_path() -> PathBuf {
    let uid = rustix::process::getuid().as_raw();
    PathBuf::from(format!("/run/user/{}/column-compositor.sock", uid))
}
