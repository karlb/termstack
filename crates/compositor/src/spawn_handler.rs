//! Terminal and GUI application spawn handling
//!
//! Processes IPC spawn requests for both terminal commands and GUI applications.
//! Handles environment setup, script extraction, and focus management.

use std::path::PathBuf;
use crate::ipc::SpawnRequest;
use crate::state::{FocusedWindow, StackWindow, TermStack};
use crate::terminal_manager::{TerminalId, TerminalManager};

/// Handle IPC spawn requests for terminal commands
///
/// Processes pending terminal spawn requests, focuses the new terminal,
/// and scrolls to show it.
pub fn handle_ipc_spawn_requests(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
    calculate_window_heights: impl Fn(&TermStack, &TerminalManager) -> Vec<i32>,
) {
    while let Some(request) = compositor.pending_spawn_requests.pop() {
        if let Some(id) = process_spawn_request(compositor, terminal_manager, request) {
            // Focus the new command terminal
            for (i, node) in compositor.layout_nodes.iter().enumerate() {
                if let StackWindow::Terminal(tid) = node.cell {
                    if tid == id {
                        compositor.set_focus_by_index(i);
                        tracing::info!(id = id.0, index = i, "focused new command terminal");
                        break;
                    }
                }
            }

            // Update cell heights
            let new_heights = calculate_window_heights(compositor, terminal_manager);
            compositor.update_layout_heights(new_heights);

            // Scroll to show the new terminal
            if let Some(focused_idx) = compositor.focused_index() {
                if let Some(new_scroll) = compositor.scroll_to_show_window_bottom(focused_idx) {
                    tracing::info!(
                        id = id.0,
                        focused_idx,
                        new_scroll,
                        "spawned command terminal, scrolling to show"
                    );
                }
            }
        }
    }
}

/// Handle GUI spawn requests from IPC (termstack gui)
///
/// Spawns GUI app commands with foreground/background mode support.
/// In foreground mode, the launching terminal is hidden until the GUI exits.
pub fn handle_gui_spawn_requests(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
    calculate_window_heights: impl Fn(&TermStack, &TerminalManager) -> Vec<i32>,
) {
    while let Some(request) = compositor.pending_gui_spawn_requests.pop() {
        // Get the launching terminal (currently focused)
        let launching_terminal = compositor.focused_window.as_ref().and_then(|cell| match cell {
            FocusedWindow::Terminal(id) => Some(*id),
            FocusedWindow::External(_) => None,
        });

        // Extract foreground flag (guaranteed to be Some for GUI spawns)
        let foreground = request.foreground.unwrap_or(true);

        // Modify environment for GUI apps - use compositor's display, not host's
        let mut env = request.env.clone();
        if let Ok(wayland_display) = std::env::var("WAYLAND_DISPLAY") {
            env.insert("WAYLAND_DISPLAY".to_string(), wayland_display);
        }
        // Also set DISPLAY for X11 apps (via xwayland-satellite)
        if let Ok(display) = std::env::var("DISPLAY") {
            env.insert("DISPLAY".to_string(), display);
        }
        // Set XAUTHORITY to our xauth file (created by setup_xauthority).
        // GTK apps require an xauth entry to exist, even though XWayland doesn't validate it.
        if let Ok(xauthority) = std::env::var("XAUTHORITY") {
            env.insert("XAUTHORITY".to_string(), xauthority);
        }
        // Preserve host display variables for consistency
        if let Ok(host_wayland) = std::env::var("HOST_WAYLAND_DISPLAY") {
            env.insert("HOST_WAYLAND_DISPLAY".to_string(), host_wayland);
        }
        if let Ok(host_x11) = std::env::var("HOST_DISPLAY") {
            env.insert("HOST_DISPLAY".to_string(), host_x11);
        }
        // Force X11 backend for GTK/Qt apps via the `gui` command.
        // Our compositor doesn't implement all Wayland protocols GTK needs, so GTK apps
        // fail on Wayland. XWayland works perfectly. Native Wayland apps (swayimg, etc.)
        // ignore these variables and connect directly.
        env.insert("GDK_BACKEND".to_string(), "x11".to_string());
        env.insert("QT_QPA_PLATFORM".to_string(), "xcb".to_string());
        if let Ok(shell) = std::env::var("SHELL") {
            env.insert("SHELL".to_string(), shell);
        }
        // XDG_RUNTIME_DIR is critical for Wayland - socket lives here
        if let Ok(xdg_runtime) = std::env::var("XDG_RUNTIME_DIR") {
            env.insert("XDG_RUNTIME_DIR".to_string(), xdg_runtime);
        }
        // HOME is needed for many apps
        if let Ok(home) = std::env::var("HOME") {
            env.insert("HOME".to_string(), home);
        }
        // USER and LOGNAME for identity
        if let Ok(user) = std::env::var("USER") {
            env.insert("USER".to_string(), user);
        }
        if let Ok(logname) = std::env::var("LOGNAME") {
            env.insert("LOGNAME".to_string(), logname);
        }

        tracing::debug!(
            display = ?env.get("DISPLAY"),
            xauthority = ?env.get("XAUTHORITY"),
            gdk_backend = ?env.get("GDK_BACKEND"),
            command = %request.command,
            "GUI spawn environment"
        );

        // Create output terminal with WaitingForOutput visibility
        let parent = launching_terminal;
        match terminal_manager.spawn_command(&request.prompt, &request.command, &request.cwd, &env, parent) {
            Ok(output_terminal_id) => {
                tracing::info!(
                    output_terminal_id = output_terminal_id.0,
                    launching_terminal = ?launching_terminal,
                    foreground,
                    command = %request.command,
                    "spawned GUI command terminal"
                );

                // Add output terminal to layout
                compositor.add_terminal(output_terminal_id);

                // Set up for window linking
                compositor.pending_window_output_terminal = Some(output_terminal_id);
                compositor.pending_window_command = Some(request.command.clone());
                compositor.pending_gui_foreground = foreground;

                // If foreground mode, hide launching terminal and track the session
                if foreground {
                    if let Some(launcher_id) = launching_terminal {
                        if let Some(launcher) = terminal_manager.get_mut(launcher_id) {
                            launcher.visibility.hide_for_gui();
                            tracing::info!(
                                launcher_id = launcher_id.0,
                                "hid launching terminal for foreground GUI"
                            );
                        }

                        // Track the session: output_terminal_id -> (launcher_id, window_was_linked=false)
                        compositor.foreground_gui_sessions.insert(
                            output_terminal_id,
                            (launcher_id, false),
                        );
                    }
                }

                // Update cell heights
                let new_heights = calculate_window_heights(compositor, terminal_manager);
                compositor.update_layout_heights(new_heights);

                // spawn_command auto-focuses the new terminal, but for GUI spawns we want different behavior:
                // - Foreground mode: GUI window will get focus when created (in add_window)
                // - Background mode: focus stays on launcher terminal
                //
                // In both cases, restore focus to the launcher terminal now.
                // For foreground mode, add_window will focus the GUI window when it's created.
                if let Some(launcher_id) = launching_terminal {
                    compositor.focused_window = Some(FocusedWindow::Terminal(launcher_id));
                    tracing::debug!(
                        launcher_id = launcher_id.0,
                        "restored terminal focus to launcher after gui_spawn"
                    );
                }
            }
            Err(e) => {
                tracing::error!(command = %request.command, error = ?e, "failed to spawn GUI command");
            }
        }
    }
}

/// Handle builtin command requests from IPC (termstack --builtin)
///
/// Creates persistent stack entries for shell builtins like cd, export, alias, etc.
/// These show the command and its output (if any) as a static terminal entry.
pub fn handle_builtin_requests(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
    calculate_window_heights: impl Fn(&TermStack, &TerminalManager) -> Vec<i32>,
) {
    while let Some(request) = compositor.pending_builtin_requests.pop() {
        // Create static terminal with the builtin command and result
        match terminal_manager.create_builtin_terminal(
            &request.prompt,
            &request.command,
            &request.result,
            request.success,
        ) {
            Ok(id) => {
                // Find the launcher terminal (last terminal in layout)
                // Builtins should appear above the launcher
                let launcher_idx = compositor.layout_nodes.iter()
                    .rposition(|node| matches!(node.cell, StackWindow::Terminal(_)))
                    .unwrap_or(compositor.layout_nodes.len());

                // Insert above launcher (at launcher's position, pushing launcher down)
                compositor.layout_nodes.insert(launcher_idx, super::state::LayoutNode {
                    cell: StackWindow::Terminal(id),
                    height: 0, // Will be updated in calculate_window_heights
                });

                tracing::info!(
                    id = id.0,
                    insert_index = launcher_idx,
                    command = %request.command,
                    success = request.success,
                    "inserted builtin terminal"
                );

                // Update cell heights
                let new_heights = calculate_window_heights(compositor, terminal_manager);
                compositor.update_layout_heights(new_heights);

                // Scroll to show the builtin entry
                if let Some(new_scroll) = compositor.scroll_to_show_window_bottom(launcher_idx) {
                    tracing::debug!(
                        id = id.0,
                        new_scroll,
                        "scrolled to show builtin terminal"
                    );
                }
            }
            Err(e) => {
                tracing::error!(
                    command = %request.command,
                    error = ?e,
                    "failed to create builtin terminal"
                );
            }
        }
    }
}

/// Process a spawn request for a terminal command
///
/// Sets up environment variables, adds helper scripts to PATH, and spawns
/// the terminal. Returns the terminal ID on success.
fn process_spawn_request(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
    request: SpawnRequest,
) -> Option<TerminalId> {
    // Decide what command to run
    // Title bar now shows the command, so no need for echo prefix
    let command = if request.command.is_empty() {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    } else {
        request.command.clone()
    };

    // Modify environment
    let mut env = request.env.clone();
    env.insert("GIT_PAGER".to_string(), "cat".to_string());
    env.insert("PAGER".to_string(), "cat".to_string());
    env.insert("LESS".to_string(), "-FRX".to_string());

    // For regular terminal spawns (not gui spawns), use host display so GUI windows
    // appear on the host desktop. Only 'gui' prefix should bring windows into termstack.
    if let Ok(host_wayland) = std::env::var("HOST_WAYLAND_DISPLAY") {
        env.insert("WAYLAND_DISPLAY".to_string(), host_wayland.clone());
        env.insert("HOST_WAYLAND_DISPLAY".to_string(), host_wayland);
    }
    if let Ok(host_x11) = std::env::var("HOST_DISPLAY") {
        env.insert("DISPLAY".to_string(), host_x11.clone());
        env.insert("HOST_DISPLAY".to_string(), host_x11);
    }
    // Don't force GTK/Qt backend - let apps use host defaults
    env.remove("GDK_BACKEND");
    env.remove("QT_QPA_PLATFORM");
    // Pass SHELL so spawn_command uses the correct shell for syntax
    // This ensures fish loops work when user's shell is fish
    if let Ok(shell) = std::env::var("SHELL") {
        env.insert("SHELL".to_string(), shell);
    }

    // Add scripts directory to PATH so helper scripts (like 'gui') are available
    // Scripts are extracted from embedded content at runtime
    match get_or_create_scripts_dir() {
        Ok(scripts_dir) => {
            let scripts_dir_str = scripts_dir.display().to_string();
            if let Some(current_path) = env.get("PATH") {
                // Prepend scripts directory to existing PATH
                env.insert("PATH".to_string(), format!("{}:{}", scripts_dir_str, current_path));
            } else {
                // No PATH set, create one with just scripts directory
                env.insert("PATH".to_string(), scripts_dir_str);
            }
        }
        Err(e) => {
            // Log warning but don't fail spawn - terminal will work, just without helper scripts
            tracing::warn!(?e, "failed to create scripts directory, helper scripts unavailable");
        }
    }

    let parent = compositor.focused_window.as_ref().and_then(|cell| match cell {
        FocusedWindow::Terminal(id) => Some(*id),
        FocusedWindow::External(_) => None,
    });

    // Reject spawns from alternate screen terminals (TUI apps)
    if let Some(parent_id) = parent {
        if let Some(parent_term) = terminal_manager.get(parent_id) {
            if parent_term.terminal.is_alternate_screen() {
                tracing::info!(command = %command, "rejecting spawn from alternate screen terminal");
                return None;
            }
        }
    }

    tracing::info!(
        command = %command,
        ?parent,
        "spawning command terminal"
    );

    match terminal_manager.spawn_command(&request.prompt, &command, &request.cwd, &env, parent) {
        Ok(id) => {
            if let Some(term) = terminal_manager.get(id) {
                let (cols, pty_rows) = term.terminal.dimensions();
                tracing::info!(id = id.0, cols, pty_rows, height = term.height, "terminal created");
            }
            compositor.add_terminal(id);

            // Set this terminal as the pending output terminal for GUI windows,
            // but ONLY if no pending value is already set. This protects against
            // a race where a GUI spawn sets pending_window_output_terminal, then
            // a regular spawn (user typing elsewhere) overwrites it before the
            // GUI window connects.
            if compositor.pending_window_output_terminal.is_none() {
                compositor.pending_window_output_terminal = Some(id);
                compositor.pending_window_command = Some(request.command.clone());
                tracing::info!(id = id.0, command = %request.command, "set as pending output terminal for GUI windows");
            } else {
                tracing::debug!(
                    id = id.0,
                    existing = ?compositor.pending_window_output_terminal,
                    "not overwriting existing pending_window_output_terminal"
                );
            }

            Some(id)
        }
        Err(e) => {
            tracing::error!(error = ?e, "failed to spawn command terminal");
            None
        }
    }
}

/// Get or create the termstack scripts directory and extract embedded scripts
///
/// Scripts are extracted from `scripts/bin/*` (embedded at compile time) and written
/// to a runtime directory. This directory should be added to PATH for spawned terminals.
///
/// Returns the PathBuf to the scripts directory that should be added to PATH.
fn get_or_create_scripts_dir() -> Result<PathBuf, std::io::Error> {
    use std::os::unix::fs::PermissionsExt;

    // Determine runtime directory: Try XDG_RUNTIME_DIR first, fall back to /tmp
    let base_dir = if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(xdg)
    } else {
        // Fall back to /tmp with user ID for isolation
        let uid = rustix::process::getuid().as_raw();
        PathBuf::from(format!("/tmp/termstack-{}", uid))
    };

    let scripts_dir = base_dir.join("termstack").join("bin");

    // Create directory if it doesn't exist
    std::fs::create_dir_all(&scripts_dir)?;

    // Extract and write all scripts from scripts/bin/*
    // Currently we have: gui
    // Future scripts can be added here as additional include_str! calls
    let scripts = vec![
        ("gui", include_str!("../../../scripts/bin/gui")),
    ];

    for (name, content) in scripts {
        let script_path = scripts_dir.join(name);

        // Always write to ensure script is up-to-date with current version
        std::fs::write(&script_path, content)?;

        // Make executable (0o755 = rwxr-xr-x)
        let mut perms = std::fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms)?;

        tracing::debug!(path = ?script_path, "extracted script");
    }

    tracing::debug!(scripts_dir = ?scripts_dir, "scripts directory initialized");
    Ok(scripts_dir)
}

#[cfg(test)]
mod tests {
    use crate::terminal_manager::TerminalId;

    /// Test the guard condition for not overwriting pending_window_output_terminal.
    /// This prevents race conditions where a regular spawn could overwrite a GUI spawn's value.
    #[test]
    fn pending_output_terminal_guard_prevents_overwrite() {
        // Simulate compositor state - GUI spawn already set the value
        let gui_terminal = TerminalId(1);
        let mut pending_window_output_terminal: Option<TerminalId> = Some(gui_terminal);
        assert_eq!(pending_window_output_terminal, Some(TerminalId(1)));

        // Regular spawn tries to set - but guard prevents overwrite
        let regular_terminal = TerminalId(2);
        if pending_window_output_terminal.is_none() {
            pending_window_output_terminal = Some(regular_terminal);
        }

        // Original GUI terminal value should be preserved
        assert_eq!(
            pending_window_output_terminal,
            Some(TerminalId(1)),
            "regular spawn should not overwrite GUI spawn's pending_window_output_terminal"
        );
    }

    /// Test that regular spawn CAN set the value when nothing is pending.
    /// This is the normal case for commands that might open GUI windows.
    #[test]
    fn regular_spawn_sets_value_when_none() {
        let mut pending_window_output_terminal: Option<TerminalId> = None;

        // Regular spawn with no pending value
        let regular_terminal = TerminalId(5);
        if pending_window_output_terminal.is_none() {
            pending_window_output_terminal = Some(regular_terminal);
        }

        // Should be set
        assert_eq!(pending_window_output_terminal, Some(TerminalId(5)));
    }

    /// Test the race condition scenario end-to-end.
    /// 1. GUI spawn sets pending
    /// 2. User types command in another terminal (triggers regular spawn)
    /// 3. GUI window arrives - should still find correct output terminal
    #[test]
    fn race_condition_scenario() {
        // Step 1: GUI spawn sets the pending value
        let gui_output_terminal = TerminalId(10);
        let mut pending_window_output_terminal: Option<TerminalId> = Some(gui_output_terminal);

        // Step 2: Regular spawn (user typing elsewhere) - guard prevents overwrite
        let user_command_terminal = TerminalId(11);
        if pending_window_output_terminal.is_none() {
            pending_window_output_terminal = Some(user_command_terminal);
        }

        // Step 3: GUI window arrives - reads the value
        let output_terminal_for_window = pending_window_output_terminal;

        // Window should be linked to the GUI's output terminal, not the user's command
        assert_eq!(
            output_terminal_for_window,
            Some(TerminalId(10)),
            "GUI window should be linked to GUI's output terminal"
        );
    }
}
