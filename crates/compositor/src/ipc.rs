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

/// Maximum IPC message size (1 MB)
const MAX_IPC_MESSAGE_SIZE: usize = 1024 * 1024;

/// Maximum number of environment variables in a spawn request
const MAX_ENV_VARS: usize = 1000;

/// Maximum size of a single environment variable (key + value)
const MAX_ENV_VAR_SIZE: usize = 1024 * 10; // 10 KB

/// Maximum command string size
const MAX_COMMAND_SIZE: usize = 1024 * 10; // 10 KB

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

    /// Message too large
    #[error("message too large: {size} bytes (max {max})")]
    MessageTooLarge { size: usize, max: usize },

    /// Validation error
    #[error("validation error: {0}")]
    ValidationError(String),
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
    // Set a short timeout to avoid blocking the compositor.
    // On macOS, set_read_timeout returns EINVAL on socket pairs when the peer
    // has already disconnected — ignore that since reads will return EOF anyway.
    if let Err(e) = stream.set_read_timeout(Some(std::time::Duration::from_millis(100))) {
        match e.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => return Err(IpcError::Timeout),
            io::ErrorKind::InvalidInput => {} // macOS: peer already gone, reads won't block
            _ => return Err(IpcError::Io(e)),
        }
    }

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

    // Check message size before parsing
    if line.len() > MAX_IPC_MESSAGE_SIZE {
        return Err(IpcError::MessageTooLarge {
            size: line.len(),
            max: MAX_IPC_MESSAGE_SIZE,
        });
    }

    tracing::debug!(message = %line.trim(), "received IPC message");

    // Parse JSON
    let message: IpcMessage = serde_json::from_str(&line)?;

    // Get the stream back from the reader for ACK
    let stream = reader.into_inner();

    match message {
        IpcMessage::Spawn { prompt, command, cwd, env, foreground } => {
            // Validate spawn request fields
            if command.len() > MAX_COMMAND_SIZE {
                return Err(IpcError::ValidationError(format!(
                    "command too large: {} bytes (max {})", command.len(), MAX_COMMAND_SIZE
                )));
            }
            if env.len() > MAX_ENV_VARS {
                return Err(IpcError::ValidationError(format!(
                    "too many environment variables: {} (max {})", env.len(), MAX_ENV_VARS
                )));
            }
            for (key, value) in &env {
                let total = key.len() + value.len();
                if total > MAX_ENV_VAR_SIZE {
                    return Err(IpcError::ValidationError(format!(
                        "environment variable too large: {}={} ({} bytes, max {})",
                        key, &value[..value.len().min(20)], total, MAX_ENV_VAR_SIZE
                    )));
                }
            }

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
///
/// Returns an error if the ACK cannot be sent within the timeout period.
/// This prevents hanging when the client has disconnected.
pub fn send_ack(mut stream: UnixStream) -> Result<(), IpcError> {
    // Set a timeout for writing the ACK
    stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))
        .map_err(|e| {
            tracing::warn!(error = ?e, "Failed to set write timeout for ACK");
            IpcError::Io(e)
        })?;

    writeln!(stream, "ok")
        .map_err(|e| {
            tracing::warn!(error = ?e, "Failed to write ACK to IPC stream");
            IpcError::Io(e)
        })?;

    stream.flush()
        .map_err(|e| {
            tracing::warn!(error = ?e, "Failed to flush ACK to IPC stream");
            IpcError::Io(e)
        })?;

    tracing::debug!("ACK sent successfully");
    Ok(())
}

/// Send a JSON response on a stream (for query operations)
///
/// Returns an error if the response cannot be sent within the timeout period.
pub fn send_json_response<T: Serialize>(mut stream: UnixStream, data: &T) -> Result<(), IpcError> {
    // Set a timeout for writing the response
    stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))
        .map_err(|e| {
            tracing::warn!(error = ?e, "Failed to set write timeout for JSON response");
            IpcError::Io(e)
        })?;

    let json = serde_json::to_string(data)?;

    writeln!(stream, "{}", json)
        .map_err(|e| {
            tracing::warn!(error = ?e, "Failed to write JSON response to IPC stream");
            IpcError::Io(e)
        })?;

    stream.flush()
        .map_err(|e| {
            tracing::warn!(error = ?e, "Failed to flush JSON response to IPC stream");
            IpcError::Io(e)
        })?;

    tracing::debug!("JSON response sent successfully");
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    /// Helper to create a connected pair for testing
    fn ipc_pair() -> (UnixStream, UnixStream) {
        UnixStream::pair().expect("failed to create socket pair")
    }

    /// Helper to send a message and read the IPC request.
    /// Uses a thread for writing to avoid deadlock on macOS where Unix socket
    /// buffers are ~8KB (large messages would block both writer and reader).
    fn send_and_read(message: &str) -> Result<IpcRequest, IpcError> {
        let (client, server) = ipc_pair();
        let msg = message.to_string();
        std::thread::spawn(move || {
            let mut client = client;
            let _ = writeln!(client, "{}", msg);
        });
        read_ipc_request(server).map(|(req, _)| req)
    }

    #[test]
    fn parse_valid_spawn_request() {
        let msg = r#"{"type":"spawn","prompt":"$ ","command":"ls","cwd":"/tmp","env":{}}"#;
        let req = send_and_read(msg).unwrap();
        assert!(matches!(req, IpcRequest::Spawn(_)));
    }

    #[test]
    fn parse_valid_resize_request() {
        let msg = r#"{"type":"resize","mode":"full"}"#;
        let req = send_and_read(msg).unwrap();
        assert!(matches!(req, IpcRequest::Resize(ResizeMode::Full)));
    }

    #[test]
    fn parse_valid_builtin_request() {
        let msg = r#"{"type":"builtin","prompt":"$ ","command":"cd ..","result":"","success":true}"#;
        let req = send_and_read(msg).unwrap();
        assert!(matches!(req, IpcRequest::Builtin(_)));
    }

    #[test]
    fn parse_valid_query_windows_request() {
        let msg = r#"{"type":"query_windows"}"#;
        let req = send_and_read(msg).unwrap();
        assert!(matches!(req, IpcRequest::QueryWindows));
    }

    #[test]
    fn reject_empty_message() {
        let result = send_and_read("");
        assert!(matches!(result, Err(IpcError::EmptyMessage)));
    }

    #[test]
    fn reject_invalid_json() {
        let result = send_and_read("not json at all");
        assert!(matches!(result, Err(IpcError::ParseError(_))));
    }

    #[test]
    fn reject_oversized_message() {
        let (client, server) = ipc_pair();
        // Spawn writer thread because >1MB write blocks on socket buffer
        std::thread::spawn(move || {
            let mut client = client;
            let huge = "x".repeat(MAX_IPC_MESSAGE_SIZE + 100);
            let _ = writeln!(client, "{}", huge);
        });
        let result = read_ipc_request(server);
        assert!(matches!(result, Err(IpcError::MessageTooLarge { .. })));
    }

    #[test]
    fn reject_command_too_large() {
        let big_command = "x".repeat(MAX_COMMAND_SIZE + 1);
        let msg = format!(
            r#"{{"type":"spawn","prompt":"","command":"{}","cwd":"/tmp","env":{{}}}}"#,
            big_command
        );
        let result = send_and_read(&msg);
        assert!(matches!(result, Err(IpcError::ValidationError(_))));
    }

    #[test]
    fn reject_too_many_env_vars() {
        let mut env_entries = Vec::new();
        for i in 0..=MAX_ENV_VARS {
            env_entries.push(format!(r#""K{}":"V{}""#, i, i));
        }
        let env_str = format!("{{{}}}", env_entries.join(","));
        let msg = format!(
            r#"{{"type":"spawn","prompt":"","command":"ls","cwd":"/tmp","env":{}}}"#,
            env_str
        );
        let result = send_and_read(&msg);
        assert!(matches!(result, Err(IpcError::ValidationError(_))));
    }

    #[test]
    fn reject_env_var_too_large() {
        let big_value = "x".repeat(MAX_ENV_VAR_SIZE + 1);
        let msg = format!(
            r#"{{"type":"spawn","prompt":"","command":"ls","cwd":"/tmp","env":{{"KEY":"{}"}}}}"#,
            big_value
        );
        let result = send_and_read(&msg);
        assert!(matches!(result, Err(IpcError::ValidationError(_))));
    }

    #[test]
    fn send_ack_works_on_connected_stream() {
        let (_client, server) = ipc_pair();
        let result = send_ack(server);
        assert!(result.is_ok());
    }

    #[test]
    fn send_json_response_works() {
        let (_client, server) = ipc_pair();
        let data = vec![WindowInfo {
            index: 0,
            width: 100,
            height: 50,
            is_external: false,
            command: "test".to_string(),
        }];
        let result = send_json_response(server, &data);
        assert!(result.is_ok());
    }

    #[test]
    fn concurrent_ipc_requests_all_parsed() {
        // Simulate multiple clients sending IPC messages concurrently.
        // Each client gets its own socket pair, mirroring real usage where
        // the compositor accepts separate connections per client.
        let count = 50;
        let handles: Vec<_> = (0..count)
            .map(|i| {
                std::thread::spawn(move || {
                    let (client, server) = UnixStream::pair().unwrap();
                    let mut client = client;
                    let msg = format!(
                        r#"{{"type":"spawn","prompt":"","command":"cmd_{}","cwd":"/tmp","env":{{}}}}"#,
                        i
                    );
                    writeln!(client, "{}", msg).unwrap();
                    drop(client);
                    read_ipc_request(server).map(|(req, _)| req)
                })
            })
            .collect();

        let mut success_count = 0;
        for handle in handles {
            let result = handle.join().expect("thread panicked");
            assert!(result.is_ok(), "IPC parse failed: {:?}", result.err());
            success_count += 1;
        }
        assert_eq!(success_count, count);
    }

    #[test]
    fn send_ack_to_disconnected_client_returns_error() {
        let (client, server) = ipc_pair();
        drop(client); // Disconnect before sending ACK
        let result = send_ack(server);
        assert!(result.is_err());
    }

    #[test]
    fn spawn_request_preserves_fields() {
        let msg = r#"{"type":"spawn","prompt":"user@host> ","command":"vim file.txt","cwd":"/home/user","env":{"TERM":"xterm-256color"},"foreground":true}"#;
        let req = send_and_read(msg).unwrap();
        match req {
            IpcRequest::Spawn(spawn) => {
                assert_eq!(spawn.prompt, "user@host> ");
                assert_eq!(spawn.command, "vim file.txt");
                assert_eq!(spawn.cwd, PathBuf::from("/home/user"));
                assert_eq!(spawn.env.get("TERM").unwrap(), "xterm-256color");
                assert_eq!(spawn.foreground, Some(true));
            }
            _ => panic!("expected Spawn request"),
        }
    }

    #[test]
    fn builtin_request_preserves_fields() {
        let msg = r#"{"type":"builtin","prompt":"$ ","command":"export FOO=bar","result":"","success":true}"#;
        let req = send_and_read(msg).unwrap();
        match req {
            IpcRequest::Builtin(builtin) => {
                assert_eq!(builtin.prompt, "$ ");
                assert_eq!(builtin.command, "export FOO=bar");
                assert_eq!(builtin.result, "");
                assert!(builtin.success);
            }
            _ => panic!("expected Builtin request"),
        }
    }
}
