//! Column Compositor - A content-aware terminal compositor
//!
//! This compositor arranges terminal windows in a scrollable vertical column,
//! with windows dynamically sizing based on their content.

use std::os::unix::net::UnixListener;
use std::time::Duration;

use smithay::backend::winit::{self, WinitEvent};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{Color32F, Frame, Renderer, Texture};
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{AsRenderElements, Element, RenderElement};
use smithay::desktop::utils::send_frames_surface_tree;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::{EventLoop, generic::Generic, Interest, Mode as CalloopMode};
use smithay::reexports::wayland_server::Display;
use smithay::utils::{Physical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::socket::ListeningSocketSource;

use compositor::config::Config;
use compositor::state::{ClientState, ColumnCell, ColumnCompositor};
use compositor::terminal_manager::{TerminalId, TerminalManager};

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
                        if let Some((request, stream)) = compositor::ipc::read_ipc_request(stream) {
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
                        } else {
                            tracing::warn!("failed to parse IPC request");
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
        // Handle new external window FIRST - must sync cached_cell_heights before height calculation
        // When a cell is inserted, the indices shift. If we don't insert into cached_cell_heights
        // here, the height calculation below will use wrong indices (e.g., terminal's height
        // would be used for the new window).
        if let Some(window_idx) = compositor.new_external_window_index.take() {
            tracing::info!(
                window_idx,
                cached_heights_before = ?compositor.cached_cell_heights,
                cells_count = compositor.cells.len(),
                focused_index = ?compositor.focused_index,
                "handling new external window"
            );

            // INSERT into cached heights (not just set) since cells were shifted
            let window_height = compositor.cells.get(window_idx)
                .and_then(|c| match c {
                    ColumnCell::External(entry) => Some(entry.state.current_height() as i32),
                    _ => None,
                })
                .unwrap_or(200);

            if window_idx <= compositor.cached_cell_heights.len() {
                compositor.cached_cell_heights.insert(window_idx, window_height);
            } else {
                // Shouldn't happen, but handle gracefully
                while compositor.cached_cell_heights.len() < window_idx {
                    compositor.cached_cell_heights.push(200);
                }
                compositor.cached_cell_heights.push(window_height);
            }

            tracing::info!(
                cached_heights_after = ?compositor.cached_cell_heights,
                "after inserting window height"
            );

            // Scroll to show the focused cell (the terminal that launched the window)
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

        // Handle external window resize - update cached heights and scroll if needed
        if let Some((resized_idx, new_height)) = compositor.external_window_resized.take() {
            tracing::info!(
                resized_idx,
                new_height,
                cached_heights_before = ?compositor.cached_cell_heights,
                "handling external window resize"
            );

            // Update cached height for the resized window
            if resized_idx < compositor.cached_cell_heights.len() {
                compositor.cached_cell_heights[resized_idx] = new_height;
            }

            // If the focused cell is at or after the resized window, recalculate scroll
            // to keep the focused cell's bottom visible
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

        // Update cell heights BEFORE processing input events
        // so click detection uses the correct positions
        //
        // IMPORTANT: We use cached heights from the PREVIOUS frame for existing cells.
        // These are the actual rendered heights (from element geometry), not bbox().
        // Using bbox() here caused a mismatch with rendering, making click detection
        // increasingly wrong further down the stack.
        //
        // We only need to calculate heights for NEW cells (where cached height is missing).
        let cell_heights: Vec<i32> = compositor.cells.iter().enumerate().map(|(i, cell)| {
            // Use cached height if available (it's the actual rendered height from last frame)
            if let Some(&cached) = compositor.cached_cell_heights.get(i) {
                if cached > 0 {
                    return cached;
                }
            }

            // Fallback for new cells: use terminal.height (not texture which may not exist yet)
            match cell {
                ColumnCell::Terminal(id) => {
                    // For new terminals, use terminal.height directly
                    // This is critical for TUI terminals which need full height immediately
                    terminal_manager.get(*id)
                        .map(|t| if t.hidden { 0 } else { t.height as i32 })
                        .unwrap_or(200)
                }
                ColumnCell::External(entry) => {
                    // For new external windows, use state height as initial guess
                    entry.state.current_height() as i32
                }
            }
        }).collect();

        // Cache heights for consistent positioning between input and render
        compositor.update_cached_cell_heights(cell_heights);

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
                compositor.recalculate_layout();
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

        // Handle terminal spawn requests
        if compositor.spawn_terminal_requested {
            compositor.spawn_terminal_requested = false;

            match terminal_manager.spawn() {
                Ok(id) => {
                    compositor.add_terminal(id);

                    // Update cached_cell_heights to include the new terminal
                    // Use cached heights for existing cells, default for new terminal
                    let new_heights: Vec<i32> = compositor.cells.iter().enumerate().map(|(i, cell)| {
                        // Use cached height if available
                        if let Some(&cached) = compositor.cached_cell_heights.get(i) {
                            if cached > 0 {
                                return cached;
                            }
                        }

                        // Fallback for new cells: use terminal.height directly
                        match cell {
                            ColumnCell::Terminal(tid) => {
                                terminal_manager.get(*tid)
                                    .map(|t| if t.hidden { 0 } else { t.height as i32 })
                                    .unwrap_or(200)
                            }
                            ColumnCell::External(entry) => {
                                entry.state.current_height() as i32
                            }
                        }
                    }).collect();
                    compositor.update_cached_cell_heights(new_heights);

                    // Scroll to show the newly focused cell (the new terminal)
                    if let Some(focused_idx) = compositor.focused_index {
                        if let Some(new_scroll) = compositor.scroll_to_show_cell_bottom(focused_idx) {
                            tracing::info!(
                                id = id.0,
                                cell_count = compositor.cells.len(),
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

        // Handle command spawn requests from IPC (column-term)
        while let Some(request) = compositor.pending_spawn_requests.pop() {
            // Decide what command to run
            let command = if request.command.is_empty() {
                // Empty command = spawn interactive shell
                std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
            } else if request.is_tui {
                // TUI apps take over the screen - no echo prefix needed
                request.command.clone()
            } else {
                // Echo the command first, then run it
                // Simple echo before the command
                let escaped = request.command.replace("'", "'\\''");
                format!("echo '> {}'; {}", escaped, request.command)
            };

            // Modify environment to disable pagers and fancy terminal features
            // This prevents commands like `git log` from clearing the echo line
            let mut env = request.env.clone();
            env.insert("GIT_PAGER".to_string(), "cat".to_string());
            env.insert("PAGER".to_string(), "cat".to_string());
            // Disable alternate screen buffer for less (if used)
            env.insert("LESS".to_string(), "-FRX".to_string());
            // Override WAYLAND_DISPLAY so GUI apps connect to our compositor
            // (column-term might have captured the host's display)
            if let Ok(wayland_display) = std::env::var("WAYLAND_DISPLAY") {
                tracing::info!(
                    original = ?env.get("WAYLAND_DISPLAY"),
                    override_to = %wayland_display,
                    "overriding WAYLAND_DISPLAY in spawn env"
                );
                env.insert("WAYLAND_DISPLAY".to_string(), wayland_display);
            }

            // Hide the focused terminal while command runs
            let parent = terminal_manager.focused;

            // Reject spawns from terminals in alternate screen mode (TUI apps).
            // When a shell runs a TUI app (vim, mc, fzf), it enters alternate screen mode.
            // If the TUI app has an internal subshell (like mc), commands typed there would
            // normally spawn new terminals. We reject these to keep the TUI app's subshell
            // communication working correctly.
            // Detection: ALT_SCREEN mode is the definitive signal that a TUI app is running.
            if let Some(parent_id) = parent {
                if let Some(parent_term) = terminal_manager.get(parent_id) {
                    if parent_term.terminal.is_alternate_screen() {
                        tracing::info!(command = %command, "rejecting spawn from alternate screen terminal");
                        continue;
                    }
                }
            }

            tracing::info!(
                command = %command,
                is_tui = request.is_tui,
                ?parent,
                max_rows = terminal_manager.max_rows,
                cell_height = terminal_manager.cell_height,
                "about to spawn command terminal"
            );

            match terminal_manager.spawn_command(&command, &request.cwd, &env, parent, request.is_tui) {
                Ok(id) => {
                    // Log the created terminal's state
                    if let Some(term) = terminal_manager.get(id) {
                        let (cols, pty_rows) = term.terminal.dimensions();
                        tracing::info!(
                            id = id.0,
                            cols,
                            pty_rows,
                            visual_height = term.height,
                            hidden = term.hidden,
                            "terminal created"
                        );
                    }

                    compositor.add_terminal(id);

                    // Focus the new command terminal (it was inserted above the old focus)
                    // Find its cell index and set focus
                    for (i, cell) in compositor.cells.iter().enumerate() {
                        if let ColumnCell::Terminal(tid) = cell {
                            if *tid == id {
                                compositor.focused_index = Some(i);
                                tracing::info!(id = id.0, index = i, "focused new command terminal");
                                break;
                            }
                        }
                    }

                    // Update cached_cell_heights to include the new terminal
                    // Hidden terminals get 0 height
                    let new_heights: Vec<i32> = compositor.cells.iter().enumerate().map(|(i, cell)| {
                        match cell {
                            ColumnCell::Terminal(tid) => {
                                // Check if hidden first
                                if terminal_manager.get(*tid).map(|t| t.hidden).unwrap_or(false) {
                                    return 0;
                                }
                                // Use cached if available
                                if let Some(&cached) = compositor.cached_cell_heights.get(i) {
                                    if cached > 0 {
                                        return cached;
                                    }
                                }
                                // For new terminals, use terminal.height (not texture which doesn't exist yet)
                                // This is critical for TUI terminals which need full height from the start
                                terminal_manager.get(*tid)
                                    .map(|t| t.height as i32)
                                    .unwrap_or(200)
                            }
                            ColumnCell::External(entry) => {
                                if let Some(&cached) = compositor.cached_cell_heights.get(i) {
                                    if cached > 0 {
                                        return cached;
                                    }
                                }
                                entry.state.current_height() as i32
                            }
                        }
                    }).collect();
                    compositor.update_cached_cell_heights(new_heights);

                    // Scroll to show the newly focused terminal (like regular spawn)
                    if let Some(focused_idx) = compositor.focused_index {
                        if let Some(new_scroll) = compositor.scroll_to_show_cell_bottom(focused_idx) {
                            tracing::info!(
                                id = id.0,
                                cell_count = compositor.cells.len(),
                                focused_idx,
                                new_scroll,
                                "spawned command terminal, scrolling to show"
                            );
                        }
                    }

                    tracing::info!(id = id.0, command = %command, ?parent, "spawned command terminal from IPC");
                }
                Err(e) => {
                    tracing::error!("failed to spawn command terminal: {}", e);
                }
            }
        }

        // Handle resize requests from IPC (column-term --resize)
        if let Some((resize_mode, ack_stream)) = compositor.pending_resize_request.take() {
            if let Some(focused_id) = terminal_manager.focused {
                let cell_height = terminal_manager.cell_height;
                let new_rows = match resize_mode {
                    compositor::ipc::ResizeMode::Full => {
                        // Resize to full viewport height for TUI apps
                        tracing::info!(id = focused_id.0, max_rows = terminal_manager.max_rows, "resize to full");
                        terminal_manager.max_rows
                    }
                    compositor::ipc::ResizeMode::Content => {
                        // Process any pending PTY output BEFORE calculating content rows
                        // This ensures TUI app's printed output is reflected in cursor position
                        if let Some(term) = terminal_manager.get_mut(focused_id) {
                            term.process();
                        }

                        // Calculate content rows from cursor position
                        // cursor_line is 0-indexed, add 2 for: +1 zero-index, +1 prompt line
                        if let Some(term) = terminal_manager.get(focused_id) {
                            let cursor_line = term.terminal.cursor_line();
                            let content_rows = (cursor_line + 2).max(3);
                            tracing::info!(id = focused_id.0, cursor_line, content_rows, "resize to content");
                            content_rows
                        } else {
                            3  // fallback minimum
                        }
                    }
                };

                // Log BEFORE state
                let before_height = terminal_manager.get(focused_id).map(|t| t.height);
                let before_dirty = terminal_manager.get(focused_id).map(|t| t.is_dirty());
                let before_pty = terminal_manager.get(focused_id).map(|t| t.terminal.dimensions().1);
                let before_grid = terminal_manager.get(focused_id).map(|t| t.terminal.grid_rows());

                if let Some(term) = terminal_manager.get_mut(focused_id) {
                    tracing::info!(
                        id = focused_id.0,
                        ?resize_mode,
                        new_rows,
                        ?before_height,
                        ?before_dirty,
                        ?before_pty,
                        ?before_grid,
                        "resizing focused terminal via IPC"
                    );
                    term.resize(new_rows, cell_height);

                    // Log AFTER state
                    let after_height = term.height;
                    let after_dirty = term.is_dirty();
                    let after_pty = term.terminal.dimensions().1;
                    let after_grid = term.terminal.grid_rows();
                    let has_texture = term.get_texture().is_some();
                    tracing::info!(
                        after_height,
                        after_dirty,
                        after_pty,
                        after_grid,
                        has_texture,
                        "AFTER resize"
                    );

                    // Update cached height for the resized terminal
                    for (i, cell) in compositor.cells.iter().enumerate() {
                        if let ColumnCell::Terminal(tid) = cell {
                            if *tid == focused_id && i < compositor.cached_cell_heights.len() {
                                compositor.cached_cell_heights[i] = term.height as i32;
                                tracing::info!(idx = i, cached_height = compositor.cached_cell_heights[i], "updated cached height");
                                break;
                            }
                        }
                    }

                    // Scroll to keep the terminal visible
                    if let Some(idx) = compositor.focused_index {
                        compositor.scroll_to_show_cell_bottom(idx);
                    }
                }

                // Send ACK AFTER resize is complete - this prevents race condition
                // column-term will wait for this before returning
                compositor::ipc::send_ack(ack_stream);
                tracing::info!("resize ACK sent, PTY and grid should now report new size");
            } else {
                tracing::warn!("resize request but no focused terminal");
                // Still send ACK so column-term doesn't hang
                compositor::ipc::send_ack(ack_stream);
            }
        }

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
            // Total content height from all cells
            let total_height: i32 = compositor.cached_cell_heights.iter().sum();
            let visible_height = compositor.output_size.h;
            let max_scroll = (total_height - visible_height).max(0) as f64;
            compositor.scroll_offset = (compositor.scroll_offset + compositor.scroll_requested)
                .clamp(0.0, max_scroll);
            compositor.scroll_requested = 0.0;
            tracing::debug!(
                total_height,
                visible_height,
                max_scroll,
                new_offset = compositor.scroll_offset,
                "scroll processed"
            );
        }

        // Process terminal PTY output and handle sizing actions
        let sizing_actions = terminal_manager.process_all();
        if !sizing_actions.is_empty() {
            tracing::info!(count = sizing_actions.len(), "received sizing actions");
        }
        for (id, action) in sizing_actions {
            match action {
                terminal::sizing::SizingAction::RequestGrowth { target_rows } => {
                    tracing::info!(id = id.0, target_rows, "processing growth request");
                    terminal_manager.grow_terminal(id, target_rows);

                    // If focused terminal grew, scroll to keep its bottom visible
                    // Use cached_cell_heights for consistency with render loop
                    if terminal_manager.focused == Some(id) {
                        // Find cell index for this terminal
                        let mut cell_idx = None;
                        for (i, cell) in compositor.cells.iter().enumerate() {
                            if let ColumnCell::Terminal(tid) = cell {
                                if *tid == id {
                                    cell_idx = Some(i);
                                    break;
                                }
                            }
                        }

                        if let Some(idx) = cell_idx {
                            // Update cached height with new size
                            if let Some(term) = terminal_manager.get(id) {
                                if idx < compositor.cached_cell_heights.len() {
                                    compositor.cached_cell_heights[idx] = term.height as i32;
                                }
                            }

                            // Scroll to keep focused terminal bottom visible during growth
                            if let Some(new_scroll) = compositor.scroll_to_show_cell_bottom(idx) {
                                tracing::debug!(
                                    id = id.0,
                                    new_scroll,
                                    "scrolled to keep focused terminal bottom visible"
                                );
                            }
                        }
                    }
                }
                terminal::sizing::SizingAction::ApplyResize { .. } => {
                    // Handled internally by grow_terminal
                }
                terminal::sizing::SizingAction::RestoreScrollback { .. } => {
                    // TODO: handle scrollback restoration if needed
                }
                terminal::sizing::SizingAction::None => {}
            }
        }

        // Auto-resize terminals that entered alternate screen mode
        // This handles TUI apps not in tui_apps config list
        let max_height = terminal_manager.max_rows as u32 * terminal_manager.cell_height;
        let visible_ids = terminal_manager.visible_ids();
        let mut ids_to_resize = Vec::new();
        for id in visible_ids {
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
                    max_rows,
                    "auto-resized terminal to full for alternate screen"
                );

                // Update cached height if in compositor cells
                for (i, cell) in compositor.cells.iter().enumerate() {
                    if let ColumnCell::Terminal(tid) = cell {
                        if *tid == id {
                            if i < compositor.cached_cell_heights.len() {
                                compositor.cached_cell_heights[i] = new_height as i32;
                            }
                            break;
                        }
                    }
                }
            }
        }

        // Cleanup dead terminals and handle focus changes
        let (dead, focus_changed_to) = terminal_manager.cleanup();
        for dead_id in &dead {
            compositor.remove_terminal(*dead_id);
            tracing::info!(id = dead_id.0, "removed dead terminal from cells");
        }

        // Sync compositor focus if a command terminal exited
        if let Some(new_focus_id) = focus_changed_to {
            // Find the cell index for this terminal
            let mut found_index = None;
            for (i, cell) in compositor.cells.iter().enumerate() {
                if let ColumnCell::Terminal(tid) = cell {
                    if *tid == new_focus_id {
                        found_index = Some(i);
                        compositor.focused_index = Some(i);
                        tracing::info!(id = new_focus_id.0, index = i, "synced compositor focus to parent terminal");
                        break;
                    }
                }
            }

            // Scroll to make the unhidden parent terminal fully visible
            // Use cached_cell_heights for consistency with render loop
            if let Some(idx) = found_index {
                // But first, update the cached height for the unhidden terminal
                // since it was 0 when hidden
                if let Some(term) = terminal_manager.get(new_focus_id) {
                    if idx < compositor.cached_cell_heights.len() {
                        compositor.cached_cell_heights[idx] = term.height as i32;
                    }
                }

                // Scroll so the bottom of the terminal is at the bottom of the screen
                if let Some(new_scroll) = compositor.scroll_to_show_cell_bottom(idx) {
                    tracing::info!(
                        id = new_focus_id.0,
                        new_scroll,
                        "scrolled to show unhidden parent terminal"
                    );
                }
            }
        }

        if !dead.is_empty() {
            // If all cells are gone, quit (only terminals can trigger this)
            if compositor.cells.is_empty() {
                tracing::info!("all cells removed, shutting down");
                break;
            }
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

            // Pre-render all terminal textures
            let all_ids = terminal_manager.ids();
            tracing::debug!(count = all_ids.len(), ids = ?all_ids.iter().map(|id| id.0).collect::<Vec<_>>(), "pre-rendering terminals");
            for id in all_ids {
                if let Some(terminal) = terminal_manager.get_mut(id) {
                    tracing::debug!(id = id.0, dirty = terminal.is_dirty(), has_texture = terminal.get_texture().is_some(), "pre-render check");
                    terminal.render(renderer);
                }
            }

            // Pre-compute render data for all cells
            let scale = Scale::from(1.0);

            // Build render data for each cell
            #[allow(dead_code)]
            enum CellRenderData {
                Terminal {
                    id: TerminalId,
                    y: i32,
                    height: i32,
                },
                External {
                    y: i32,
                    height: i32,
                    elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
                },
                // Placeholder for terminals without textures
                Empty { height: i32 },
            }

            // First pass: collect heights for ALL cells (not using cached heights which may be stale)
            let mut actual_heights: Vec<i32> = Vec::new();
            let mut external_elements: Vec<Vec<WaylandSurfaceRenderElement<GlesRenderer>>> = Vec::new();

            for cell in compositor.cells.iter() {
                // Get cached height if available, otherwise use default
                let cached_height = compositor.cached_cell_heights.get(actual_heights.len())
                    .copied()
                    .unwrap_or(200);

                match cell {
                    ColumnCell::Terminal(id) => {
                        // Use texture size if available, otherwise terminal.height
                        // (texture may not exist on first frame for new terminals)
                        let height = terminal_manager.get(*id)
                            .map(|t| {
                                if t.hidden {
                                    0
                                } else if let Some(tex) = t.get_texture() {
                                    tex.size().h
                                } else {
                                    // No texture yet - use terminal.height directly
                                    // This is critical for TUI terminals on first frame
                                    t.height as i32
                                }
                            })
                            .unwrap_or(cached_height);
                        actual_heights.push(height);
                        external_elements.push(Vec::new());
                    }
                    ColumnCell::External(entry) => {
                        let window = &entry.window;
                        let location: Point<i32, Physical> = Point::from((0, 0));
                        let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                            window.render_elements(renderer, location, scale, 1.0);

                        let actual_height = if elements.is_empty() {
                            cached_height
                        } else {
                            elements.iter()
                                .map(|e| {
                                    let geo = e.geometry(scale);
                                    geo.loc.y + geo.size.h
                                })
                                .max()
                                .unwrap_or(cached_height)
                        };

                        actual_heights.push(actual_height);
                        external_elements.push(elements);
                    }
                }
            }

            // Second pass: compute Y positions
            // OpenGL has y=0 at BOTTOM, but we want index 0 at TOP
            // So we need to flip: render_y = screen_height - content_y - cell_height

            let mut render_data: Vec<CellRenderData> = Vec::new();

            // Calculate content Y positions (index 0 at content_y=0)
            let mut content_y: i32 = -(compositor.scroll_offset as i32);

            for (cell_idx, cell) in compositor.cells.iter().enumerate() {
                let height = actual_heights[cell_idx];

                // Flip Y for OpenGL: convert from top-down to bottom-up
                let render_y = physical_size.h - content_y - height;

                match cell {
                    ColumnCell::Terminal(id) => {
                        // Debug: log terminal render state
                        if let Some(term) = terminal_manager.get(*id) {
                            let has_texture = term.get_texture().is_some();
                            tracing::debug!(
                                id = id.0,
                                hidden = term.hidden,
                                height,
                                render_y,
                                has_texture,
                                content_rows = term.content_rows(),
                                "terminal render state"
                            );
                        }
                        render_data.push(CellRenderData::Terminal {
                            id: *id,
                            y: render_y,
                            height,
                        });
                    }
                    ColumnCell::External(_) => {
                        let elements = std::mem::take(&mut external_elements[cell_idx]);
                        render_data.push(CellRenderData::External {
                            y: render_y,
                            height,
                            elements,
                        });
                    }
                }

                content_y += height;
            }

            // Debug: dump complete frame state when there are external windows
            let has_external = compositor.cells.iter().any(|c| matches!(c, ColumnCell::External(_)));
            if has_external {
                let cell_info: Vec<String> = compositor.cells.iter().enumerate().map(|(i, cell)| {
                    match cell {
                        ColumnCell::Terminal(id) => {
                            let hidden = terminal_manager.get(*id).map(|t| t.hidden).unwrap_or(false);
                            format!("[{}]Term({})h={}{}", i, id.0,
                                compositor.cached_cell_heights.get(i).unwrap_or(&0),
                                if hidden { " HIDDEN" } else { "" })
                        }
                        ColumnCell::External(e) => {
                            format!("[{}]Ext h={}", i, e.state.current_height())
                        }
                    }
                }).collect();

                let render_info: Vec<String> = render_data.iter().enumerate().map(|(i, data)| {
                    match data {
                        CellRenderData::Terminal { id, y, height } => format!("[{}]T{}@y={},h={}", i, id.0, y, height),
                        CellRenderData::External { y, height, .. } => format!("[{}]E@y={},h={}", i, y, height),
                        CellRenderData::Empty { height } => format!("[{}]empty h={}", i, height),
                    }
                }).collect();

                tracing::info!(
                    scroll = compositor.scroll_offset,
                    focused = ?compositor.focused_index,
                    screen_h = physical_size.h,
                    cells = %cell_info.join(" "),
                    render = %render_info.join(" "),
                    "FRAME STATE"
                );
            }

            // Update cached heights with actual heights for next frame's click detection
            // BUT FIRST: check if any height changed significantly - if so, we need to re-scroll
            let heights_changed = compositor.cached_cell_heights.iter()
                .zip(actual_heights.iter())
                .enumerate()
                .any(|(i, (&cached, &actual))| {
                    // Only care about cells BEFORE or AT focused index
                    // (changes after focused cell don't affect its visibility)
                    if let Some(focused) = compositor.focused_index {
                        if i <= focused && actual != cached && (actual - cached).abs() > 10 {
                            return true;
                        }
                    }
                    false
                });

            compositor.update_cached_cell_heights(actual_heights);

            // If heights changed, recalculate scroll to keep focused cell visible
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

            let mut frame = renderer.render(&mut framebuffer, physical_size, Transform::Normal)
                .map_err(|e| anyhow::anyhow!("render error: {e:?}"))?;

            // Clear with background color
            frame.clear(bg_color, &[damage])
                .map_err(|e| anyhow::anyhow!("clear error: {e:?}"))?;

            // Render all cells in order
            for (cell_idx, data) in render_data.into_iter().enumerate() {
                let is_focused = compositor.focused_index == Some(cell_idx);

                match data {
                    CellRenderData::Terminal { id, y, height } => {
                        if let Some(terminal) = terminal_manager.get(id) {
                            // Skip hidden terminals entirely
                            if terminal.hidden {
                                continue;
                            }
                            if let Some(texture) = terminal.get_texture() {
                                // Only render if visible
                                if y + height > 0 && y < physical_size.h {
                                    frame.render_texture_at(
                                        texture,
                                        Point::from((0, y)),
                                        1,     // texture_scale
                                        1.0,   // output_scale
                                        Transform::Flipped180,
                                        &[damage],
                                        &[],
                                        1.0,
                                    ).ok();

                                    // Draw focus indicator
                                    if is_focused && y >= 0 {
                                        let border_height = 2;
                                        let focus_damage = Rectangle::new(
                                            (0, y).into(),
                                            (physical_size.w, border_height).into(),
                                        );
                                        frame.clear(Color32F::new(0.0, 0.8, 0.0, 1.0), &[focus_damage]).ok();
                                    }
                                }
                            }
                        }
                    }
                    CellRenderData::External { y, height: _, elements } => {
                        // Draw focus indicator for external windows
                        if is_focused && y >= 0 && y < physical_size.h {
                            let border_height = 2;
                            let focus_damage = Rectangle::new(
                                (0, y).into(),
                                (physical_size.w, border_height).into(),
                            );
                            frame.clear(Color32F::new(0.0, 0.8, 0.0, 1.0), &[focus_damage]).ok();
                        }

                        // Render external window elements
                        for element in elements {
                            let geo = element.geometry(scale);
                            let src = element.src();

                            let dest = Rectangle::new(
                                Point::from((geo.loc.x, geo.loc.y + y)),
                                geo.size,
                            );

                            let flipped_src = Rectangle::new(
                                Point::from((src.loc.x, src.loc.y + src.size.h)),
                                Size::from((src.size.w, -src.size.h)),
                            );

                            element.draw(&mut frame, flipped_src, dest, &[damage], &[]).ok();
                        }
                    }
                    CellRenderData::Empty { .. } => {
                        // Nothing to render
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

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(true).with_line_number(true))
        .with(filter)
        .init();
}
