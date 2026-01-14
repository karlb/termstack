//! Live compositor testing infrastructure
//!
//! This module provides helpers for running tests against a real compositor
//! with actual Wayland clients (like foot terminal).
//!
//! # Requirements
//!
//! These tests require a display server (X11 via DISPLAY or Wayland via WAYLAND_DISPLAY).
//! They are marked `#[ignore]` by default and can be run with:
//!
//! ```bash
//! cargo test -- --ignored
//! ```
//!
//! # Display Detection
//!
//! Use `display_available()` to check if tests can run:
//!
//! ```ignore
//! #[test]
//! #[ignore = "requires display"]
//! fn my_live_test() {
//!     if !live::display_available() {
//!         eprintln!("Skipping: no display available");
//!         return;
//!     }
//!     // ... test code
//! }
//! ```

use std::env;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

/// Check if a display is available for testing
///
/// Returns true if either DISPLAY (X11) or WAYLAND_DISPLAY is set.
/// This is used to determine if live compositor tests can run.
pub fn display_available() -> bool {
    env::var("DISPLAY").is_ok() || env::var("WAYLAND_DISPLAY").is_ok()
}

/// Check if the X11 backend should be preferred
///
/// Returns true if DISPLAY is set. This is useful because the winit
/// backend works more reliably with X11 for testing.
pub fn prefer_x11() -> bool {
    env::var("DISPLAY").is_ok()
}

/// Get the display type that's available
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayType {
    X11,
    Wayland,
    None,
}

/// Detect which display type is available
pub fn detect_display() -> DisplayType {
    if env::var("DISPLAY").is_ok() {
        DisplayType::X11
    } else if env::var("WAYLAND_DISPLAY").is_ok() {
        DisplayType::Wayland
    } else {
        DisplayType::None
    }
}

/// Environment configuration for running the compositor in tests
pub struct TestEnvironment {
    /// Original environment variables to restore after test
    _saved_env: Vec<(String, Option<String>)>,
}

impl TestEnvironment {
    /// Create a new test environment
    ///
    /// Sets up environment variables for running the compositor.
    /// Variables are restored when the TestEnvironment is dropped.
    pub fn new() -> Self {
        let mut saved = Vec::new();

        // Save and set WINIT_UNIX_BACKEND to x11 if DISPLAY is available
        // This makes winit use X11 which works better for testing
        if prefer_x11() {
            let key = "WINIT_UNIX_BACKEND";
            saved.push((key.to_string(), env::var(key).ok()));
            env::set_var(key, "x11");
        }

        Self { _saved_env: saved }
    }
}

impl Default for TestEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TestEnvironment {
    fn drop(&mut self) {
        // Restore saved environment variables
        for (key, value) in &self._saved_env {
            match value {
                Some(v) => env::set_var(key, v),
                None => env::remove_var(key),
            }
        }
    }
}

// ============================================================================
// Compositor process utilities for integration tests
// ============================================================================

/// Get the IPC socket path for the test compositor
pub fn ipc_socket_path() -> PathBuf {
    let uid = rustix::process::getuid().as_raw();
    PathBuf::from(format!("/run/user/{}/termstack.sock", uid))
}

/// Wait for the compositor's IPC socket to become available
pub fn wait_for_socket(timeout: Duration) -> bool {
    let socket_path = ipc_socket_path();
    let start = Instant::now();
    while start.elapsed() < timeout {
        if socket_path.exists() && UnixStream::connect(&socket_path).is_ok() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Wait for XWayland to become ready and return DISPLAY value
///
/// Reads the compositor's environment from /proc to detect when XWayland starts.
pub fn wait_for_xwayland(compositor_pid: u32, timeout: Duration) -> Option<String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let environ_path = format!("/proc/{}/environ", compositor_pid);
        if let Ok(environ_data) = std::fs::read(&environ_path) {
            let env_vars: Vec<&[u8]> = environ_data.split(|&b| b == 0).collect();
            for env_var in env_vars {
                if let Ok(s) = std::str::from_utf8(env_var) {
                    if let Some(display) = s.strip_prefix("DISPLAY=") {
                        return Some(display.to_string());
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
    None
}

/// Find the workspace root by looking for Cargo.toml with [workspace]
pub fn find_workspace_root() -> PathBuf {
    let mut dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
                if content.contains("[workspace]") {
                    return dir;
                }
            }
        }
        if !dir.pop() {
            return env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        }
    }
}

/// Clean up any leftover sockets from previous runs
pub fn cleanup_sockets() {
    let socket_path = ipc_socket_path();
    let _ = std::fs::remove_file(&socket_path);

    // Also clean up wayland sockets
    let uid = rustix::process::getuid().as_raw();
    let runtime_dir = format!("/run/user/{}", uid);
    for i in 0..10 {
        let _ = std::fs::remove_file(format!("{}/wayland-{}", runtime_dir, i));
        let _ = std::fs::remove_file(format!("{}/wayland-{}.lock", runtime_dir, i));
    }
}

/// Check if an environment variable is set in a process's environment
///
/// Reads from /proc/{pid}/environ and returns the value if found.
pub fn get_env_from_process(pid: u32, var_name: &str) -> Option<String> {
    let environ_path = format!("/proc/{}/environ", pid);
    if let Ok(environ_data) = std::fs::read(&environ_path) {
        let prefix = format!("{}=", var_name);
        let env_vars: Vec<&[u8]> = environ_data.split(|&b| b == 0).collect();
        for env_var in env_vars {
            if let Ok(s) = std::str::from_utf8(env_var) {
                if s.starts_with(&prefix) {
                    return Some(s[prefix.len()..].to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_display_works() {
        // Just verify this doesn't panic
        let display = detect_display();
        match display {
            DisplayType::X11 => println!("X11 display detected"),
            DisplayType::Wayland => println!("Wayland display detected"),
            DisplayType::None => println!("No display detected"),
        }
    }
}
