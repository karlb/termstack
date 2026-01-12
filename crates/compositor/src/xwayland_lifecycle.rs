//! XWayland lifecycle management
//!
//! Handles XWayland initialization, xwayland-satellite spawning, health monitoring,
//! and automatic crash recovery with exponential backoff.

use std::time::{Duration, Instant};
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_server::Display;
use crate::state::{TermStack, XWaylandSatelliteMonitor};

/// Initialize XWayland support with xwayland-satellite for running X11 applications
///
/// xwayland-satellite acts as the X11 window manager and presents X11 windows as
/// normal Wayland toplevels to the compositor.
pub fn initialize_xwayland(
    _compositor: &mut TermStack,
    display: &mut Display<TermStack>,
    loop_handle: LoopHandle<'static, TermStack>,
) {
    // Spawn XWayland without a window manager (xwayland-satellite will be the WM)
    use smithay::xwayland::{XWayland, XWaylandEvent};

    let (xwayland, _client) = match XWayland::spawn(
        &display.handle(),
        None, // Let XWayland pick display number
        std::iter::empty::<(String, String)>(),
        false, // Use on-disk socket (not abstract) so xwayland-satellite can connect
        std::process::Stdio::null(),
        std::process::Stdio::null(),
        |_| (),
    ) {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!(?e, "Failed to spawn XWayland - X11 apps will not work");
            return;
        }
    };

    // Insert XWayland event source to handle Ready/Error events
    if let Err(e) = loop_handle.insert_source(xwayland, move |event, _, compositor| {
        match event {
            XWaylandEvent::Ready { display_number, .. } => {
                tracing::info!(display_number, "XWayland ready, spawning xwayland-satellite");

                // Set DISPLAY for child processes
                std::env::set_var("DISPLAY", format!(":{}", display_number));
                compositor.x11_display_number = Some(display_number);

                // Set up X authorization for GTK apps.
                // XWayland accepts unauthenticated local connections (no -auth flag),
                // but GTK apps require an xauth entry to exist. We create a dummy
                // cookie that GTK will find, even though XWayland won't validate it.
                setup_xauthority(display_number);

                // Spawn xwayland-satellite (acts as X11 WM, presents windows as Wayland toplevels)
                match spawn_xwayland_satellite(display_number) {
                    Ok(child) => {
                        tracing::info!("xwayland-satellite launched successfully");
                        compositor.xwayland_satellite = Some(child);
                    }
                    Err(e) => {
                        // Soft dependency: warn but continue
                        // Print to stderr for visibility (only shown once at startup)
                        eprintln!();
                        eprintln!("⚠️  WARNING: xwayland-satellite not found");
                        eprintln!("   X11 applications will not work (Wayland apps only)");
                        eprintln!("   Install: cargo install xwayland-satellite");
                        eprintln!();

                        tracing::warn!(
                            ?e,
                            "Failed to spawn xwayland-satellite - continuing in Wayland-only mode"
                        );
                    }
                }

                // Spawn initial terminal now that DISPLAY is set
                compositor.spawn_initial_terminal = true;
            }
            XWaylandEvent::Error => {
                tracing::error!("XWayland failed");
            }
        }
    }) {
        tracing::warn!(?e, "Failed to insert XWayland event source");
    }
}

/// Monitor xwayland-satellite health and auto-restart on crash with backoff
///
/// Returns true if the compositor should continue running, false if shutdown is requested.
pub fn monitor_xwayland_satellite_health(compositor: &mut TermStack) -> bool {
    if let Some(mut monitor) = compositor.xwayland_satellite.take() {
        match monitor.child.try_wait() {
            Ok(Some(status)) => {
                // xwayland-satellite crashed! Try to read stderr to see why
                let stderr_output = if let Some(ref mut stderr) = monitor.child.stderr {
                    use std::io::Read;
                    let mut buf = String::new();
                    stderr.read_to_string(&mut buf).ok();
                    buf
                } else {
                    String::new()
                };

                if !stderr_output.is_empty() {
                    tracing::error!(?status, stderr = %stderr_output, "xwayland-satellite crashed");
                } else {
                    tracing::warn!(?status, "xwayland-satellite exited");
                }

                // Determine if this is a rapid crash (within 10s of last crash)
                let now = Instant::now();
                let time_since_last = monitor.last_crash_time.map(|t| now.duration_since(t));
                let is_rapid_crash = time_since_last.is_some_and(|d| d < Duration::from_secs(10));

                if is_rapid_crash {
                    monitor.crash_count += 1;
                } else {
                    // Ran for a while before crashing, reset counter
                    monitor.crash_count = 1;
                }

                monitor.last_crash_time = Some(now);

                if monitor.crash_count <= 3 {
                    tracing::warn!(
                        attempt = monitor.crash_count,
                        "xwayland-satellite crashed, restarting (attempt {}/3)",
                        monitor.crash_count
                    );

                    // Attempt restart
                    match spawn_xwayland_satellite(compositor.x11_display_number.unwrap()) {
                        Ok(mut new_monitor) => {
                            // Preserve crash tracking from old monitor
                            new_monitor.crash_count = monitor.crash_count;
                            new_monitor.last_crash_time = monitor.last_crash_time;
                            compositor.xwayland_satellite = Some(new_monitor);
                        }
                        Err(e) => {
                            tracing::error!(?e, "Failed to restart xwayland-satellite");
                        }
                    }
                } else {
                    tracing::error!(
                        "xwayland-satellite crashed {} times in rapid succession, giving up",
                        monitor.crash_count
                    );
                    tracing::warn!("X11 apps will not work for the rest of this session");
                    // Don't put monitor back - X11 support disabled for session
                }
            }
            Ok(None) => {
                // Still running, put monitor back
                compositor.xwayland_satellite = Some(monitor);
            }
            Err(e) => {
                tracing::debug!(?e, "Error checking xwayland-satellite status");
                // Put monitor back even on error
                compositor.xwayland_satellite = Some(monitor);
            }
        }
    }

    true // Continue running
}

/// Set up X authorization for GTK apps to connect to XWayland.
///
/// XWayland (started without -auth) accepts unauthenticated local connections,
/// but GTK apps require an xauth entry to exist. We create a dummy cookie
/// that satisfies GTK's check, even though XWayland won't validate it.
fn setup_xauthority(display_number: u32) {
    let xauth_path = if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        format!("{}/termstack-xauth", runtime_dir)
    } else {
        let uid = rustix::process::getuid().as_raw();
        format!("/tmp/termstack-xauth-{}", uid)
    };

    // Create xauth entry with a dummy cookie (XWayland won't validate it)
    let result = std::process::Command::new("xauth")
        .args(["-f", &xauth_path, "add", &format!(":{}", display_number), "MIT-MAGIC-COOKIE-1", "0"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match result {
        Ok(status) if status.success() => {
            std::env::set_var("XAUTHORITY", &xauth_path);
            tracing::info!(path = %xauth_path, display = display_number, "X authorization set up for GTK apps");
        }
        Ok(status) => {
            tracing::warn!(?status, "xauth command failed, GTK X11 apps may not work");
        }
        Err(e) => {
            tracing::warn!(?e, "Failed to run xauth, GTK X11 apps may not work");
        }
    }
}

/// Spawn xwayland-satellite process to act as X11 window manager
fn spawn_xwayland_satellite(display_number: u32) -> std::io::Result<XWaylandSatelliteMonitor> {
    // Try to find xwayland-satellite in common locations
    let xwayland_satellite_path = find_xwayland_satellite()
        .unwrap_or_else(|| "xwayland-satellite".to_string());

    // xwayland-satellite needs to connect to OUR compositor's Wayland socket (wayland-1),
    // not the host compositor's socket (wayland-0)
    let child = std::process::Command::new(xwayland_satellite_path)
        .arg(format!(":{}", display_number))
        // In nested setup: host is wayland-0, our compositor is wayland-1
        // xwayland-satellite MUST connect to our compositor, not the host
        .env("WAYLAND_DISPLAY", "wayland-1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped()) // Capture stderr for logging
        .spawn()?;

    Ok(XWaylandSatelliteMonitor {
        child,
        last_crash_time: None,
        crash_count: 0,
    })
}

/// Find xwayland-satellite binary in common installation locations
fn find_xwayland_satellite() -> Option<String> {
    // Check cargo install location first (most common for development)
    if let Ok(home) = std::env::var("HOME") {
        let cargo_bin = format!("{}/.cargo/bin/xwayland-satellite", home);
        if std::path::Path::new(&cargo_bin).exists() {
            tracing::debug!(path = %cargo_bin, "found xwayland-satellite in ~/.cargo/bin");
            return Some(cargo_bin);
        }
    }

    // Check system locations
    let system_paths = [
        "/usr/local/bin/xwayland-satellite",
        "/usr/bin/xwayland-satellite",
    ];

    for path in &system_paths {
        if std::path::Path::new(path).exists() {
            tracing::debug!(path = %path, "found xwayland-satellite in system path");
            return Some(path.to_string());
        }
    }

    // Fall back to relying on PATH (will fail if not in PATH, but worth trying)
    tracing::debug!("xwayland-satellite not found in known locations, trying PATH");
    None
}
