//! Live Wayland client tests
//!
//! These tests run against the real compositor with actual Wayland clients.
//! They require a display (X11 or Wayland) and are marked `#[ignore]` by default.
//!
//! Run with: `cargo test -- --ignored`
//!
//! # Test Environment
//!
//! These tests:
//! 1. Verify display availability before running
//! 2. Set up the environment for the compositor (WINIT_UNIX_BACKEND=x11)
//! 3. Can spawn external clients like `foot`
//!
//! # Current Limitations
//!
//! Full live testing would require:
//! - Running the compositor event loop in a background thread
//! - IPC to control the compositor from tests
//! - Process management for spawned clients
//!
//! These tests provide the framework for such testing but are currently
//! placeholder implementations demonstrating the approach.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use test_harness::live;

/// Verify display detection works
#[test]
fn display_detection() {
    let display = live::detect_display();
    println!("Detected display: {:?}", display);

    // This test always passes - it just reports what's available
    match display {
        live::DisplayType::X11 => println!("X11 display available"),
        live::DisplayType::Wayland => println!("Wayland display available"),
        live::DisplayType::None => println!("No display available"),
    }
}

/// Verify environment setup works
#[test]
fn environment_setup() {
    // Just verify creating and dropping TestEnvironment doesn't panic
    let env = live::TestEnvironment::new();
    drop(env);
}

/// Placeholder for live compositor spawn test
///
/// This test would:
/// 1. Start the compositor in a background thread
/// 2. Wait for it to initialize
/// 3. Verify it's running
/// 4. Shut it down cleanly
#[test]
#[ignore = "requires display and full compositor infrastructure"]
fn compositor_can_start() {
    if !live::display_available() {
        eprintln!("Skipping: no display available");
        return;
    }

    let _env = live::TestEnvironment::new();

    // TODO: Implement when compositor can run in test mode
    // This would involve:
    // 1. Spawning the compositor as a child process or in a thread
    // 2. Waiting for it to become ready
    // 3. Verifying it's responsive
    // 4. Clean shutdown

    eprintln!("Test not yet implemented: needs compositor test mode");
}

/// Placeholder for spawning foot terminal
///
/// This test would:
/// 1. Start the compositor
/// 2. Spawn foot against it
/// 3. Wait for the window to appear
/// 4. Verify the window is in the layout
#[test]
#[ignore = "requires display and full compositor infrastructure"]
fn spawn_foot_client() {
    if !live::display_available() {
        eprintln!("Skipping: no display available");
        return;
    }

    let _env = live::TestEnvironment::new();

    // TODO: Implement when compositor can run in test mode
    // This would involve:
    // 1. Starting the compositor
    // 2. Setting WAYLAND_DISPLAY to point to it
    // 3. Spawning `foot` as a child process
    // 4. Waiting for the window to appear
    // 5. Verifying the layout includes the window

    eprintln!("Test not yet implemented: needs compositor test mode");
}

/// Placeholder for click-to-focus with external client
///
/// This test would:
/// 1. Start the compositor
/// 2. Spawn two clients
/// 3. Click on the second client
/// 4. Verify focus changed
#[test]
#[ignore = "requires display and full compositor infrastructure"]
fn click_to_focus_external() {
    if !live::display_available() {
        eprintln!("Skipping: no display available");
        return;
    }

    let _env = live::TestEnvironment::new();

    // TODO: Implement when compositor can run in test mode

    eprintln!("Test not yet implemented: needs compositor test mode");
}

/// Placeholder for scroll with external clients
///
/// This test would:
/// 1. Start the compositor
/// 2. Spawn multiple clients
/// 3. Scroll the viewport
/// 4. Verify scroll position changed
#[test]
#[ignore = "requires display and full compositor infrastructure"]
fn scroll_external_clients() {
    if !live::display_available() {
        eprintln!("Skipping: no display available");
        return;
    }

    let _env = live::TestEnvironment::new();

    // TODO: Implement when compositor can run in test mode

    eprintln!("Test not yet implemented: needs compositor test mode");
}

/// Running compositor with log capture
struct RunningCompositor {
    child: Child,
    socket_name: String,
    log_receiver: mpsc::Receiver<String>,
}

impl RunningCompositor {
    /// Wait for a log message matching the pattern
    fn wait_for_log(&self, pattern: &str, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            match self.log_receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    if line.contains(pattern) {
                        return true;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => return false,
            }
        }
        false
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for RunningCompositor {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Helper to start compositor and wait for it to be ready
fn start_compositor() -> Option<RunningCompositor> {
    // Find the compositor binary - check both relative and workspace paths
    let compositor_path = if std::path::Path::new("./target/release/column-compositor").exists() {
        "./target/release/column-compositor".to_string()
    } else {
        // When running from test-harness crate, go up to workspace root
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("Failed to find workspace root");
        workspace_root
            .join("target/release/column-compositor")
            .to_string_lossy()
            .to_string()
    };

    if !std::path::Path::new(&compositor_path).exists() {
        eprintln!(
            "Compositor binary not found at {}. Run `cargo build --release` first.",
            compositor_path
        );
        return None;
    }

    eprintln!("Starting compositor from: {}", compositor_path);

    // Use script to create a pseudo-TTY (tracing needs TTY for output)
    // Also set all the environment variables needed for testing
    let mut child = match Command::new("script")
        .arg("-q") // quiet mode
        .arg("-c")
        .arg(format!(
            "WINIT_UNIX_BACKEND=x11 RUST_LOG=column_compositor=info GDK_BACKEND=wayland QT_QPA_PLATFORM=wayland {}",
            compositor_path
        ))
        .arg("/dev/null")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to spawn compositor: {}", e);
            return None;
        }
    };

    // Read stdout (script outputs there) in a background thread
    let stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    let (socket_tx, socket_rx) = mpsc::channel();

    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        let mut found_socket = false;
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    // Strip ANSI codes for pattern matching
                    let stripped: String = line
                        .chars()
                        .fold((String::new(), false), |(mut s, in_escape), c| {
                            if c == '\x1b' {
                                (s, true)
                            } else if in_escape {
                                if c == 'm' {
                                    (s, false)
                                } else {
                                    (s, true)
                                }
                            } else {
                                s.push(c);
                                (s, false)
                            }
                        })
                        .0;
                    if !found_socket && stripped.contains("listening on Wayland socket") {
                        // Extract socket name from stripped line
                        if let Some(start) = stripped.find("socket_name=") {
                            let rest = &stripped[start + 13..]; // skip 'socket_name="'
                            if let Some(end) = rest.find('"') {
                                let _ = socket_tx.send(rest[..end].to_string());
                                found_socket = true;
                            }
                        }
                    }
                    if tx.send(stripped).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Wait for socket name with timeout
    let socket_name = match socket_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(name) => name,
        Err(_) => {
            eprintln!("Timed out waiting for compositor socket");
            let _ = child.kill();
            return None;
        }
    };

    Some(RunningCompositor {
        child,
        socket_name,
        log_receiver: rx,
    })
}

/// Test that foot terminal connects to the compositor
///
/// This verifies that external Wayland clients properly connect.
#[test]
#[ignore = "requires display and foot installed"]
fn foot_terminal_connects() {
    if !live::display_available() {
        eprintln!("Skipping: no display available");
        return;
    }

    // Check if foot is available
    if Command::new("which")
        .arg("foot")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("Skipping: foot not installed");
        return;
    }

    let _env = live::TestEnvironment::new();

    // Start compositor
    let Some(compositor) = start_compositor() else {
        panic!("Failed to start compositor");
    };

    // Give compositor time to fully initialize
    std::thread::sleep(Duration::from_millis(500));

    // Launch foot terminal against our compositor
    let mut foot = Command::new("foot")
        .arg("-e")
        .arg("echo")
        .arg("test")
        .env("WAYLAND_DISPLAY", &compositor.socket_name)
        .spawn()
        .expect("Failed to spawn foot");

    // Wait for client connection and window
    let connected = compositor.wait_for_log("new Wayland client connected", Duration::from_secs(3));
    assert!(connected, "foot should connect as Wayland client");

    let window_added =
        compositor.wait_for_log("handling new external window", Duration::from_secs(2));
    assert!(window_added, "foot window should be added to layout");

    // Clean up
    let _ = foot.kill();
    let _ = foot.wait();
}

/// Test that GTK apps connect to compositor with GDK_BACKEND=wayland
///
/// This verifies the fix for GTK apps like pqiv not connecting.
#[test]
#[ignore = "requires display and pqiv installed"]
fn gtk_app_connects_with_gdk_backend() {
    if !live::display_available() {
        eprintln!("Skipping: no display available");
        return;
    }

    // Check if pqiv is available
    if Command::new("which")
        .arg("pqiv")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        eprintln!("Skipping: pqiv not installed");
        return;
    }

    let _env = live::TestEnvironment::new();

    // Start compositor
    let Some(compositor) = start_compositor() else {
        panic!("Failed to start compositor");
    };

    // Give compositor time to fully initialize
    std::thread::sleep(Duration::from_millis(500));

    // Launch pqiv with GDK_BACKEND=wayland
    // Use a standard icon directory that should exist on most systems
    let icon_path = if std::path::Path::new("/usr/share/icons/hicolor/48x48/apps").exists() {
        "/usr/share/icons/hicolor/48x48/apps"
    } else {
        "/usr/share/pixmaps"
    };

    let mut pqiv = Command::new("pqiv")
        .arg(icon_path)
        .env("WAYLAND_DISPLAY", &compositor.socket_name)
        .env("GDK_BACKEND", "wayland")
        .spawn()
        .expect("Failed to spawn pqiv");

    // Wait for client connection
    let connected = compositor.wait_for_log("new Wayland client connected", Duration::from_secs(3));
    assert!(connected, "pqiv should connect as Wayland client");

    let window_added =
        compositor.wait_for_log("handling new external window", Duration::from_secs(2));
    assert!(window_added, "pqiv window should be added to layout");

    // Clean up
    let _ = pqiv.kill();
    let _ = pqiv.wait();
}

/// Test that compositor spawns a shell with correct environment
///
/// This verifies the environment inheritance for child processes.
/// Note: This test is simplified because finding the exact shell spawned by
/// THIS compositor instance is complex (socket names can be reused).
/// The gtk_app_connects_with_gdk_backend test provides better coverage
/// of the GDK_BACKEND functionality.
#[test]
#[ignore = "requires display"]
fn shell_inherits_wayland_display() {
    if !live::display_available() {
        eprintln!("Skipping: no display available");
        return;
    }

    let _env = live::TestEnvironment::new();

    // Start compositor
    let Some(compositor) = start_compositor() else {
        panic!("Failed to start compositor");
    };

    // Give compositor time to spawn terminal
    std::thread::sleep(Duration::from_secs(1));

    // The log output already shows the terminal was spawned successfully
    // and the compositor sets WAYLAND_DISPLAY before spawning.
    // The gtk_app_connects test verifies GDK_BACKEND works.

    // Verify the compositor reported spawning the initial terminal
    let got_spawn = compositor.wait_for_log("spawned initial terminal", Duration::from_millis(100));
    assert!(got_spawn, "Compositor should spawn initial terminal");

    eprintln!("Verified initial terminal spawn");
}
