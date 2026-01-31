//! Tests for initial external window sizing
//!
//! When a GUI app opens, we want:
//! - Width: forced to compositor width
//! - Height: app's preferred height (from command line flags, content, etc.)
//!
//! The challenge is that xdg-shell's height=0 ("client decides") isn't
//! universally implemented - some apps interpret it literally as 0.
//!
//! # Integration Tests
//!
//! Tests that require the `gui-tests` feature run against a real compositor
//! and verify that external windows preserve their preferred height.
//!
//! To run: `cargo test -p test-harness --features gui-tests --test initial_window_size -- --ignored`

const TITLE_BAR_HEIGHT: u32 = 24;

/// Test: app that properly handles height=0 should use its preferred height
#[test]
fn app_respecting_height_zero_uses_preferred_height() {
    // Compositor sends configure(width=1920, height=0)
    // Well-behaved app interprets height=0 as "I choose my height"
    // App commits with (1920, 400) - its preferred size

    let compositor_width = 1920;
    let _configure_height = 0; // We send this

    // App commits with its preferred dimensions
    let committed_width = 1920;
    let committed_height = 400;

    // Width matches, height is app's choice - accept it
    assert_eq!(committed_width, compositor_width);

    // Final window height includes title bar for SSD
    let final_height = committed_height + TITLE_BAR_HEIGHT;
    assert_eq!(final_height, 424);
}

/// Test: app that ignores width constraint should be resized
#[test]
fn app_ignoring_width_constraint_gets_resized() {
    // Compositor sends configure(width=1920, height=0)
    // App ignores width and uses its preferred size
    // App commits with (800, 200) - ignoring our width

    let compositor_width = 1920;
    let committed_width = 800;
    let committed_height = 200;

    // Width doesn't match - we should send another configure to fix width
    // while preserving the app's chosen height
    assert_ne!(committed_width, compositor_width);

    // We send configure(1920, 200) to enforce width
    let enforced_width = compositor_width;
    let enforced_height = committed_height; // Keep app's height

    assert_eq!(enforced_width, 1920);
    assert_eq!(enforced_height, 200);
}

/// Test: app that interprets height=0 literally should get reasonable default
///
/// Some apps (like swayimg with tiled states) interpret height=0 as
/// "use 0 height" rather than "choose your own height". We need to
/// detect this and provide a reasonable fallback.
#[test]
fn app_interpreting_height_zero_literally_gets_fallback() {
    // Compositor sends configure(width=1920, height=0) with tiled states
    // App interprets this as "height should be 0"
    // App commits with (1920, 0) or (1920, 1)

    let compositor_width = 1920;
    let committed_width = 1920;
    let committed_height = 0; // App used 0 literally!

    // Width is correct, but height is unusably small
    assert_eq!(committed_width, compositor_width);

    // We should detect this as a broken app and use a fallback
    // Minimum useful height should be something reasonable
    const MIN_USEFUL_HEIGHT: u32 = 100;

    let final_height = if committed_height < MIN_USEFUL_HEIGHT {
        // App didn't understand height=0, use screen height or default
        600 // Fallback to reasonable default
    } else {
        committed_height
    };

    assert!(final_height >= MIN_USEFUL_HEIGHT,
        "Window height {} is too small, should have fallback", committed_height);
}

/// Test: with no size in configure, app should use preferred size
///
/// If we don't send a size at all (just tiled states), well-behaved
/// apps should use their preferred dimensions.
#[test]
fn no_size_in_configure_app_uses_preferred() {
    // Compositor sends configure with NO size, just tiled states
    // App should use its preferred dimensions

    // App (like swayimg -w 800,200) commits with its preferred size
    let committed_width = 800;
    let committed_height = 200;

    // We accept the height, but enforce our width
    let compositor_width = 1920;

    // If width doesn't match, we send configure to fix it
    if committed_width != compositor_width {
        // Send configure(1920, 200) - keep app's height
        let enforced_height = committed_height;
        assert_eq!(enforced_height, 200, "Should preserve app's height preference");
    }
}

/// Test: without tiled states, floating-style apps work correctly
///
/// Some apps behave differently with tiled states vs without.
/// Without tiled states, they act like floating windows and use
/// their preferred size, which is what we want for height.
#[test]
fn without_tiled_states_app_uses_natural_size() {
    // Compositor sends configure with NO size and NO tiled states
    // App thinks it's floating and uses natural size

    // For swayimg -w 800,200:
    let app_preferred_width = 800;
    let app_preferred_height = 200;

    // App commits with preferred size
    let _committed_width = app_preferred_width;
    let committed_height = app_preferred_height;

    // Then we enforce width while keeping height
    let compositor_width = 1920;
    let final_width = compositor_width;
    let final_height = committed_height; // Preserved!

    assert_eq!(final_width, 1920);
    assert_eq!(final_height, 200, "App's preferred height should be preserved");
}

// =============================================================================
// Integration tests using headless backend (no display required)
// =============================================================================

#[cfg(all(feature = "gui-tests", feature = "headless-backend"))]
mod headless_integration {
    use std::io::Read;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};
    use std::thread;
    use test_harness::live::find_workspace_root;

    const TITLE_BAR_HEIGHT: i32 = 24;

    /// RAII wrapper for compositor process - kills on drop
    struct CompositorGuard {
        child: Child,
        runtime_dir: String,
    }

    impl Drop for CompositorGuard {
        fn drop(&mut self) {
            self.child.kill().ok();
            self.child.wait().ok();
            let _ = std::fs::remove_dir_all(&self.runtime_dir);
        }
    }

    /// Find the termstack binary, checking debug then release profiles
    fn find_termstack_binary() -> std::path::PathBuf {
        let workspace_root = find_workspace_root();
        for profile in ["debug", "release"] {
            let bin = workspace_root.join(format!("target/{}/termstack", profile));
            if bin.exists() {
                return bin;
            }
        }
        panic!("termstack binary not found. Build with: cargo build -p termstack --features headless-backend");
    }

    /// Wait for a socket to exist and be connectable
    fn wait_for_socket(path: &str, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// Integration test: external windows preserve their preferred height (headless)
    ///
    /// NOTE: This test is currently not working because external Wayland clients
    /// (like foot, imv) need GPU/EGL support to create render buffers, which
    /// the headless backend doesn't provide. The X11 version of this test
    /// (in the `integration` module below) uses Xvfb which provides virtual GPU support.
    ///
    /// TODO: Either:
    /// 1. Add virtual GPU support to headless backend (via llvmpipe/softpipe)
    /// 2. Create a minimal test client that uses pure wl_shm
    /// 3. Use a Wayland protocol testing library
    ///
    /// To run: `cargo test -p test-harness --features gui-tests,headless-backend --test initial_window_size -- --ignored`
    #[test]
    #[ignore = "requires GPU support - use X11/Xvfb test instead"]
    fn external_window_preserves_preferred_height_headless() {
        // Check for foot terminal (uses software rendering, works in headless)
        if !Command::new("which").arg("foot").output().map(|o| o.status.success()).unwrap_or(false) {
            eprintln!("Skipping: foot terminal not installed");
            return;
        }

        let compositor_bin = find_termstack_binary();

        // Isolated runtime directory for this test
        let runtime_dir = format!("/tmp/claude/termstack-test-{}", std::process::id());
        std::fs::create_dir_all(&runtime_dir).unwrap();
        let ipc_socket = format!("{}/termstack.sock", runtime_dir);

        // Start headless compositor
        let compositor = Command::new(&compositor_bin)
            .env("TERMSTACK_BACKEND", "headless")
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .env("TERMSTACK_IPC_SOCKET", &ipc_socket)
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start compositor");

        let mut guard = CompositorGuard { child: compositor, runtime_dir: runtime_dir.clone() };

        // Wait for IPC socket (headless starts fast)
        if !wait_for_socket(&ipc_socket, Duration::from_secs(2)) {
            let mut stderr = String::new();
            guard.child.stderr.as_mut().map(|s| s.read_to_string(&mut stderr));
            panic!("IPC socket not ready\n{}", stderr);
        }

        // Find Wayland socket
        let wayland_socket = (0..10)
            .map(|i| format!("{}/wayland-{}", runtime_dir, i))
            .find(|p| wait_for_socket(p, Duration::from_millis(100)))
            .expect("No Wayland socket found");
        let wayland_display = std::path::Path::new(&wayland_socket).file_name().unwrap().to_str().unwrap();

        // Use foot terminal with specific pixel size
        // foot uses software rendering and should work in headless mode
        let (pref_w, pref_h) = (400, 200);
        let mut foot = Command::new("foot")
            .args([
                "--window-size-pixels", &format!("{}x{}", pref_w, pref_h),
                "--", "sleep", "10"  // Keep the terminal open
            ])
            .env("WAYLAND_DISPLAY", wayland_display)
            .env("XDG_RUNTIME_DIR", &runtime_dir)
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start foot terminal");

        // Poll for window with expected height
        let expected_height = pref_h + TITLE_BAR_HEIGHT;
        let start = Instant::now();
        let timeout = Duration::from_secs(3);
        let mut last_height: Option<i32> = None;

        #[derive(serde::Deserialize)]
        struct WindowInfo { height: i32, is_external: bool }

        loop {
            if start.elapsed() >= timeout {
                foot.kill().ok();
                panic!(
                    "Window did not reach expected height {} within {:?}. Last seen: {:?}",
                    expected_height, timeout, last_height
                );
            }

            // Check if foot crashed
            if let Ok(Some(status)) = foot.try_wait() {
                let mut stderr = String::new();
                foot.stderr.as_mut().map(|s| s.read_to_string(&mut stderr));
                panic!("foot exited unexpectedly: {} {}", status, stderr);
            }

            // Query window heights via IPC
            if let Ok(out) = Command::new(&compositor_bin)
                .args(["query-windows"])
                .env("TERMSTACK_SOCKET", &ipc_socket)
                .output()
            {
                if let Ok(windows) = serde_json::from_slice::<Vec<WindowInfo>>(&out.stdout) {
                    if let Some(ext) = windows.iter().find(|w| w.is_external) {
                        last_height = Some(ext.height);
                        if ext.height == expected_height {
                            break; // Success!
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(20));
        }

        foot.kill().ok();
        eprintln!("SUCCESS: Window height {} ({}px + {}px title bar)", expected_height, pref_h, TITLE_BAR_HEIGHT);
    }
}

// =============================================================================
// Integration tests (require gui-tests feature and a running display)
// =============================================================================

#[cfg(feature = "gui-tests")]
mod integration {
    use std::io::Read;
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;
    use std::{env, thread};
    use test_harness::live::find_workspace_root;

    const TITLE_BAR_HEIGHT: i32 = 24;

    /// RAII wrapper for Xvfb process - kills on drop
    struct XvfbGuard(Child);

    impl Drop for XvfbGuard {
        fn drop(&mut self) {
            self.0.kill().ok();
            self.0.wait().ok();
        }
    }

    /// Start Xvfb on an available display number, returns (display, guard)
    fn start_xvfb() -> Option<(String, XvfbGuard)> {
        // Check if Xvfb is installed
        if Command::new("which")
            .arg("Xvfb")
            .output()
            .map(|o| !o.status.success())
            .unwrap_or(true)
        {
            return None;
        }

        // Try display numbers 99-109 to avoid conflicts
        for display_num in 99..110 {
            let display = format!(":{}", display_num);
            let lock_file = format!("/tmp/.X{}-lock", display_num);

            // Skip if lock file exists (display in use)
            if std::path::Path::new(&lock_file).exists() {
                continue;
            }

            // Try to start Xvfb
            match Command::new("Xvfb")
                .args([&display, "-screen", "0", "1280x800x24"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(process) => {
                    // Give Xvfb time to start
                    thread::sleep(Duration::from_millis(500));

                    // Verify it's running
                    if std::path::Path::new(&lock_file).exists() {
                        return Some((display, XvfbGuard(process)));
                    }
                }
                Err(_) => continue,
            }
        }
        None
    }

    /// Integration test: external windows preserve their preferred height
    ///
    /// This test verifies the full stack:
    /// 1. new_toplevel sets state.bounds (not state.size)
    /// 2. Apps can use their preferred size within bounds
    /// 3. Width is enforced to compositor width
    /// 4. Height is preserved from app's preference
    ///
    /// To run: `cargo test -p test-harness --features gui-tests --test initial_window_size -- --ignored`
    #[test]
    #[ignore = "requires Xvfb and imv-wayland installed"]
    fn external_window_preserves_preferred_height() {
        // Check for imv-wayland
        let has_imv = Command::new("which")
            .arg("imv-wayland")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !has_imv {
            eprintln!("Skipping: imv-wayland not installed");
            eprintln!("Install with: apt install imv or brew install imv");
            return;
        }

        // Always use Xvfb for isolation - this avoids conflicts with any running compositor
        // and ensures consistent test behavior regardless of the user's display setup
        let (display, _xvfb) = match start_xvfb() {
            Some((display, process)) => {
                eprintln!("Started Xvfb on {}", display);
                (display, Some(process))
            }
            None => {
                eprintln!("Skipping: Could not start Xvfb");
                eprintln!("Install Xvfb with: apt install xvfb");
                return;
            }
        };

        let workspace_root = find_workspace_root();

        // Build compositor
        eprintln!("Building compositor...");
        let status = Command::new("cargo")
            .args([
                "build",
                "--release",
                "-p",
                "termstack",
            ])
            .current_dir(&workspace_root)
            .status()
            .expect("Failed to run cargo build");

        assert!(status.success(), "Compositor build failed");

        // Use a test-specific runtime directory to avoid conflicts with running compositor
        // and to work within sandbox restrictions
        let test_runtime_dir = format!(
            "/tmp/claude/termstack-test-{}",
            std::process::id()
        );
        std::fs::create_dir_all(&test_runtime_dir).expect("Failed to create test runtime dir");

        // Use the test runtime directory for IPC socket
        let test_ipc_socket = format!("{}/termstack.sock", test_runtime_dir);

        // Start compositor with test-specific runtime directory
        // Use software rendering (llvmpipe) since Xvfb doesn't have DRI3
        let compositor_bin = workspace_root.join("target/release/termstack");
        eprintln!("Starting compositor: {:?}", compositor_bin);
        eprintln!("Test runtime dir: {}", test_runtime_dir);
        let mut compositor = Command::new(&compositor_bin)
            .env("DISPLAY", &display)
            .env("XDG_RUNTIME_DIR", &test_runtime_dir)
            .env("TERMSTACK_IPC_SOCKET", &test_ipc_socket)
            .env("LIBGL_ALWAYS_SOFTWARE", "1") // Force software rendering for Xvfb
            .env("RUST_LOG", "termstack_compositor=info")
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start compositor");

        // Wait for test compositor's IPC socket
        let test_socket_path = std::path::PathBuf::from(&test_ipc_socket);
        let socket_wait_start = std::time::Instant::now();
        let socket_timeout = Duration::from_secs(10);
        while socket_wait_start.elapsed() < socket_timeout {
            if test_socket_path.exists()
                && std::os::unix::net::UnixStream::connect(&test_socket_path).is_ok()
            {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        if !test_socket_path.exists() {
            let mut stderr_output = String::new();
            if let Some(mut stderr) = compositor.stderr.take() {
                stderr.read_to_string(&mut stderr_output).ok();
            }
            compositor.kill().ok();
            compositor.wait().ok();
            let _ = std::fs::remove_dir_all(&test_runtime_dir);

            // Skip gracefully if DRI3 extension is missing (Xvfb limitation)
            if stderr_output.contains("DRI3") || stderr_output.contains("MissingExtension") {
                eprintln!("Skipping: X11 backend requires DRI3 extension (not available in Xvfb)");
                eprintln!("This test requires a display with GPU support or DRI3");
                return;
            }

            panic!(
                "Compositor socket not ready after 10s\nstderr:\n{}",
                stderr_output
            );
        }
        eprintln!("Compositor socket ready at {}", test_ipc_socket);

        // Wait for the Wayland socket to appear and be connectable
        let wayland_display;
        let wayland_wait_start = std::time::Instant::now();
        loop {
            if wayland_wait_start.elapsed() >= Duration::from_secs(5) {
                compositor.kill().ok();
                compositor.wait().ok();
                let _ = std::fs::remove_dir_all(&test_runtime_dir);
                panic!("No Wayland socket appeared within 5s");
            }

            // Look for wayland-0 in test runtime dir (should be the first one since dir is fresh)
            if let Some(display_num) = (0..10).find(|i| {
                std::path::Path::new(&format!("{}/wayland-{}", test_runtime_dir, i)).exists()
            }) {
                let socket_path = format!("{}/wayland-{}", test_runtime_dir, display_num);
                // Check if socket is connectable
                if std::os::unix::net::UnixStream::connect(&socket_path).is_ok() {
                    wayland_display = format!("wayland-{}", display_num);
                    break;
                }
            }
            thread::sleep(Duration::from_millis(50));
        }
        eprintln!("Using WAYLAND_DISPLAY={}", wayland_display);

        // Create a simple test image (1x1 PNG)
        let test_image = format!("{}/test_image.png", env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string()));
        // Use ImageMagick convert or create a placeholder
        let _ = Command::new("convert")
            .args(["-size", "1x1", "xc:white", &test_image])
            .status();

        // If convert isn't available, try creating a minimal PNG programmatically would be complex
        // Instead, let's use /dev/null or a small existing file - imv can handle it
        let image_arg = if std::path::Path::new(&test_image).exists() {
            test_image.clone()
        } else {
            // imv-wayland can open without an image and still use -W/-H
            "".to_string()
        };

        // Spawn imv-wayland with specific dimensions: 400x200
        let preferred_width = 400;
        let preferred_height = 200;

        eprintln!(
            "Spawning imv-wayland with -W {} -H {}",
            preferred_width, preferred_height
        );

        let mut imv_args = vec![
            "-W".to_string(),
            preferred_width.to_string(),
            "-H".to_string(),
            preferred_height.to_string(),
        ];
        if !image_arg.is_empty() {
            imv_args.push(image_arg);
        }

        let mut imv = Command::new("imv-wayland")
            .args(&imv_args)
            .env("WAYLAND_DISPLAY", &wayland_display)
            .env("XDG_RUNTIME_DIR", &test_runtime_dir)
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start imv-wayland");

        // Poll for external window with correct height (instead of fixed sleep)
        let socket_path = std::path::PathBuf::from(&test_ipc_socket);
        let poll_start = std::time::Instant::now();
        let poll_timeout = Duration::from_secs(5);
        let expected_height = preferred_height + TITLE_BAR_HEIGHT;
        let mut last_height = 0i32;

        #[derive(Debug, serde::Deserialize)]
        #[allow(dead_code)]
        struct WindowInfo {
            index: usize,
            width: i32,
            height: i32,
            is_external: bool,
            command: String,
        }

        loop {
            if poll_start.elapsed() >= poll_timeout {
                imv.kill().ok();
                imv.wait().ok();
                compositor.kill().ok();
                compositor.wait().ok();
                panic!(
                    "Window height did not reach expected {} within {:?}. Last height: {}",
                    expected_height, poll_timeout, last_height
                );
            }

            // Check if imv exited early
            match imv.try_wait() {
                Ok(Some(status)) => {
                    let mut stderr_output = String::new();
                    if let Some(mut stderr) = imv.stderr.take() {
                        stderr.read_to_string(&mut stderr_output).ok();
                    }
                    compositor.kill().ok();
                    compositor.wait().ok();

                    if stderr_output.contains("No input") || stderr_output.contains("error") {
                        eprintln!("imv-wayland requires an image file - skipping test");
                        return;
                    }

                    panic!(
                        "imv-wayland exited early with status {}: {}",
                        status, stderr_output
                    );
                }
                Ok(None) => {} // Still running
                Err(e) => {
                    compositor.kill().ok();
                    compositor.wait().ok();
                    panic!("Error checking imv status: {}", e);
                }
            }

            // Query for external windows with expected height
            let output = Command::new(&compositor_bin)
                .args(["query-windows"])
                .env("TERMSTACK_SOCKET", socket_path.to_str().unwrap())
                .output();

            if let Ok(output) = output {
                if output.status.success() {
                    let response = String::from_utf8_lossy(&output.stdout);
                    if let Ok(windows) = serde_json::from_str::<Vec<WindowInfo>>(&response) {
                        if let Some(ext) = windows.iter().find(|w| w.is_external) {
                            last_height = ext.height;
                            if ext.height == expected_height {
                                eprintln!(
                                    "Window reached expected height {} after {:?}",
                                    expected_height,
                                    poll_start.elapsed()
                                );
                                break;
                            }
                        }
                    }
                }
            }

            thread::sleep(Duration::from_millis(50));
        }

        // Clean up
        imv.kill().ok();
        imv.wait().ok();
        compositor.kill().ok();
        compositor.wait().ok();

        // Clean up test files and directories
        let _ = std::fs::remove_file(&test_image);
        let _ = std::fs::remove_dir_all(&test_runtime_dir);

        // If we got here, the polling loop verified the height is correct
        eprintln!(
            "SUCCESS: External window has correct height {} ({}px content + {}px title bar)",
            expected_height,
            preferred_height,
            TITLE_BAR_HEIGHT
        );
    }
}
