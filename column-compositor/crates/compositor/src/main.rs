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
                        if let Some(request) = compositor::ipc::read_spawn_request(stream) {
                            tracing::info!(command = %request.command, "IPC spawn request queued");
                            state.pending_spawn_requests.push(request);
                        } else {
                            tracing::warn!("failed to parse IPC spawn request");
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

            // Fallback for new cells: use texture size or default
            match cell {
                ColumnCell::Terminal(id) => {
                    terminal_manager.get(*id)
                        .and_then(|t| t.get_texture())
                        .map(|tex| tex.size().h)
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

                        // Fallback for new cells
                        match cell {
                            ColumnCell::Terminal(tid) => {
                                terminal_manager.get(*tid)
                                    .and_then(|t| t.get_texture())
                                    .map(|tex| tex.size().h)
                                    .unwrap_or(200) // Default height for new terminals
                            }
                            ColumnCell::External(entry) => {
                                entry.state.current_height() as i32
                            }
                        }
                    }).collect();
                    compositor.update_cached_cell_heights(new_heights);

                    // Scroll to show the newly focused cell (the new terminal)
                    if let Some(focused_idx) = compositor.focused_index {
                        let y: i32 = compositor.cached_cell_heights.iter().take(focused_idx).sum();
                        let total_height: i32 = compositor.cached_cell_heights.iter().sum();
                        let visible_height = compositor.output_size.h;
                        let max_scroll = (total_height - visible_height).max(0) as f64;
                        compositor.scroll_offset = (y as f64).clamp(0.0, max_scroll);

                        tracing::info!(
                            id = id.0,
                            cell_count = compositor.cells.len(),
                            focused_idx,
                            scroll = compositor.scroll_offset,
                            "spawned terminal, scrolling to show"
                        );
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
            } else {
                // Echo the command first, then run it
                // Escape single quotes in command for the echo
                let escaped = request.command.replace("'", "'\\''");
                format!("echo '> {}'; {}", escaped, request.command)
            };

            match terminal_manager.spawn_command(&command, &request.cwd, &request.env) {
                Ok(id) => {
                    compositor.add_terminal(id);

                    // Update cached_cell_heights to include the new terminal
                    let new_heights: Vec<i32> = compositor.cells.iter().enumerate().map(|(i, cell)| {
                        if let Some(&cached) = compositor.cached_cell_heights.get(i) {
                            if cached > 0 {
                                return cached;
                            }
                        }
                        match cell {
                            ColumnCell::Terminal(tid) => {
                                terminal_manager.get(*tid)
                                    .and_then(|t| t.get_texture())
                                    .map(|tex| tex.size().h)
                                    .unwrap_or(200)
                            }
                            ColumnCell::External(entry) => {
                                entry.state.current_height() as i32
                            }
                        }
                    }).collect();
                    compositor.update_cached_cell_heights(new_heights);

                    tracing::info!(id = id.0, command = %command, "spawned command terminal from IPC");
                }
                Err(e) => {
                    tracing::error!("failed to spawn command terminal: {}", e);
                }
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
                // Calculate y position of focused cell
                let y: i32 = compositor.cached_cell_heights.iter().take(focused_idx).sum();
                let visible_height = compositor.output_size.h;
                let total_height: i32 = compositor.cached_cell_heights.iter().sum();
                let max_scroll = (total_height - visible_height).max(0) as f64;
                // Scroll so focused cell is at top
                compositor.scroll_offset = (y as f64).clamp(0.0, max_scroll);
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

        // Cleanup dead terminals
        let dead = terminal_manager.cleanup();
        for dead_id in &dead {
            compositor.remove_terminal(*dead_id);
            tracing::info!(id = dead_id.0, "removed dead terminal from cells");
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
            for id in terminal_manager.ids() {
                if let Some(terminal) = terminal_manager.get_mut(id) {
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
                        let height = terminal_manager.get(*id)
                            .and_then(|t| t.get_texture())
                            .map(|tex| tex.size().h)
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

            // Update cached heights with actual heights for next frame's click detection
            compositor.update_cached_cell_heights(actual_heights);

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
