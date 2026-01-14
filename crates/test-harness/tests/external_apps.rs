//! Integration tests for external Wayland applications
//!
//! These tests verify that external apps can connect to and run inside the compositor.
//! Requires a running X11 display (Xvfb doesn't work due to missing DRI3 support).
//!
//! To run: cargo test -p test-harness --features gui-tests --test external_apps

#![cfg(feature = "gui-tests")]

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Duration;
use std::{env, thread};
use test_harness::live::{cleanup_sockets, find_workspace_root, wait_for_socket};

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
        .args(["build", "--release", "-p", "termstack", "--bin", "termstack"])
        .current_dir(&workspace_root)
        .status()
        .expect("Failed to run cargo build");

    assert!(status.success(), "Compositor build failed");

    let uid = rustix::process::getuid().as_raw();

    // Clean up sockets from previous runs
    cleanup_sockets();

    // Start compositor
    let compositor_bin = workspace_root.join("target/release/termstack");
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
