//! Integration tests for external Wayland applications
//!
//! These tests verify that external apps can connect to and run inside the compositor.
//! Requires a running X11 display (Xvfb doesn't work due to missing DRI3 support).
//!
//! To run: cargo test -p test-harness --features gui-tests --test external_apps

#![cfg(feature = "gui-tests")]

use std::io::Read;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{env, thread};

/// Get the IPC socket path for the test compositor
fn ipc_socket_path() -> PathBuf {
    let uid = rustix::process::getuid().as_raw();
    PathBuf::from(format!("/run/user/{}/column-compositor.sock", uid))
}

/// Wait for the compositor's IPC socket to become available
fn wait_for_socket(timeout: Duration) -> bool {
    let socket_path = ipc_socket_path();
    let start = Instant::now();
    while start.elapsed() < timeout {
        if socket_path.exists() {
            if UnixStream::connect(&socket_path).is_ok() {
                return true;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Find the workspace root by looking for Cargo.toml with [workspace]
fn find_workspace_root() -> PathBuf {
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

/// Test that foot terminal connects to the compositor successfully
#[test]
fn test_foot_connects() {
    // Check for foot
    if Command::new("which")
        .arg("foot")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("Skipping: foot not installed");
        return;
    }

    // Require DISPLAY (Xvfb doesn't work - Smithay X11 backend needs DRI3)
    let display = match env::var("DISPLAY") {
        Ok(d) => d,
        Err(_) => {
            eprintln!("Skipping: DISPLAY not set (Xvfb not supported due to DRI3 requirement)");
            return;
        }
    };
    eprintln!("Using DISPLAY={}", display);

    let workspace_root = find_workspace_root();

    // Build compositor
    let status = Command::new("cargo")
        .args(["build", "--release", "-p", "compositor", "--bin", "column-compositor"])
        .current_dir(&workspace_root)
        .status()
        .expect("Failed to run cargo build");

    assert!(status.success(), "Compositor build failed");

    let uid = rustix::process::getuid().as_raw();

    // Clean up sockets from previous runs
    let socket_path = ipc_socket_path();
    let _ = std::fs::remove_file(&socket_path);
    let wayland_socket = format!("/run/user/{}/wayland-1", uid);
    let wayland_lock = format!("/run/user/{}/wayland-1.lock", uid);
    let _ = std::fs::remove_file(&wayland_socket);
    let _ = std::fs::remove_file(&wayland_lock);

    // Start compositor
    let compositor_bin = workspace_root.join("target/release/column-compositor");
    let mut compositor = Command::new(&compositor_bin)
        .env("DISPLAY", &display)
        .env("RUST_LOG", "compositor=info")
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start compositor");

    // Wait for compositor socket
    if !wait_for_socket(Duration::from_secs(10)) {
        let mut stderr_output = String::new();
        if let Some(mut stderr) = compositor.stderr.take() {
            stderr.read_to_string(&mut stderr_output).ok();
        }
        compositor.kill().ok();
        compositor.wait().ok();
        panic!(
            "Compositor socket not ready after 10s\nstderr:\n{}",
            stderr_output
        );
    }
    thread::sleep(Duration::from_millis(500));

    // Run foot with compositor's display
    let mut foot = Command::new("foot")
        .args(["-e", "sh", "-c", "echo test && sleep 1"])
        .env_clear()
        .env("WAYLAND_DISPLAY", "wayland-1")
        .env("XDG_RUNTIME_DIR", format!("/run/user/{}", uid))
        .env("HOME", env::var("HOME").unwrap_or_default())
        .env("PATH", env::var("PATH").unwrap_or_default())
        .env("SHELL", "/bin/sh")
        .env("TERM", "xterm-256color")
        .env("LANG", "C.UTF-8")
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to start foot");

    thread::sleep(Duration::from_secs(2));

    // Check if foot ran
    match foot.try_wait() {
        Ok(Some(status)) => {
            let mut stderr_output = String::new();
            if let Some(mut stderr) = foot.stderr.take() {
                stderr.read_to_string(&mut stderr_output).ok();
            }
            if !status.success() {
                eprintln!("foot stderr: {}", stderr_output);
            }
            assert!(status.success(), "foot failed: {}", stderr_output);
        }
        Ok(None) => {
            // Still running, that's fine - kill it
            foot.kill().ok();
            foot.wait().ok();
        }
        Err(e) => {
            foot.kill().ok();
            panic!("Error checking foot status: {}", e);
        }
    }

    // Clean up compositor
    compositor.kill().ok();
    compositor.wait().ok();
}
