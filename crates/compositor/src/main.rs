//! Column Compositor - A content-aware terminal compositor
//!
//! This compositor arranges terminal windows in a scrollable vertical column,
//! with windows dynamically sizing based on their content.

use std::os::unix::net::UnixListener;
use std::time::Duration;

use smithay::backend::winit::{self, WinitEvent};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{Color32F, Frame, Renderer};
use smithay::desktop::utils::send_frames_surface_tree;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::{EventLoop, generic::Generic, Interest, Mode as CalloopMode};
use smithay::reexports::wayland_server::Display;
use smithay::utils::{Physical, Rectangle, Scale, Size, Transform};
use smithay::wayland::socket::ListeningSocketSource;

use compositor::config::Config;
use compositor::render::{
    CellRenderData, prerender_terminals, prerender_title_bars,
    collect_cell_data, build_render_data, log_frame_state,
    heights_changed_significantly, render_terminal, render_external,
};
use compositor::state::{ClientState, ColumnCell, ColumnCompositor};
use compositor::terminal_manager::{TerminalId, TerminalManager};
use compositor::title_bar::{TitleBarRenderer, TITLE_BAR_HEIGHT};

/// Minimum terminal height in rows.
/// Prevents terminals from becoming too small to be usable.
const MIN_TERMINAL_ROWS: u16 = 3;

/// Extra rows to add beyond cursor position for content-based sizing.
/// Accounts for: +1 for 0-indexed cursor line, +1 for shell prompt.
const CURSOR_TO_CONTENT_OFFSET: u16 = 2;

fn main() -> anyhow::Result<()> {
    // Initialize logging
    setup_logging();

    tracing::info!("starting column-compositor");

    // Load configuration
    let config = Config::load();

    // Create event loop
    let mut event_loop: EventLoop<ColumnCompositor> = EventLoop::try_new()?;

    // Create Wayland display
    let display: Display<ColumnCompositor> = Display::new()?;

    // Initialize winit backend
    let (mut backend, mut winit_event_loop) = winit::init::<GlesRenderer>()
        .map_err(|e| anyhow::anyhow!("winit init error: {e:?}"))?;

    let mode = Mode {
        size: backend.window_size(),
        refresh: 60_000,
    };

    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Smithay".to_string(),
            model: "Winit".to_string(),
        },
    );
    output.change_current_state(Some(mode), Some(Transform::Normal), None, Some((0, 0).into()));
    output.set_preferred(mode);

    // Convert logical to physical size
    let output_size: Size<i32, Physical> = Size::from((mode.size.w, mode.size.h));

    // Create compositor state (keep display separate for dispatching)
    let (mut compositor, mut display) = ColumnCompositor::new(
        display,
        event_loop.handle(),
        output_size,
    );

    // Add output to compositor
    compositor.space.map_output(&output, (0, 0));

    // Create output global so clients can discover it
    let _output_global = output.create_global::<ColumnCompositor>(&compositor.display_handle);

    // Create listening socket
    let listening_socket = ListeningSocketSource::new_auto()
        .expect("failed to create Wayland socket");

    let socket_name = listening_socket
        .socket_name()
        .to_string_lossy()
        .to_string();

    tracing::info!(?socket_name, "listening on Wayland socket");

    // Save original WAYLAND_DISPLAY for running apps on host
    let host_wayland_display = std::env::var("WAYLAND_DISPLAY").ok();
    if let Some(ref host) = host_wayland_display {
        std::env::set_var("HOST_WAYLAND_DISPLAY", host);
        tracing::info!(host_display = ?host, "saved host WAYLAND_DISPLAY");
    }

    // Set WAYLAND_DISPLAY for child processes (apps will open inside compositor)
    std::env::set_var("WAYLAND_DISPLAY", &socket_name);

    // Force GTK and Qt apps to use Wayland backend (otherwise they may use X11/Xwayland)
    std::env::set_var("GDK_BACKEND", "wayland");
    std::env::set_var("QT_QPA_PLATFORM", "wayland");

    // Insert socket source into event loop for new client connections
    event_loop.handle().insert_source(listening_socket, |client_stream, _, state| {
        tracing::info!("new Wayland client connected");
        state.display_handle.insert_client(client_stream, std::sync::Arc::new(ClientState {
            compositor_state: Default::default(),
        })).expect("failed to insert client");
    }).expect("failed to insert socket source");

    // Create IPC socket for column-term commands
    let ipc_socket_path = compositor::ipc::socket_path();
    let _ = std::fs::remove_file(&ipc_socket_path); // Clean up old socket
    let ipc_listener = UnixListener::bind(&ipc_socket_path)
        .expect("failed to create IPC socket");
    ipc_listener.set_nonblocking(true).expect("failed to set nonblocking");

    // Set environment variable for child processes
    std::env::set_var("COLUMN_COMPOSITOR_SOCKET", &ipc_socket_path);

    tracing::info!(path = ?ipc_socket_path, "IPC socket created");

    // Insert IPC socket source into event loop
    event_loop.handle().insert_source(
        Generic::new(ipc_listener, Interest::READ, CalloopMode::Level),
        |_, listener, state| {
            // Accept incoming connections
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        tracing::info!("IPC connection received");
                        match compositor::ipc::read_ipc_request(stream) {
                            Ok((request, stream)) => {
                                match request {
                                    compositor::ipc::IpcRequest::Spawn(spawn_req) => {
                                        tracing::info!(command = %spawn_req.command, "IPC spawn request queued");
                                        state.pending_spawn_requests.push(spawn_req);
                                        // Spawn doesn't need ACK - it's fire-and-forget
                                    }
                                    compositor::ipc::IpcRequest::Resize(mode) => {
                                        tracing::info!(?mode, "IPC resize request queued");
                                        // Store stream for ACK after resize completes
                                        state.pending_resize_request = Some((mode, stream));
                                    }
                                }
                            }
                            Err(compositor::ipc::IpcError::Timeout) => {
                                tracing::debug!("IPC read timeout");
                            }
                            Err(compositor::ipc::IpcError::EmptyMessage) => {
                                tracing::debug!("IPC received empty message");
                            }
                            Err(e) => {
                                tracing::warn!("IPC request error: {}", e);
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        tracing::warn!("IPC accept error: {}", e);
                        break;
                    }
                }
            }
            Ok(smithay::reexports::calloop::PostAction::Continue)
        },
    ).expect("failed to insert IPC socket source");

    tracing::info!("entering main loop");

    let bg_color = Color32F::new(
        config.background_color[0],
        config.background_color[1],
        config.background_color[2],
        config.background_color[3],
    );

    // Create terminal manager with output size
    let mut terminal_manager = TerminalManager::new_with_size(
        output_size.w as u32,
        output_size.h as u32,
    );

    // Create title bar renderer for external windows
    let mut title_bar_renderer = TitleBarRenderer::new();
    if title_bar_renderer.is_none() {
        tracing::warn!("Title bar renderer unavailable - no font found");
    }

    // Spawn initial terminal
    match terminal_manager.spawn() {
        Ok(id) => {
            compositor.add_terminal(id);
            tracing::info!(id = id.0, "spawned initial terminal");
        }
        Err(e) => {
            tracing::error!("failed to spawn initial terminal: {}", e);
        }
    }

    // Main event loop
    while compositor.running {
        // Handle external window insert/resize events
        handle_external_window_events(&mut compositor);

        // Update cell heights for input event processing
        let cell_heights = calculate_cell_heights(&compositor, &terminal_manager);
        compositor.update_layout_heights(cell_heights);

        // Update Space positions to match current terminal height and scroll
        // This ensures Space.element_under works correctly for click detection
        compositor.recalculate_layout();

        // Dispatch winit events
        let _ = winit_event_loop.dispatch_new_events(|event| {
            tracing::debug!("winit event: {:?}", std::mem::discriminant(&event));
            if let WinitEvent::Input(input_event) = &event {
                tracing::debug!("winit input event: {:?}", std::mem::discriminant(input_event));
            }
            match event {
            WinitEvent::Resized { size, .. } => {
                output.change_current_state(
                    Some(Mode {
                        size,
                        refresh: 60_000,
                    }),
                    None,
                    None,
                    None,
                );
                compositor.output_size = Size::from((size.w, size.h));

                // Update terminal manager dimensions
                terminal_manager.update_output_size(size.w as u32, size.h as u32);

                // Resize all existing terminals to new width
                terminal_manager.resize_all_terminals(size.w as u32);

                // Resize all external windows to new width
                compositor.resize_all_external_windows(size.w);

                compositor.recalculate_layout();

                tracing::info!(
                    width = size.w,
                    height = size.h,
                    "compositor window resized, content updated"
                );
            }
            WinitEvent::Input(event) => compositor.process_input_event_with_terminals(event, &mut terminal_manager),
            WinitEvent::Focus(focused) => {
                tracing::info!("window focus changed: {}", focused);
            }
            WinitEvent::Redraw => {}
            WinitEvent::CloseRequested => {
                compositor.running = false;
            }
        }});

        if !compositor.running {
            break;
        }

        // Dispatch Wayland client requests
        display.dispatch_clients(&mut compositor)
            .expect("failed to dispatch clients");

        // Handle terminal spawn requests (keyboard shortcut)
        handle_terminal_spawn(&mut compositor, &mut terminal_manager);

        // Handle command spawn requests from IPC (column-term)
        handle_ipc_spawn_requests(&mut compositor, &mut terminal_manager);

        // Handle resize requests from IPC (column-term --resize)
        handle_ipc_resize_request(&mut compositor, &mut terminal_manager);

        // Handle key repeat for terminals
        handle_key_repeat(&mut compositor, &mut terminal_manager);

        // Handle focus change requests
        if compositor.focus_change_requested != 0 {
            if compositor.focus_change_requested > 0 {
                compositor.focus_next();
            } else {
                compositor.focus_prev();
            }
            compositor.focus_change_requested = 0;

            // Scroll to show the newly focused cell
            if let Some(focused_idx) = compositor.focused_index {
                compositor.scroll_to_show_cell_bottom(focused_idx);
            }
        }

        // Handle scroll requests
        if compositor.scroll_requested != 0.0 {
            let delta = compositor.scroll_requested;
            compositor.scroll_requested = 0.0;
            compositor.scroll(delta);
            tracing::debug!(
                new_offset = compositor.scroll_offset,
                "scroll processed"
            );
        }

        // Process terminal PTY output and handle sizing actions
        process_terminal_output(&mut compositor, &mut terminal_manager);

        // Promote output terminals that have content to standalone cells
        promote_output_terminals(&mut compositor, &terminal_manager);

        // Handle cleanup of output terminals from closed windows
        handle_output_terminal_cleanup(&mut compositor, &mut terminal_manager);

        // Cleanup dead terminals and handle focus changes
        if cleanup_and_sync_focus(&mut compositor, &mut terminal_manager) {
            break;
        }

        // Get window size before binding
        let size = backend.window_size();
        let physical_size: Size<i32, Physical> = Size::from((size.w, size.h));

        #[allow(deprecated)]
        let damage: Rectangle<i32, Physical> = Rectangle::from_loc_and_size(
            (0, 0),
            (size.w, size.h),
        );

        // Render frame - bind returns (&mut Renderer, Framebuffer)
        {
            let (renderer, mut framebuffer) = backend.bind()
                .map_err(|e| anyhow::anyhow!("bind error: {e:?}"))?;

            let scale = Scale::from(1.0);

            // Pre-render all terminal textures
            prerender_terminals(&mut terminal_manager, renderer);

            // Pre-render title bar textures for all cells with SSD
            let title_bar_textures = prerender_title_bars(
                &compositor.layout_nodes,
                &mut title_bar_renderer,
                &terminal_manager,
                renderer,
                physical_size.w,
            );

            // Collect actual heights and external window elements
            let (actual_heights, mut external_elements) = collect_cell_data(
                &compositor.layout_nodes,
                &terminal_manager,
                renderer,
                scale,
            );

            // Build render data with computed Y positions
            let render_data = build_render_data(
                &compositor.layout_nodes,
                &actual_heights,
                &mut external_elements,
                &title_bar_textures,
                compositor.scroll_offset,
                physical_size.h,
                &terminal_manager,
            );

            // Debug logging for external windows
            log_frame_state(
                &compositor.layout_nodes,
                &render_data,
                &terminal_manager,
                compositor.scroll_offset,
                compositor.focused_index,
                physical_size.h,
            );

            // Check if heights changed significantly and update cache
            // We use the previous frame's layout heights for comparison
            let current_heights: Vec<i32> = compositor.layout_nodes.iter().map(|n| n.height).collect();
            let heights_changed = heights_changed_significantly(
                &current_heights,
                &actual_heights,
                compositor.focused_index,
            );

            compositor.update_layout_heights(actual_heights);

            // Adjust scroll if heights changed
            if heights_changed {
                if let Some(focused_idx) = compositor.focused_index {
                    if let Some(new_scroll) = compositor.scroll_to_show_cell_bottom(focused_idx) {
                        tracing::info!(
                            focused_idx,
                            new_scroll,
                            "scroll adjusted due to actual height change"
                        );
                    }
                }
            }

            // Begin actual rendering
            let mut frame = renderer.render(&mut framebuffer, physical_size, Transform::Normal)
                .map_err(|e| anyhow::anyhow!("render error: {e:?}"))?;

            frame.clear(bg_color, &[damage])
                .map_err(|e| anyhow::anyhow!("clear error: {e:?}"))?;

            // Render all cells
            for (cell_idx, data) in render_data.into_iter().enumerate() {
                let is_focused = compositor.focused_index == Some(cell_idx);

                match data {
                    CellRenderData::Terminal { id, y, height, title_bar_texture } => {
                        render_terminal(
                            &mut frame,
                            &terminal_manager,
                            id,
                            y,
                            height,
                            title_bar_texture,
                            is_focused,
                            physical_size,
                            damage,
                        );
                    }
                    CellRenderData::External { y, height, elements, title_bar_texture } => {
                        render_external(
                            &mut frame,
                            y,
                            height,
                            elements,
                            title_bar_texture,
                            is_focused,
                            physical_size,
                            damage,
                            scale,
                        );
                    }
                }
            }
        }

        backend.submit(Some(&[damage]))?;

        // Send frame callbacks to all toplevel surfaces
        for surface in compositor.xdg_shell_state.toplevel_surfaces() {
            send_frames_surface_tree(
                surface.wl_surface(),
                &output,
                Duration::ZERO,
                None,
                |_, _| Some(output.clone()),
            );
        }

        // Flush clients
        compositor.display_handle.flush_clients()?;

        // Dispatch calloop events
        event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut compositor)
            .map_err(|e| anyhow::anyhow!("event loop error: {e}"))?;
    }

    tracing::info!("compositor shutting down");

    Ok(())
}

fn setup_logging() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,smithay=warn"));

    // Respect NO_COLOR environment variable for testing
    let use_ansi = std::env::var("NO_COLOR").is_err();

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_target(true)
                .with_line_number(true)
                .with_ansi(use_ansi),
        )
        .with(filter)
        .init();
}

/// Handle external window insert and resize events.
///
/// Updates cached heights and scroll position when external windows
/// are added or resized.
fn handle_external_window_events(compositor: &mut ColumnCompositor) {
    // Handle new external window - heights are already managed in add_window,
    // just need to scroll
    if let Some(window_idx) = compositor.new_external_window_index.take() {
        tracing::info!(
            window_idx,
            cells_count = compositor.layout_nodes.len(),
            focused_index = ?compositor.focused_index,
            "handling new external window"
        );

        // Scroll to show the focused cell
        if let Some(focused_idx) = compositor.focused_index {
            if let Some(new_scroll) = compositor.scroll_to_show_cell_bottom(focused_idx) {
                tracing::info!(
                    window_idx,
                    focused_idx,
                    new_scroll,
                    "scrolled to show focused cell after external window added"
                );
            }
        }
    }

    // Handle external window resize
    if let Some((resized_idx, new_height)) = compositor.external_window_resized.take() {
        tracing::info!(
            resized_idx,
            new_height,
            "handling external window resize"
        );

        if let Some(node) = compositor.layout_nodes.get_mut(resized_idx) {
            node.height = new_height;
        }

        if let Some(focused_idx) = compositor.focused_index {
            if focused_idx >= resized_idx {
                if let Some(new_scroll) = compositor.scroll_to_show_cell_bottom(focused_idx) {
                    tracing::info!(
                        resized_idx,
                        focused_idx,
                        new_scroll,
                        "scrolled to show focused cell after external window resize"
                    );
                }
            }
        }
    }
}

/// Calculate cell heights for layout.
///
/// For terminals: hidden terminals always get 0 height; otherwise uses cached
/// height if available, falls back to terminal.height for new cells.
///
/// For external windows: uses cached height if available, otherwise computes
/// from window state (including title bar height for SSD windows).
fn calculate_cell_heights(
    compositor: &ColumnCompositor,
    terminal_manager: &TerminalManager,
) -> Vec<i32> {
    compositor.layout_nodes.iter().map(|node| {
        match &node.cell {
            ColumnCell::Terminal(tid) => {
                // Hidden terminals always get 0 height
                if terminal_manager.get(*tid).map(|t| t.hidden).unwrap_or(false) {
                    return 0;
                }
                // Use cached height if available
                if node.height > 0 {
                    return node.height;
                }
                // Fallback for new cells
                terminal_manager.get(*tid)
                    .map(|t| t.height as i32)
                    .unwrap_or(200)
            }
            ColumnCell::External(entry) => {
                // Use cached height if available
                if node.height > 0 {
                    return node.height;
                }
                // Include title bar height for SSD windows only
                let base_height = entry.state.current_height() as i32;
                if entry.uses_csd {
                    base_height
                } else {
                    base_height + TITLE_BAR_HEIGHT as i32
                }
            }
        }
    }).collect()
}

/// Handle keyboard shortcut to spawn a new terminal.
fn handle_terminal_spawn(
    compositor: &mut ColumnCompositor,
    terminal_manager: &mut TerminalManager,
) {
    if !compositor.spawn_terminal_requested {
        return;
    }
    compositor.spawn_terminal_requested = false;

    match terminal_manager.spawn() {
        Ok(id) => {
            compositor.add_terminal(id);

            // Update cell heights
            let new_heights = calculate_cell_heights(compositor, terminal_manager);
            compositor.update_layout_heights(new_heights);

            // Scroll to show the new terminal
            if let Some(focused_idx) = compositor.focused_index {
                if let Some(new_scroll) = compositor.scroll_to_show_cell_bottom(focused_idx) {
                    tracing::info!(
                        id = id.0,
                        cell_count = compositor.layout_nodes.len(),
                        focused_idx,
                        new_scroll,
                        "spawned terminal, scrolling to show"
                    );
                }
            }
        }
        Err(e) => {
            tracing::error!("failed to spawn terminal: {}", e);
        }
    }
}

/// Handle IPC spawn requests from column-term.
///
/// Processes pending spawn requests, sets up environment, and spawns
/// command terminals with proper parent hiding.
fn handle_ipc_spawn_requests(
    compositor: &mut ColumnCompositor,
    terminal_manager: &mut TerminalManager,
) {
    while let Some(request) = compositor.pending_spawn_requests.pop() {
        if let Some(id) = process_spawn_request(compositor, terminal_manager, request) {
            // Focus the new command terminal
            for (i, node) in compositor.layout_nodes.iter().enumerate() {
                if let ColumnCell::Terminal(tid) = node.cell {
                    if tid == id {
                        compositor.focused_index = Some(i);
                        tracing::info!(id = id.0, index = i, "focused new command terminal");
                        break;
                    }
                }
            }

            // Update cell heights
            let new_heights = calculate_cell_heights(compositor, terminal_manager);
            compositor.update_layout_heights(new_heights);

            // Scroll to show the new terminal
            if let Some(focused_idx) = compositor.focused_index {
                if let Some(new_scroll) = compositor.scroll_to_show_cell_bottom(focused_idx) {
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

/// Handle key repeat for terminal input.
///
/// When a key is held down, this sends repeat events at regular intervals.
fn handle_key_repeat(
    compositor: &mut ColumnCompositor,
    terminal_manager: &mut TerminalManager,
) {
    let Some((ref bytes, next_repeat)) = compositor.key_repeat else {
        return;
    };

    let now = std::time::Instant::now();
    if now < next_repeat {
        return;
    }

    // Time to send a repeat event
    let bytes_to_send = bytes.clone();

    if let Some(terminal) = terminal_manager.get_focused_mut() {
        if let Err(e) = terminal.write(&bytes_to_send) {
            tracing::error!(?e, "failed to write repeat to terminal");
            compositor.key_repeat = None;
            return;
        }
    } else {
        // No focused terminal, stop repeating
        compositor.key_repeat = None;
        return;
    }

    // Schedule next repeat
    let next = now + std::time::Duration::from_millis(compositor.repeat_interval_ms);
    compositor.key_repeat = Some((bytes_to_send, next));
}

/// Process a single spawn request, returning the new terminal ID if successful.
fn process_spawn_request(
    compositor: &mut ColumnCompositor,
    terminal_manager: &mut TerminalManager,
    request: compositor::ipc::SpawnRequest,
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
    if let Ok(wayland_display) = std::env::var("WAYLAND_DISPLAY") {
        env.insert("WAYLAND_DISPLAY".to_string(), wayland_display);
    }
    // Force GTK/Qt apps to use Wayland backend
    if let Ok(gdk_backend) = std::env::var("GDK_BACKEND") {
        env.insert("GDK_BACKEND".to_string(), gdk_backend);
    }
    if let Ok(qt_platform) = std::env::var("QT_QPA_PLATFORM") {
        env.insert("QT_QPA_PLATFORM".to_string(), qt_platform);
    }
    // Pass SHELL so spawn_command uses the correct shell for syntax
    // This ensures fish loops work when user's shell is fish
    if let Ok(shell) = std::env::var("SHELL") {
        env.insert("SHELL".to_string(), shell);
    }

    let parent = terminal_manager.focused;

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

    match terminal_manager.spawn_command(&command, &request.cwd, &env, parent) {
        Ok(id) => {
            if let Some(term) = terminal_manager.get(id) {
                let (cols, pty_rows) = term.terminal.dimensions();
                tracing::info!(id = id.0, cols, pty_rows, height = term.height, "terminal created");
            }
            compositor.add_terminal(id);

            // Set this terminal as the pending output terminal for GUI windows.
            // If the command opens a GUI window, that window will be linked to this terminal.
            // The terminal will be hidden until it has output, then promoted to a standalone cell.
            compositor.pending_window_output_terminal = Some(id);
            compositor.pending_window_command = Some(request.command.clone());
            tracing::info!(id = id.0, command = %request.command, "set as pending output terminal for GUI windows");

            Some(id)
        }
        Err(e) => {
            tracing::error!("failed to spawn command terminal: {}", e);
            None
        }
    }
}

/// Process terminal PTY output and handle sizing actions.
///
/// Processes all terminal output, handles growth requests, and auto-resizes
/// terminals that enter alternate screen mode.
fn process_terminal_output(
    compositor: &mut ColumnCompositor,
    terminal_manager: &mut TerminalManager,
) {
    // Process PTY output and get sizing actions
    let sizing_actions = terminal_manager.process_all();

    // Handle sizing actions
    for (id, action) in sizing_actions {
        if let terminal::sizing::SizingAction::RequestGrowth { target_rows } = action {
            tracing::info!(id = id.0, target_rows, "processing growth request");
            terminal_manager.grow_terminal(id, target_rows);

            // If focused terminal grew, update cache and scroll
            if terminal_manager.focused == Some(id) {
                if let Some(idx) = find_terminal_cell_index(compositor, id) {
                    if let Some(term) = terminal_manager.get(id) {
                        if let Some(node) = compositor.layout_nodes.get_mut(idx) {
                            node.height = term.height as i32;
                        }
                    }
                    compositor.scroll_to_show_cell_bottom(idx);
                }
            }
        }
    }

    // Auto-resize terminals entering alternate screen mode
    auto_resize_alt_screen_terminals(compositor, terminal_manager);
}

/// Auto-resize terminals that entered alternate screen mode.
fn auto_resize_alt_screen_terminals(
    compositor: &mut ColumnCompositor,
    terminal_manager: &mut TerminalManager,
) {
    let max_height = terminal_manager.max_rows as u32 * terminal_manager.cell_height;
    // Check ALL terminals, not just visible ones - TUI apps like fzf enter
    // alternate screen before producing content_rows, so they'd be hidden
    let all_ids = terminal_manager.ids();

    let mut ids_to_resize = Vec::new();
    for id in all_ids {
        if let Some(term) = terminal_manager.get_mut(id) {
            if term.check_alt_screen_resize_needed(max_height) {
                ids_to_resize.push(id);
            }
        }
    }

    let max_rows = terminal_manager.max_rows;
    let cell_height = terminal_manager.cell_height;

    for id in ids_to_resize {
        if let Some(term) = terminal_manager.get_mut(id) {
            let old_height = term.height;
            term.resize(max_rows, cell_height);
            let new_height = term.height;

            tracing::info!(
                id = id.0,
                old_height,
                new_height,
                "auto-resized terminal for alternate screen"
            );

            // Update cached height
            if let Some(idx) = find_terminal_cell_index(compositor, id) {
                if let Some(node) = compositor.layout_nodes.get_mut(idx) {
                    node.height = new_height as i32;
                }
            }
        }
    }
}

/// Find the cell index for a terminal ID.
fn find_terminal_cell_index(compositor: &ColumnCompositor, id: TerminalId) -> Option<usize> {
    compositor.layout_nodes.iter().enumerate().find_map(|(i, node)| {
        if let ColumnCell::Terminal(tid) = node.cell {
            if tid == id {
                return Some(i);
            }
        }
        None
    })
}

/// Handle IPC resize request from column-term --resize.
///
/// Resizes the focused terminal to full or content-based height and
/// sends ACK to unblock the column-term process.
fn handle_ipc_resize_request(
    compositor: &mut ColumnCompositor,
    terminal_manager: &mut TerminalManager,
) {
    let Some((resize_mode, ack_stream)) = compositor.pending_resize_request.take() else {
        return;
    };

    let Some(focused_id) = terminal_manager.focused else {
        tracing::warn!("resize request but no focused terminal");
        compositor::ipc::send_ack(ack_stream);
        return;
    };

    let cell_height = terminal_manager.cell_height;
    let new_rows = match resize_mode {
        compositor::ipc::ResizeMode::Full => {
            tracing::info!(id = focused_id.0, max_rows = terminal_manager.max_rows, "resize to full");
            terminal_manager.max_rows
        }
        compositor::ipc::ResizeMode::Content => {
            // Process pending PTY output first
            if let Some(term) = terminal_manager.get_mut(focused_id) {
                term.process();
            }

            // Calculate content rows from cursor position
            if let Some(term) = terminal_manager.get(focused_id) {
                let cursor_line = term.terminal.cursor_line();
                let content_rows = (cursor_line + CURSOR_TO_CONTENT_OFFSET).max(MIN_TERMINAL_ROWS);
                tracing::info!(id = focused_id.0, cursor_line, content_rows, "resize to content");
                content_rows
            } else {
                MIN_TERMINAL_ROWS
            }
        }
    };

    if let Some(term) = terminal_manager.get_mut(focused_id) {
        tracing::info!(id = focused_id.0, ?resize_mode, new_rows, "resizing terminal via IPC");
        term.resize(new_rows, cell_height);

        // Update cached height
        for node in compositor.layout_nodes.iter_mut() {
            if let ColumnCell::Terminal(tid) = node.cell {
                if tid == focused_id {
                    node.height = term.height as i32;
                    break;
                }
            }
        }

        // Scroll to keep terminal visible
        if let Some(idx) = compositor.focused_index {
            compositor.scroll_to_show_cell_bottom(idx);
        }
    }

    compositor::ipc::send_ack(ack_stream);
    tracing::info!("resize ACK sent");
}


/// Promote output terminals that have content to standalone cells.
///
/// Checks each external window's output terminal. If it has output and isn't
/// already a cell, inserts it as a cell right after the window.
fn promote_output_terminals(
    compositor: &mut ColumnCompositor,
    terminal_manager: &TerminalManager,
) {
    // Collect (window_cell_idx, term_id) pairs for terminals to promote
    let mut to_promote: Vec<(usize, TerminalId)> = Vec::new();

    for (cell_idx, node) in compositor.layout_nodes.iter().enumerate() {
        if let ColumnCell::External(entry) = &node.cell {
            if let Some(term_id) = entry.output_terminal {
                // Check if terminal already in cells
                let already_cell = compositor.layout_nodes.iter().any(|n| {
                    matches!(n.cell, ColumnCell::Terminal(id) if id == term_id)
                });

                if !already_cell {
                    if let Some(term) = terminal_manager.get(term_id) {
                        // Promote if terminal has any content
                        if term.content_rows() > 0 {
                            to_promote.push((cell_idx, term_id));
                        }
                    }
                }
            }
        }
    }

    // Promote terminals (one at a time to avoid index issues)
    // Insert in reverse order so earlier insertions don't affect later indices
    for (window_idx, term_id) in to_promote.into_iter().rev() {
        // Insert terminal cell right after this window
        // (cell_idx + 1 puts it below the window in the column)
        let insert_idx = window_idx + 1;
        
        let height = terminal_manager.get(term_id)
            .map(|t| t.height as i32)
            .unwrap_or(0);
            
        compositor.layout_nodes.insert(insert_idx, compositor::state::LayoutNode {
            cell: ColumnCell::Terminal(term_id),
            height,
        });

        tracing::info!(
            terminal_id = term_id.0,
            window_idx,
            insert_idx,
            "promoted output terminal to standalone cell"
        );
    }
}

/// Handle cleanup of output terminals from closed windows.
///
/// Terminals that have had output stay visible. Terminals that never had output are removed.
fn handle_output_terminal_cleanup(
    compositor: &mut ColumnCompositor,
    terminal_manager: &mut TerminalManager,
) {
    let cleanup_ids = std::mem::take(&mut compositor.pending_output_terminal_cleanup);

    for term_id in cleanup_ids {
        let has_had_output = terminal_manager.get(term_id)
            .map(|t| t.has_had_output)
            .unwrap_or(false);

        if has_had_output {
            // Terminal has had output - keep it visible
            tracing::info!(
                terminal_id = term_id.0,
                "output terminal has had output, keeping visible after window close"
            );
        } else {
            // Never had output - remove from layout and TerminalManager
            compositor.layout_nodes.retain(|n| {
                !matches!(n.cell, ColumnCell::Terminal(id) if id == term_id)
            });
            terminal_manager.remove(term_id);
            tracing::info!(
                terminal_id = term_id.0,
                "removed output terminal (never had output) after window close"
            );
        }
    }
}

/// Cleanup dead terminals, sync focus, and handle shutdown.
///
/// Returns `true` if the compositor should shut down (all cells removed).
fn cleanup_and_sync_focus(
    compositor: &mut ColumnCompositor,
    terminal_manager: &mut TerminalManager,
) -> bool {
    let (dead, focus_changed_to) = terminal_manager.cleanup();

    // Remove dead terminals from compositor
    for dead_id in &dead {
        compositor.remove_terminal(*dead_id);
        tracing::info!(id = dead_id.0, "removed dead terminal from cells");
    }

    // Sync compositor focus if a command terminal exited
    if let Some(new_focus_id) = focus_changed_to {
        if let Some(idx) = find_terminal_cell_index(compositor, new_focus_id) {
            compositor.focused_index = Some(idx);
            tracing::info!(id = new_focus_id.0, index = idx, "synced compositor focus to parent terminal");

            // Update cached height for unhidden terminal (was 0 when hidden)
            if let Some(term) = terminal_manager.get(new_focus_id) {
                if let Some(node) = compositor.layout_nodes.get_mut(idx) {
                    node.height = term.height as i32;
                }
            }

            // Scroll to show the unhidden parent terminal
            if let Some(new_scroll) = compositor.scroll_to_show_cell_bottom(idx) {
                tracing::info!(
                    id = new_focus_id.0,
                    new_scroll,
                    "scrolled to show unhidden parent terminal"
                );
            }
        }
    }

    // Check if all cells are gone
    if !dead.is_empty() && compositor.layout_nodes.is_empty() {
        tracing::info!("all cells removed, shutting down");
        return true;
    }

    false
}
