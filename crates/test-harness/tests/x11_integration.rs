//! Integration tests for X11 application support via xwayland-satellite
//!
//! These tests verify that X11 applications can connect and run correctly
//! through XWayland + xwayland-satellite integration.
//!
//! Requirements:
//! - Running X11 display (DISPLAY set)
//! - xwayland-satellite installed
//! - X11 test apps (xeyes, xclock, etc.)
//!
//! To run: cargo test -p test-harness --features gui-tests --test x11_integration

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

/// Wait for XWayland to become ready by checking DISPLAY environment
fn wait_for_xwayland(compositor_pid: u32, timeout: Duration) -> Option<String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        // Check compositor's environment for DISPLAY
        let environ_path = format!("/proc/{}/environ", compositor_pid);
        if let Ok(environ_data) = std::fs::read(&environ_path) {
            // Parse null-terminated environment variables
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

/// Test that xeyes X11 app launches and connects successfully
#[test]
fn test_xeyes_launches() {
    // Check for xeyes
    if Command::new("which")
        .arg("xeyes")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("Skipping: xeyes not installed");
        return;
    }

    // Check for xwayland-satellite
    if Command::new("which")
        .arg("xwayland-satellite")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("Skipping: xwayland-satellite not installed");
        eprintln!("Install from: https://github.com/Supreeeme/xwayland-satellite");
        return;
    }

    // Require DISPLAY (for compositor's X11 backend)
    let display = match env::var("DISPLAY") {
        Ok(d) => d,
        Err(_) => {
            eprintln!("Skipping: DISPLAY not set");
            return;
        }
    };
    eprintln!("Using DISPLAY={}", display);

    let workspace_root = find_workspace_root();

    // Build compositor
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "-p",
            "compositor",
            "--bin",
            "column-compositor",
        ])
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

    let compositor_pid = compositor.id();

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

    // Wait for XWayland to become ready (DISPLAY set in compositor environment)
    let x_display = match wait_for_xwayland(compositor_pid, Duration::from_secs(15)) {
        Some(d) => d,
        None => {
            let mut stderr_output = String::new();
            if let Some(mut stderr) = compositor.stderr.take() {
                stderr.read_to_string(&mut stderr_output).ok();
            }
            compositor.kill().ok();
            compositor.wait().ok();
            eprintln!("Compositor stderr:\n{}", stderr_output);
            panic!("XWayland did not become ready after 15s - check if xwayland-satellite is working");
        }
    };
    eprintln!("XWayland ready on {}", x_display);

    thread::sleep(Duration::from_secs(1));

    // Run xeyes with XWayland's display
    let mut xeyes = Command::new("xeyes")
        .env("DISPLAY", x_display)
        .env("XDG_RUNTIME_DIR", format!("/run/user/{}", uid))
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start xeyes");

    // Let xeyes run for a bit
    thread::sleep(Duration::from_secs(2));

    // Check if xeyes is still running (should be)
    match xeyes.try_wait() {
        Ok(Some(status)) => {
            let mut stderr_output = String::new();
            if let Some(mut stderr) = xeyes.stderr.take() {
                stderr.read_to_string(&mut stderr_output).ok();
            }
            compositor.kill().ok();
            compositor.wait().ok();
            panic!(
                "xeyes exited early with status {}: {}",
                status, stderr_output
            );
        }
        Ok(None) => {
            // Still running - good!
            eprintln!("xeyes is running successfully");
        }
        Err(e) => {
            compositor.kill().ok();
            compositor.wait().ok();
            panic!("Error checking xeyes status: {}", e);
        }
    }

    // Clean up
    xeyes.kill().ok();
    xeyes.wait().ok();
    compositor.kill().ok();
    compositor.wait().ok();

    eprintln!("Test passed: X11 app (xeyes) launched and connected successfully");
}

/// Test that multiple X11 apps can run simultaneously
#[test]
fn test_multiple_x11_apps() {
    // Check for xeyes and xclock
    let has_xeyes = Command::new("which")
        .arg("xeyes")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let has_xclock = Command::new("which")
        .arg("xclock")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !has_xeyes || !has_xclock {
        eprintln!("Skipping: xeyes or xclock not installed");
        return;
    }

    // Check for xwayland-satellite
    if Command::new("which")
        .arg("xwayland-satellite")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("Skipping: xwayland-satellite not installed");
        return;
    }

    // Require DISPLAY
    let display = match env::var("DISPLAY") {
        Ok(d) => d,
        Err(_) => {
            eprintln!("Skipping: DISPLAY not set");
            return;
        }
    };

    let workspace_root = find_workspace_root();

    // Build compositor
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "-p",
            "compositor",
            "--bin",
            "column-compositor",
        ])
        .current_dir(&workspace_root)
        .status()
        .expect("Failed to run cargo build");

    assert!(status.success(), "Compositor build failed");

    let uid = rustix::process::getuid().as_raw();

    // Clean up sockets
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

    let compositor_pid = compositor.id();

    // Wait for compositor and XWayland
    if !wait_for_socket(Duration::from_secs(10)) {
        compositor.kill().ok();
        compositor.wait().ok();
        panic!("Compositor socket not ready");
    }

    let x_display = match wait_for_xwayland(compositor_pid, Duration::from_secs(15)) {
        Some(d) => d,
        None => {
            compositor.kill().ok();
            compositor.wait().ok();
            panic!("XWayland not ready");
        }
    };

    thread::sleep(Duration::from_secs(1));

    // Launch multiple X11 apps
    let mut xeyes = Command::new("xeyes")
        .env("DISPLAY", &x_display)
        .env("XDG_RUNTIME_DIR", format!("/run/user/{}", uid))
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start xeyes");

    thread::sleep(Duration::from_millis(500));

    let mut xclock = Command::new("xclock")
        .env("DISPLAY", &x_display)
        .env("XDG_RUNTIME_DIR", format!("/run/user/{}", uid))
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start xclock");

    // Let them both run
    thread::sleep(Duration::from_secs(2));

    // Both should still be running
    match xeyes.try_wait() {
        Ok(None) => eprintln!("xeyes running"),
        _ => panic!("xeyes not running"),
    }

    match xclock.try_wait() {
        Ok(None) => eprintln!("xclock running"),
        _ => panic!("xclock not running"),
    }

    // Clean up
    xeyes.kill().ok();
    xeyes.wait().ok();
    xclock.kill().ok();
    xclock.wait().ok();
    compositor.kill().ok();
    compositor.wait().ok();

    eprintln!("Test passed: Multiple X11 apps ran simultaneously");
}

/// Test that compositor works without xwayland-satellite (Wayland-only mode)
#[test]
fn test_compositor_works_without_xwayland_satellite() {
    // Require DISPLAY
    let display = match env::var("DISPLAY") {
        Ok(d) => d,
        Err(_) => {
            eprintln!("Skipping: DISPLAY not set");
            return;
        }
    };

    let workspace_root = find_workspace_root();

    // Build compositor
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "-p",
            "compositor",
            "--bin",
            "column-compositor",
        ])
        .current_dir(&workspace_root)
        .status()
        .expect("Failed to run cargo build");

    assert!(status.success(), "Compositor build failed");

    let uid = rustix::process::getuid().as_raw();

    // Clean up sockets
    let socket_path = ipc_socket_path();
    let _ = std::fs::remove_file(&socket_path);
    let wayland_socket = format!("/run/user/{}/wayland-1", uid);
    let wayland_lock = format!("/run/user/{}/wayland-1.lock", uid);
    let _ = std::fs::remove_file(&wayland_socket);
    let _ = std::fs::remove_file(&wayland_lock);

    // Start compositor with PATH cleared (so xwayland-satellite can't be found)
    let compositor_bin = workspace_root.join("target/release/column-compositor");
    let mut compositor = Command::new(&compositor_bin)
        .env("DISPLAY", &display)
        .env("RUST_LOG", "compositor=info")
        .env("PATH", "") // Clear PATH to prevent xwayland-satellite from being found
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start compositor");

    // Wait for compositor socket (should still work)
    if !wait_for_socket(Duration::from_secs(10)) {
        let mut stderr_output = String::new();
        if let Some(mut stderr) = compositor.stderr.take() {
            stderr.read_to_string(&mut stderr_output).ok();
        }
        compositor.kill().ok();
        compositor.wait().ok();
        panic!(
            "Compositor socket not ready\nstderr:\n{}",
            stderr_output
        );
    }

    // Compositor should be running even without xwayland-satellite
    thread::sleep(Duration::from_secs(1));

    match compositor.try_wait() {
        Ok(None) => {
            eprintln!("Compositor running successfully without xwayland-satellite");
        }
        Ok(Some(status)) => {
            let mut stderr_output = String::new();
            if let Some(mut stderr) = compositor.stderr.take() {
                stderr.read_to_string(&mut stderr_output).ok();
            }
            panic!(
                "Compositor exited with status {}\nstderr:\n{}",
                status, stderr_output
            );
        }
        Err(e) => {
            panic!("Error checking compositor status: {}", e);
        }
    }

    // Clean up
    compositor.kill().ok();
    compositor.wait().ok();

    eprintln!("Test passed: Compositor runs in Wayland-only mode without xwayland-satellite");
}
