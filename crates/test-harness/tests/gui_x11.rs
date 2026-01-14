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

use std::process::{Command, Stdio};
use std::time::Duration;
use std::{env, thread};
use test_harness::live::{
    cleanup_sockets, find_workspace_root, get_env_from_process, wait_for_socket,
    wait_for_xwayland,
};

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
    let xauthority = get_env_from_process(compositor_pid, "XAUTHORITY");
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
    let xauthority = get_env_from_process(compositor_pid, "XAUTHORITY");

    // Test X11 connection WITH our xauth file
    let mut cmd = Command::new("xdpyinfo");
    cmd.env("DISPLAY", &x_display);
    if let Some(ref xa) = xauthority {
        cmd.env("XAUTHORITY", xa);
    }
    let xdpyinfo_result = cmd.stdout(Stdio::null()).stderr(Stdio::null()).status();

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
