//! Integration tests for GUI X11 environment configuration
//!
//! These tests verify that the compositor sets up the X11 environment correctly
//! for GUI apps (DISPLAY set, XAUTHORITY cleared).
//!
//! Requirements:
//! - Running X11 or Wayland display
//! - xwayland-satellite installed
//! - xdpyinfo available
//!
//! To run: cargo test -p test-harness --features gui-tests --test gui_x11

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
    PathBuf::from(format!("/run/user/{}/termstack.sock", uid))
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

/// Wait for XWayland to become ready and return DISPLAY value
fn wait_for_xwayland(compositor_pid: u32, timeout: Duration) -> Option<String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        let environ_path = format!("/proc/{}/environ", compositor_pid);
        if let Ok(environ_data) = std::fs::read(&environ_path) {
            let env_vars: Vec<&[u8]> = environ_data.split(|&b| b == 0).collect();
            for env_var in env_vars {
                if let Ok(s) = std::str::from_utf8(env_var) {
                    if s.starts_with("DISPLAY=") {
                        return Some(s["DISPLAY=".len()..].to_string());
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
    None
}

/// Check if XAUTHORITY is set in compositor's environment
fn check_xauthority_in_compositor(compositor_pid: u32) -> Option<String> {
    let environ_path = format!("/proc/{}/environ", compositor_pid);
    if let Ok(environ_data) = std::fs::read(&environ_path) {
        let env_vars: Vec<&[u8]> = environ_data.split(|&b| b == 0).collect();
        for env_var in env_vars {
            if let Ok(s) = std::str::from_utf8(env_var) {
                if s.starts_with("XAUTHORITY=") {
                    return Some(s["XAUTHORITY=".len()..].to_string());
                }
            }
        }
    }
    None
}

/// Find the workspace root
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

/// Clean up any leftover sockets from previous runs
fn cleanup_sockets() {
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

#[test]
#[ignore] // Run manually with: cargo test -p test-harness --features gui-tests --test gui_x11 -- --ignored
fn test_xauthority_not_set_in_compositor() {
    // Check for xwayland-satellite
    if Command::new("which")
        .arg("xwayland-satellite")
        .stdout(Stdio::null())
        .status()
        .map(|s| !s.success())
        .unwrap_or(true)
    {
        eprintln!("Skipping: xwayland-satellite not found");
        return;
    }

    // Check for display
    let display = env::var("DISPLAY").or_else(|_| env::var("WAYLAND_DISPLAY"));
    if display.is_err() {
        eprintln!("Skipping: No display available (DISPLAY or WAYLAND_DISPLAY)");
        return;
    }

    cleanup_sockets();

    // Build compositor
    let workspace_root = find_workspace_root();
    let build_status = Command::new("cargo")
        .args(["build", "-p", "termstack"])
        .current_dir(&workspace_root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("Failed to run cargo build");

    assert!(build_status.success(), "Compositor build failed");

    let compositor_bin = workspace_root.join("target/debug/termstack");

    // Start compositor
    let mut compositor = Command::new(&compositor_bin)
        .env("RUST_LOG", "compositor=info")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start compositor");

    let compositor_pid = compositor.id();

    // Wait for socket
    assert!(
        wait_for_socket(Duration::from_secs(10)),
        "Compositor IPC socket did not become available"
    );

    // Wait for XWayland
    let x_display = wait_for_xwayland(compositor_pid, Duration::from_secs(10));
    assert!(x_display.is_some(), "XWayland did not start");

    // Check that XAUTHORITY IS set in compositor environment (for GTK apps)
    let xauthority = check_xauthority_in_compositor(compositor_pid);
    assert!(
        xauthority.is_some(),
        "XAUTHORITY should be set in compositor for GTK app support"
    );
    // Verify it points to termstack-xauth
    let xauth_path = xauthority.unwrap();
    assert!(
        xauth_path.contains("termstack-xauth"),
        "XAUTHORITY should point to termstack-xauth, got: {}",
        xauth_path
    );

    // Clean up
    compositor.kill().ok();
    compositor.wait().ok();
    cleanup_sockets();
}

#[test]
#[ignore]
fn test_x11_connection_without_auth() {
    // Check for xdpyinfo
    if Command::new("which")
        .arg("xdpyinfo")
        .stdout(Stdio::null())
        .status()
        .map(|s| !s.success())
        .unwrap_or(true)
    {
        eprintln!("Skipping: xdpyinfo not found");
        return;
    }

    // Check for xwayland-satellite
    if Command::new("which")
        .arg("xwayland-satellite")
        .stdout(Stdio::null())
        .status()
        .map(|s| !s.success())
        .unwrap_or(true)
    {
        eprintln!("Skipping: xwayland-satellite not found");
        return;
    }

    let display = env::var("DISPLAY").or_else(|_| env::var("WAYLAND_DISPLAY"));
    if display.is_err() {
        eprintln!("Skipping: No display available");
        return;
    }

    cleanup_sockets();

    let workspace_root = find_workspace_root();
    let build_status = Command::new("cargo")
        .args(["build", "-p", "termstack"])
        .current_dir(&workspace_root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("Failed to run cargo build");

    assert!(build_status.success(), "Compositor build failed");

    let compositor_bin = workspace_root.join("target/debug/termstack");

    let mut compositor = Command::new(&compositor_bin)
        .env("RUST_LOG", "compositor=info")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start compositor");

    let compositor_pid = compositor.id();

    assert!(wait_for_socket(Duration::from_secs(10)));

    let x_display = wait_for_xwayland(compositor_pid, Duration::from_secs(10));
    assert!(x_display.is_some(), "XWayland did not start");
    let x_display = x_display.unwrap();

    // Give xwayland-satellite time to start
    thread::sleep(Duration::from_millis(500));

    // Get XAUTHORITY from compositor environment
    let xauthority = check_xauthority_in_compositor(compositor_pid);

    // Test X11 connection WITH our xauth file
    let mut cmd = Command::new("xdpyinfo");
    cmd.env("DISPLAY", &x_display);
    if let Some(ref xa) = xauthority {
        cmd.env("XAUTHORITY", xa);
    }
    let xdpyinfo_result = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    let success = xdpyinfo_result.map(|s| s.success()).unwrap_or(false);

    // Clean up
    compositor.kill().ok();
    compositor.wait().ok();
    cleanup_sockets();

    assert!(
        success,
        "xdpyinfo should succeed without XAUTHORITY on display {}",
        x_display
    );
}
