//! Column Compositor - A content-aware terminal compositor
//!
//! This compositor arranges terminal windows in a scrollable vertical column,
//! with windows dynamically sizing based on their content.

use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::time::Duration;
use std::collections::HashSet;

use smithay::backend::allocator::dmabuf::DmabufAllocator;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::input::InputEvent;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::element::surface::render_elements_from_surface_tree;
use smithay::backend::renderer::element::{Element, Kind, RenderElement};
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::{Color32F, Frame, Renderer, Bind};
use smithay::backend::x11::{X11Backend, X11Event, X11Input, WindowBuilder};
use smithay::desktop::PopupKind;
use smithay::desktop::utils::send_frames_surface_tree;
use smithay::desktop::PopupManager;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::utils::{DeviceFd, Point};
use smithay::reexports::calloop::{EventLoop, generic::Generic, Interest, Mode as CalloopMode};
use smithay::reexports::wayland_server::{Display, Resource};
use smithay::utils::{Physical, Rectangle, Scale, Size, Transform};
use smithay::wayland::socket::ListeningSocketSource;

use crate::config::Config;
use crate::cursor::CursorManager;
use crate::render::{
    CellRenderData, prerender_terminals, prerender_title_bars,
    collect_window_data, build_render_data, log_frame_state,
    heights_changed_significantly, render_terminal, render_external,
    TitleBarCache,
};
use crate::state::{ClientState, StackWindow, TermStack};
use crate::xwayland_lifecycle;
use crate::terminal_manager::{TerminalId, TerminalManager};
use crate::title_bar::{TitleBarRenderer, TITLE_BAR_HEIGHT};

/// Default terminal height in pixels (fallback when terminal doesn't exist)
const DEFAULT_TERMINAL_HEIGHT: i32 = 200;

/// Popup render data: (x, y, geo_offset_x, geo_offset_y, elements)
type PopupRenderData = Vec<(i32, i32, i32, i32, Vec<WaylandSurfaceRenderElement<GlesRenderer>>)>;

pub fn run_compositor() -> anyhow::Result<()> {
    // Initialize logging (setup_logging() should be called by the binary before this)

    tracing::info!("starting termstack");

    // Load configuration
    let config = Config::load();

    // Create event loop
    let mut event_loop: EventLoop<TermStack> = EventLoop::try_new()?;

    // Create Wayland display
    let display: Display<TermStack> = Display::new()?;

    // Initialize X11 backend
    let window_title = match config.theme {
        crate::config::Theme::Light => "Column Compositor (Light)",
        crate::config::Theme::Dark => "Column Compositor (Dark)",
    };

    let x11_backend = X11Backend::new()
        .map_err(|e| anyhow::anyhow!("X11 backend init error: {e:?}"))?;
    let x11_handle = x11_backend.handle();

    // Create window
    let x11_window = WindowBuilder::new()
        .title(window_title)
        .size((1280u16, 800u16).into())
        .build(&x11_handle)
        .map_err(|e| anyhow::anyhow!("X11 window creation error: {e:?}"))?;

    // Get DRM node for GPU rendering
    let (_drm_node, fd) = x11_handle.drm_node()
        .map_err(|e| anyhow::anyhow!("Failed to get DRM node: {e:?}"))?;

    // Create GBM device for buffer allocation
    let gbm_device = GbmDevice::new(DeviceFd::from(fd))
        .map_err(|e| anyhow::anyhow!("Failed to create GBM device: {e:?}"))?;

    // Create EGL display and context for OpenGL rendering
    let egl_display = unsafe { EGLDisplay::new(gbm_device.clone()) }
        .map_err(|e| anyhow::anyhow!("Failed to create EGL display: {e:?}"))?;
    let egl_context = EGLContext::new(&egl_display)
        .map_err(|e| anyhow::anyhow!("Failed to create EGL context: {e:?}"))?;

    // Get supported modifiers for buffer allocation
    let modifiers: HashSet<_> = egl_context
        .dmabuf_render_formats()
        .iter()
        .map(|format| format.modifier)
        .collect();

    // Create X11 surface for presenting buffers
    let mut x11_surface = x11_handle.create_surface(
        &x11_window,
        DmabufAllocator(GbmAllocator::new(gbm_device, GbmBufferFlags::RENDERING)),
        modifiers.into_iter(),
    ).map_err(|e| anyhow::anyhow!("Failed to create X11 surface: {e:?}"))?;

    // Create GLES renderer from EGL context
    let mut renderer = unsafe { GlesRenderer::new(egl_context) }
        .map_err(|e| anyhow::anyhow!("Failed to create GLES renderer: {e:?}"))?;

    // Map the window to make it visible
    x11_window.map();

    // Create cursor manager for resize cursor feedback
    let mut cursor_manager = match CursorManager::new(
        x11_handle.connection(),
        x11_handle.screen(),
        x11_window.id(),
    ) {
        Ok(cm) => Some(cm),
        Err(e) => {
            tracing::warn!(?e, "failed to create cursor manager, cursor changes disabled");
            None
        }
    };

    let initial_size = x11_window.size();
    let mode = Mode {
        size: (initial_size.w as i32, initial_size.h as i32).into(),
        refresh: 60_000,
    };

    let output = Output::new(
        "x11".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Smithay".to_string(),
            model: "X11".to_string(),
        },
    );
    output.change_current_state(Some(mode), Some(Transform::Normal), None, Some((0, 0).into()));
    output.set_preferred(mode);

    // Convert logical to physical size
    let output_size: Size<i32, Physical> = Size::from((mode.size.w, mode.size.h));

    // Track current window size for resize events
    let mut current_size = initial_size;

    // Create compositor state (keep display separate for dispatching)
    let (mut compositor, mut display) = TermStack::new(
        display,
        event_loop.handle(),
        output_size,
        config.csd_apps.clone(),
    );

    // Add output to compositor
    compositor.space.map_output(&output, (0, 0));

    // Create output global so clients can discover it
    let _output_global = output.create_global::<TermStack>(&compositor.display_handle);

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

    // Save original DISPLAY for clipboard operations
    // We need this because arboard uses X11 clipboard, and some X11 operations
    // may need the DISPLAY variable even after initial connection
    let host_x11_display = std::env::var("DISPLAY").ok();
    if let Some(ref x11_display) = host_x11_display {
        std::env::set_var("HOST_DISPLAY", x11_display);
        tracing::info!(x11_display, "saved host DISPLAY");
    }

    // Set WAYLAND_DISPLAY for child processes (apps will open inside compositor)
    std::env::set_var("WAYLAND_DISPLAY", &socket_name);

    // Force GTK and Qt apps to use Wayland backend (otherwise they may use X11/Xwayland)
    std::env::set_var("GDK_BACKEND", "wayland");
    std::env::set_var("QT_QPA_PLATFORM", "wayland");

    // Unset DISPLAY so X11 apps spawned from terminals use our XWayland, not the host.
    // XWayland will set DISPLAY when it's ready.
    // Note: Terminals spawned before XWayland is ready won't have DISPLAY set.
    std::env::remove_var("DISPLAY");

    // Insert socket source into event loop for new client connections
    event_loop.handle().insert_source(listening_socket, |client_stream, _, state| {
        tracing::info!("new Wayland client connected");
        state.display_handle.insert_client(client_stream, std::sync::Arc::new(ClientState {
            compositor_state: Default::default(),
        })).expect("failed to insert client");
    }).expect("failed to insert socket source");

    // Create IPC socket for termstack commands
    let ipc_socket_path = crate::ipc::socket_path();
    let _ = std::fs::remove_file(&ipc_socket_path); // Clean up old socket
    let ipc_listener = UnixListener::bind(&ipc_socket_path)
        .expect("failed to create IPC socket");
    ipc_listener.set_nonblocking(true).expect("failed to set nonblocking");

    // Set environment variable for child processes
    std::env::set_var("TERMSTACK_SOCKET", &ipc_socket_path);

    tracing::info!(
        path = ?ipc_socket_path,
        env_set = ?std::env::var("TERMSTACK_SOCKET"),
        "IPC socket created, TERMSTACK_SOCKET env var set"
    );

    // Insert IPC socket source into event loop
    event_loop.handle().insert_source(
        Generic::new(ipc_listener, Interest::READ, CalloopMode::Level),
        |_, listener, state| {
            // Accept incoming connections
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        tracing::info!("IPC connection received");
                        match crate::ipc::read_ipc_request(stream) {
                            Ok((request, stream)) => {
                                match request {
                                    crate::ipc::IpcRequest::Spawn(spawn_req) => {
                                        // Detect if this is a gui command that slipped through the fish integration
                                        if spawn_req.command.starts_with("gui ") || spawn_req.command == "gui" {
                                            tracing::warn!(
                                                command = %spawn_req.command,
                                                "Ignoring 'gui' command - use 'gui' function from shell integration"
                                            );
                                            // Don't spawn - this would cause an infinite loop
                                        } else if spawn_req.foreground.is_some() {
                                            // GUI spawn request (foreground or background mode)
                                            tracing::info!(
                                                command = %spawn_req.command,
                                                foreground = spawn_req.foreground,
                                                "IPC GUI spawn request queued"
                                            );
                                            state.pending_gui_spawn_requests.push(spawn_req);
                                        } else {
                                            // Terminal spawn request
                                            tracing::info!(command = %spawn_req.command, "IPC terminal spawn request queued");
                                            state.pending_spawn_requests.push(spawn_req);
                                        }
                                        // Spawn doesn't need ACK - it's fire-and-forget
                                    }
                                    crate::ipc::IpcRequest::Resize(mode) => {
                                        tracing::info!(?mode, "IPC resize request queued");
                                        // Store stream for ACK after resize completes
                                        state.pending_resize_request = Some((mode, stream));
                                    }
                                }
                            }
                            Err(crate::ipc::IpcError::Timeout) => {
                                tracing::debug!("IPC read timeout");
                            }
                            Err(crate::ipc::IpcError::EmptyMessage) => {
                                tracing::debug!("IPC received empty message");
                            }
                            Err(e) => {
                                tracing::warn!(error = ?e, "IPC request parsing failed");
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        tracing::warn!(error = ?e, "IPC accept error");
                        break;
                    }
                }
            }
            Ok(smithay::reexports::calloop::PostAction::Continue)
        },
    ).expect("failed to insert IPC socket source");

    // Create channel for X11 input events
    // The X11 backend callback will send events, main loop will receive them
    let (x11_event_tx, x11_event_rx) = mpsc::channel::<InputEvent<X11Input>>();

    // Insert X11 backend into event loop
    event_loop.handle().insert_source(x11_backend, move |event, _, state| {
        match event {
            X11Event::Input { event: input_event, .. } => {
                // Log that we received an X11 input event (helps debug freezes)
                tracing::debug!("X11 input event received");
                // Send input events through channel to be processed in main loop
                // where we have access to terminal_manager
                let _ = x11_event_tx.send(input_event);
            }
            X11Event::Resized { new_size, .. } => {
                state.compositor_window_resize_pending = Some((new_size.w, new_size.h));
            }
            X11Event::CloseRequested { .. } => {
                state.running = false;
            }
            X11Event::Focus { focused, .. } => {
                tracing::info!("X11 window focus changed: {}", focused);
            }
            X11Event::Refresh { .. } => {
                // Window needs redraw - will happen in main loop anyway
            }
            X11Event::PresentCompleted { .. } => {
                // Buffer presentation complete - ready for next frame
            }
        }
    }).expect("failed to insert X11 backend source");

    // Initialize XWayland support for X11 apps
    xwayland_lifecycle::initialize_xwayland(&mut compositor, &mut display, event_loop.handle());

    tracing::info!("entering main loop");

    let bg_color = Color32F::new(
        config.background_color[0],
        config.background_color[1],
        config.background_color[2],
        config.background_color[3],
    );

    // Create terminal manager with output size and theme
    let terminal_theme = config.theme.to_terminal_theme();
    let mut terminal_manager = TerminalManager::new_with_size(
        output_size.w as u32,
        output_size.h as u32,
        terminal_theme,
    );

    // Create title bar renderer for external windows
    let mut title_bar_renderer = TitleBarRenderer::new(terminal_theme);
    if title_bar_renderer.is_none() {
        tracing::warn!("Title bar renderer unavailable - no font found");
    }

    // Cache for title bar textures to avoid re-rendering every frame
    let mut title_bar_cache: TitleBarCache = TitleBarCache::new();

    // Initial terminal will be spawned after XWayland is ready (in main loop)
    // This ensures DISPLAY is set correctly for X11 app support

    // Main event loop
    while compositor.running {
        // Clear stale drag state if no pointer buttons are pressed
        // This handles lost release events when window loses focus mid-drag
        compositor.clear_stale_drag_state(compositor.pointer_buttons_pressed > 0);

        // Cancel pending resizes from unresponsive clients
        compositor.cancel_stale_pending_resizes();

        // Spawn initial terminal once XWayland is ready
        if compositor.spawn_initial_terminal {
            compositor.spawn_initial_terminal = false;
            match terminal_manager.spawn() {
                Ok(id) => {
                    compositor.add_terminal(id);
                    tracing::info!(id = id.0, "spawned initial terminal (XWayland ready)");
                }
                Err(e) => {
                    tracing::error!(error = ?e, "failed to spawn initial terminal");
                }
            }
        }

        // Monitor xwayland-satellite health and auto-restart on crash with backoff
        xwayland_lifecycle::monitor_xwayland_satellite_health(&mut compositor);

        // Handle external window insert/resize events
        crate::window_lifecycle::handle_external_window_events(&mut compositor, &mut terminal_manager);

        // Update cell heights for input event processing
        let window_heights = calculate_window_heights(&compositor, &terminal_manager);
        compositor.update_layout_heights(window_heights);

        // Update Space positions to match current terminal height and scroll
        // This ensures Space.element_under works correctly for click detection
        compositor.recalculate_layout();

        // Process X11 input events from channel
        while let Ok(input_event) = x11_event_rx.try_recv() {
            compositor.process_input_event_with_terminals(input_event, &mut terminal_manager);
        }

        // Handle X11 resize events
        if let Some((new_w, new_h)) = compositor.compositor_window_resize_pending.take() {
            let new_size: Size<i32, Physical> = (new_w as i32, new_h as i32).into();
            // Update Smithay output mode
            output.change_current_state(
                Some(Mode {
                    size: new_size,
                    refresh: 60_000,
                }),
                None,
                None,
                None,
            );
            // Handle compositor-side resize
            handle_compositor_resize(
                &mut compositor,
                &mut terminal_manager,
                Size::from((new_size.w, new_size.h)),
            );
            current_size = (new_w, new_h).into();
        }

        // Update cursor icon based on whether pointer is on a resize handle
        if let Some(ref mut cm) = cursor_manager {
            cm.set_resize_cursor(compositor.cursor_on_resize_handle);
        }

        if !compositor.running {
            break;
        }

        // Cleanup popup internal resources
        compositor.popup_manager.cleanup();

        // Dispatch Wayland client requests
        display.dispatch_clients(&mut compositor)
            .expect("failed to dispatch clients");

        // Handle terminal spawn requests (keyboard shortcut)
        crate::window_lifecycle::handle_terminal_spawn(
            &mut compositor,
            &mut terminal_manager,
            calculate_window_heights,
        );

        // Handle command spawn requests from IPC (termstack)
        crate::spawn_handler::handle_ipc_spawn_requests(
            &mut compositor,
            &mut terminal_manager,
            calculate_window_heights,
        );

        // Handle GUI spawn requests from IPC (termstack gui)
        crate::spawn_handler::handle_gui_spawn_requests(
            &mut compositor,
            &mut terminal_manager,
            calculate_window_heights,
        );

        // Handle resize requests from IPC (termstack --resize)
        crate::terminal_output::handle_ipc_resize_request(&mut compositor, &mut terminal_manager);

        // Handle key repeat for terminals
        handle_key_repeat(&mut compositor, &mut terminal_manager);

        // Handle focus and scroll requests from input
        handle_focus_and_scroll_requests(&mut compositor, &mut terminal_manager);

        // Process terminal PTY output and handle sizing actions
        crate::terminal_output::process_terminal_output(&mut compositor, &mut terminal_manager);

        // Promote output terminals that have content to standalone cells
        crate::terminal_output::promote_output_terminals(&mut compositor, &terminal_manager);

        // Handle cleanup of output terminals from closed windows
        crate::window_lifecycle::handle_output_terminal_cleanup(&mut compositor, &mut terminal_manager);

        // Cleanup dead terminals and handle focus changes
        if crate::window_lifecycle::cleanup_and_sync_focus(&mut compositor, &mut terminal_manager) {
            break;
        }

        // Get window size for rendering
        let physical_size: Size<i32, Physical> = Size::from((current_size.w as i32, current_size.h as i32));

        #[allow(deprecated)]
        let damage: Rectangle<i32, Physical> = Rectangle::from_loc_and_size(
            (0, 0),
            (current_size.w as i32, current_size.h as i32),
        );

        // Get buffer from X11 surface for rendering
        let (mut buffer, _buffer_age) = match x11_surface.buffer() {
            Ok(buf) => buf,
            Err(e) => {
                tracing::warn!(error = ?e, "Failed to get X11 surface buffer");
                continue;
            }
        };

        // Render frame - bind the buffer to the renderer
        {
            let mut framebuffer = match renderer.bind(&mut buffer) {
                Ok(fb) => fb,
                Err(e) => {
                    tracing::warn!(error = ?e, "Failed to bind buffer");
                    continue;
                }
            };

            let scale = Scale::from(1.0);

            // Pre-render all terminal textures
            prerender_terminals(&mut terminal_manager, &mut renderer);

            // Pre-render title bar textures for all cells with SSD
            let title_bar_textures = prerender_title_bars(
                &compositor.layout_nodes,
                &mut title_bar_renderer,
                &terminal_manager,
                &mut renderer,
                physical_size.w,
                &mut title_bar_cache,
            );

            // Collect actual heights and external window elements
            let (actual_heights, mut external_elements) = collect_window_data(
                &compositor.layout_nodes,
                &terminal_manager,
                &mut renderer,
                scale,
            );

            // Build heights for positioning:
            // - Terminals being resized: use node.height for instant visual feedback
            // - Terminals NOT resizing: use actual_heights (includes title bar from collect_window_data)
            // - External windows being resized: use drag target height
            // - External windows NOT resizing: use committed height from WindowState
            let layout_heights: Vec<i32> = compositor.layout_nodes
                .iter()
                .enumerate()
                .map(|(i, node)| {
                    // Check if this window is being resized
                    let is_resizing = compositor.resizing
                        .as_ref()
                        .map(|drag| drag.window_index == i)
                        .unwrap_or(false);

                    match &node.cell {
                        StackWindow::Terminal(_) => {
                            if is_resizing {
                                // Being resized: use node.height for instant visual feedback
                                // (avoids lag from texture rendering)
                                node.height
                            } else {
                                // Not resizing: use actual_heights which includes title bar
                                actual_heights[i]
                            }
                        }
                        StackWindow::External(entry) => {
                            if is_resizing {
                                if let Some(drag) = &compositor.resizing {
                                    // Being resized: use drag target for visual feedback
                                    return drag.target_height;
                                }
                            }
                            // Not being resized: use committed height from WindowState
                            entry.state.current_height() as i32
                        }
                    }
                })
                .collect();

            // Build render data with computed Y positions
            let render_data = build_render_data(
                &compositor.layout_nodes,
                &layout_heights,
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
                compositor.focused_index(),
                physical_size.h,
            );

            // Check height changes and auto-scroll if needed
            check_and_handle_height_changes(&mut compositor, actual_heights);

            // Collect popup elements BEFORE starting the frame (need renderer access)
            // Store: (popup_x, popup_top, geo_offset_x, geo_offset_y, elements)
            // where popup_x/popup_top is where the popup content should appear in render coords
            let mut popup_render_data: PopupRenderData = Vec::new();

            for (window_idx, data) in render_data.iter().enumerate() {
                if let CellRenderData::External { y, .. } = data {
                    if let Some(node) = compositor.layout_nodes.get(window_idx) {
                        if let StackWindow::External(entry) = &node.cell {
                            // Get parent window geometry for proper popup positioning
                            // The geometry tells us where actual content is vs shadow/decoration areas
                            let parent_window_geo = entry.window.geometry();

                            // Get the wl_surface for popup handling
                            let wl_surface = entry.surface.wl_surface();
                            for (popup_kind, popup_offset) in PopupManager::popups_for_surface(wl_surface) {
                                let popup_surface = match &popup_kind {
                                    PopupKind::Xdg(xdg_popup) => xdg_popup,
                                    _ => continue,
                                };

                                let wl_surface = popup_surface.wl_surface();

                                // Two geometries to consider:
                                // 1. popup_position_geo: where popup content should appear relative to parent surface
                                //    (from our configure, stored in pending state)
                                // 2. popup_window_geo: where content is within the popup surface
                                //    (from client's set_window_geometry, for shadows/decorations)
                                let popup_position_geo = popup_surface.with_pending_state(|state| state.geometry);
                                let popup_window_geo = popup_kind.geometry();

                                // Popup position relative to parent surface
                                // If popup_offset from PopupManager is non-zero, use it; otherwise use our configured geometry
                                let popup_position = if popup_offset.x != 0 || popup_offset.y != 0 {
                                    popup_offset
                                } else {
                                    Point::from((popup_position_geo.loc.x, popup_position_geo.loc.y))
                                };

                                // Parent window's client area top in render coords
                                let title_bar_offset = if entry.uses_csd { 0 } else { TITLE_BAR_HEIGHT as i32 };
                                let client_area_top = *y + node.height - title_bar_offset;

                                // Calculate popup CONTENT position in screen coords
                                // popup_position is relative to parent surface, so add parent's screen offset
                                let popup_content_x = popup_position.x + parent_window_geo.loc.x + crate::render::FOCUS_INDICATOR_WIDTH;
                                let popup_content_top = client_area_top - popup_position.y - parent_window_geo.loc.y;

                                // Popup SURFACE position = content position minus window geometry offset
                                // If popup has shadows, window_geo.loc is where content starts within surface
                                let popup_surface_x = popup_content_x - popup_window_geo.loc.x;
                                let popup_surface_top = popup_content_top + popup_window_geo.loc.y;

                                tracing::trace!(
                                    ?popup_position,
                                    ?popup_window_geo,
                                    popup_surface_x,
                                    popup_surface_top,
                                    "popup render position"
                                );

                                let popup_elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                                    render_elements_from_surface_tree(
                                        &mut renderer,
                                        wl_surface,
                                        Point::from((0i32, 0i32)),
                                        scale,
                                        1.0,
                                        Kind::Unspecified,
                                    );

                                if !popup_elements.is_empty() {
                                    // Store surface position (not content position) for rendering
                                    popup_render_data.push((popup_surface_x, popup_surface_top, 0, 0, popup_elements));
                                }
                            }
                        }
                    }
                }
            }

            // Begin actual rendering
            // X11 backend needs Flipped180 because OpenGL Y=0 is at bottom but X11 Y=0 is at top
            let mut frame = renderer.render(&mut framebuffer, physical_size, Transform::Flipped180)
                .map_err(|e| anyhow::anyhow!("render error: {e:?}"))?;

            frame.clear(bg_color, &[damage])
                .map_err(|e| anyhow::anyhow!("clear error: {e:?}"))?;

            // Render all cells
            for (window_idx, data) in render_data.into_iter().enumerate() {
                let is_focused = compositor.focused_index() == Some(window_idx);

                match data {
                    CellRenderData::Terminal { id, y, height, title_bar_texture } => {
                        // Check if terminal is still running (for indicator)
                        let is_running = terminal_manager.get(id)
                            .map(|t| !t.has_exited())
                            .unwrap_or(false);

                        render_terminal(
                            &mut frame,
                            &terminal_manager,
                            id,
                            y,
                            height,
                            title_bar_texture,
                            is_focused,
                            is_running,
                            physical_size,
                            damage,
                        );
                    }
                    CellRenderData::External { y, height, elements, title_bar_texture, uses_csd } => {
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
                            uses_csd,
                        );
                    }
                }
            }

            // Render popups on top of all cells (using pre-collected elements)
            // popup_render_data contains (popup_surface_x, popup_surface_top, _, _, elements)
            // popup_surface_x/top is where the popup SURFACE origin should render (already adjusted for window geometry)
            for (popup_surface_x, popup_surface_top, _, _, popup_elements) in popup_render_data {
                for element in popup_elements.iter() {
                    let geo = element.geometry(scale);
                    let src = element.src();

                    // Element geometry is relative to popup surface origin
                    // popup_surface_top is the TOP of the popup surface in render coords (Y increases upward)
                    let dest_x = geo.loc.x + popup_surface_x;
                    let dest_y = popup_surface_top - geo.size.h + geo.loc.y;

                    let dest = Rectangle::new(
                        Point::from((dest_x, dest_y)),
                        geo.size,
                    );

                    // Flip source for OpenGL Y coordinates
                    let flipped_src = Rectangle::new(
                        Point::from((src.loc.x, src.loc.y + src.size.h)),
                        Size::from((src.size.w, -src.size.h)),
                    );

                    element.draw(&mut frame, flipped_src, dest, &[damage], &[]).ok();
                }
            }
        }

        // Submit the rendered buffer to X11
        if let Err(e) = x11_surface.submit() {
            tracing::warn!(error = ?e, "Failed to submit X11 surface");
        }

        // Send frame callbacks to all toplevel surfaces and their popups
        let toplevel_count = compositor.xdg_shell_state.toplevel_surfaces().len();
        let mut total_popup_count = 0;
        let mut frame_callback_popup_ids: Vec<_> = Vec::new();
        for surface in compositor.xdg_shell_state.toplevel_surfaces() {
            send_frames_surface_tree(
                surface.wl_surface(),
                &output,
                Duration::ZERO,
                Some(Duration::ZERO),  // Also use throttle for toplevels
                |_, _| Some(output.clone()),
            );

            // Send frame callbacks to popups for this toplevel
            // Use Some(Duration::ZERO) throttle to always send callbacks when overdue.
            // With None, callbacks only sent when on_primary_scanout_output matches,
            // but popup surfaces may not have output_update called on them.
            let popups: Vec<_> = PopupManager::popups_for_surface(surface.wl_surface()).collect();
            let popup_count = popups.len();
            total_popup_count += popup_count;
            for (popup, _) in popups {
                frame_callback_popup_ids.push(format!("{:?}", popup.wl_surface().id()));
                send_frames_surface_tree(
                    popup.wl_surface(),
                    &output,
                    Duration::ZERO,
                    Some(Duration::ZERO),
                    |_, _| Some(output.clone()),
                );
            }
        }
        if total_popup_count > 0 {
            tracing::debug!(toplevel_count, total_popup_count, "frame callbacks sent to toplevels and popups");
        }

        // Flush clients
        compositor.display_handle.flush_clients()?;

        // Dispatch calloop events
        // With X11 backend, calloop properly handles all events including X11 input
        // Use 16ms timeout for ~60fps frame rate
        event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut compositor)
            .map_err(|e| anyhow::anyhow!("event loop error: {e}"))?;
    }

    // Terminate xwayland-satellite on compositor shutdown
    if let Some(mut monitor) = compositor.xwayland_satellite.take() {
        if let Err(e) = monitor.child.kill() {
            tracing::warn!(?e, "Failed to kill xwayland-satellite");
        }
        if let Err(e) = monitor.child.wait() {
            tracing::warn!(?e, "Failed to wait for xwayland-satellite");
        }
        tracing::info!("xwayland-satellite terminated");
    }

    tracing::info!("compositor shutting down");

    Ok(())
}

pub fn setup_logging() {
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

/// Calculate cell heights for layout.
///
/// All cells store visual height in node.height (including title bar for SSD windows).
/// This is set by configure_notify (X11), configure_ack (Wayland), or terminal resize.
///
/// For terminals: hidden terminals always get 0 height; otherwise uses cached height
/// if available, falls back to terminal.height for new cells (already includes title bar).
///
/// For external windows: uses cached visual height if available, otherwise computes
/// from window state (which stores content height, so we add title bar for SSD).
fn calculate_window_heights(
    compositor: &TermStack,
    terminal_manager: &TerminalManager,
) -> Vec<i32> {
    compositor.layout_nodes.iter().map(|node| {
        match &node.cell {
            StackWindow::Terminal(tid) => {
                // Hidden terminals always get 0 height
                if !terminal_manager.is_terminal_visible(*tid) {
                    return 0;
                }
                // Use cached visual height if available
                if node.height > 0 {
                    return node.height;
                }
                // Fallback for new cells: terminal.height is content height, add title bar if needed
                terminal_manager.get(*tid)
                    .map(|t| {
                        let content = t.height as i32;
                        if t.show_title_bar {
                            content + TITLE_BAR_HEIGHT as i32
                        } else {
                            content
                        }
                    })
                    .unwrap_or(DEFAULT_TERMINAL_HEIGHT)
            }
            StackWindow::External(entry) => {
                // Use cached visual height if available
                if node.height > 0 {
                    return node.height;
                }
                // Fallback for new cells: window state stores content height
                let content_height = entry.state.current_height() as i32;
                if entry.uses_csd {
                    content_height
                } else {
                    // Add title bar for SSD windows to get visual height
                    content_height + TITLE_BAR_HEIGHT as i32
                }
            }
        }
    }).collect()
}

/// Handle key repeat for terminal input.
///
/// When a key is held down, this sends repeat events at regular intervals.
fn handle_key_repeat(
    compositor: &mut TermStack,
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

    if let Some(terminal) = terminal_manager.get_focused_mut(compositor.focused_window.as_ref()) {
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

/// Handle compositor window resize.
///
/// Updates all terminals and external windows to match the new size,
/// and recalculates the layout.
fn handle_compositor_resize(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
    new_size: Size<i32, Physical>,
) {
    compositor.output_size = new_size;

    // Update terminal manager dimensions
    terminal_manager.update_output_size(new_size.w as u32, new_size.h as u32);

    // Resize all existing terminals to new width
    terminal_manager.resize_all_terminals(new_size.w as u32);

    // Resize all external windows to new width
    compositor.resize_all_external_windows(new_size.w);

    compositor.recalculate_layout();
}

/// Handle focus change and scroll requests from input handlers.
///
/// This processes the `focus_change_requested` and `scroll_requested` fields
/// set by the input handler, applying the changes to compositor state.
fn handle_focus_and_scroll_requests(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
) {
    // Handle focus change requests
    if compositor.focus_change_requested != 0 {
        // Create visibility checker closure
        let is_terminal_visible = |id| terminal_manager.is_terminal_visible(id);

        if compositor.focus_change_requested > 0 {
            compositor.focus_next(is_terminal_visible);
        } else {
            compositor.focus_prev(is_terminal_visible);
        }
        compositor.focus_change_requested = 0;

        // Update keyboard focus to match the newly focused cell
        compositor.update_keyboard_focus_for_focused_window();

        // Scroll to show focused cell
        if let Some(focused_idx) = compositor.focused_index() {
            compositor.scroll_to_show_window_bottom(focused_idx);
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
}

/// Check if heights changed significantly and auto-scroll if needed.
///
/// This updates the layout heights cache and adjusts scroll to keep the focused
/// cell visible when content changes size. Skips height updates entirely during
/// manual resize to avoid overwriting the user's drag updates.
fn check_and_handle_height_changes(
    compositor: &mut TermStack,
    _actual_heights: Vec<i32>,
) {
    let is_resizing = compositor.resizing.is_some();

    // During resize: update layout positions to show target state for visual feedback
    // - Terminals: instant resize (drag-updated height)
    // - External windows being resized: use TARGET height for layout (shows final positions)
    // - External windows NOT being resized: use committed height
    // The resizing window will render at committed size but be positioned at target size,
    // giving visual feedback without flickering
    let heights_to_apply: Vec<i32> = compositor.layout_nodes.iter().enumerate().map(|(i, node)| {
        match &node.cell {
            StackWindow::Terminal(_) => {
                // Terminals: always use drag-updated height (instant resize)
                node.height
            }
            StackWindow::External(entry) => {
                // Check if this is the window being resized
                if let Some(drag) = &compositor.resizing {
                    if i == drag.window_index {
                        // Resizing window: use TARGET height for layout positioning
                        // (content still renders at committed size, but positioned at target)
                        return drag.target_height;
                    }
                }
                // Non-resizing external windows: use committed height from WindowState
                // This prevents flickering (no partial buffers) and jumping (no frame delay)
                entry.state.current_height() as i32
            }
        }
    }).collect();

    let current_heights: Vec<i32> = compositor
        .layout_nodes
        .iter()
        .map(|n| n.height)
        .collect();

    let heights_changed = heights_changed_significantly(
        &current_heights,
        &heights_to_apply,
        compositor.focused_index(),
    );

    // Skip autoscroll during resize to avoid disrupting drag
    let should_autoscroll = if heights_changed && !is_resizing {
        if let Some(focused_idx) = compositor.focused_index() {
            is_window_bottom_visible(compositor, focused_idx)
        } else {
            false
        }
    } else {
        false
    };

    compositor.update_layout_heights(heights_to_apply);

    // Adjust scroll if heights changed AND focused cell bottom was visible
    // This allows users to scroll up while content continues to flow in
    if should_autoscroll {
        if let Some(focused_idx) = compositor.focused_index() {
            if let Some(new_scroll) = compositor.scroll_to_show_window_bottom(focused_idx) {
                tracing::info!(
                    focused_idx,
                    new_scroll,
                    "scroll adjusted due to actual height change (bottom was visible)"
                );
            }
        }
    }
}

