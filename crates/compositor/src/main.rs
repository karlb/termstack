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

use compositor::config::Config;
use compositor::cursor::CursorManager;
use compositor::render::{
    CellRenderData, prerender_terminals, prerender_title_bars,
    collect_window_data, build_render_data, log_frame_state,
    heights_changed_significantly, render_terminal, render_external,
    TitleBarCache,
};
use compositor::state::{ClientState, StackWindow, TermStack, XWaylandSatelliteMonitor};
use compositor::terminal_manager::{TerminalId, TerminalManager};
use compositor::title_bar::{TitleBarRenderer, TITLE_BAR_HEIGHT};

/// Minimum terminal height in rows.
const MIN_TERMINAL_ROWS: u16 = 1;

/// Popup render data: (x, y, geo_offset_x, geo_offset_y, elements)
type PopupRenderData = Vec<(i32, i32, i32, i32, Vec<WaylandSurfaceRenderElement<GlesRenderer>>)>;

fn main() -> anyhow::Result<()> {
    // Initialize logging
    setup_logging();

    tracing::info!("starting termstack");

    // Load configuration
    let config = Config::load();

    // Create event loop
    let mut event_loop: EventLoop<TermStack> = EventLoop::try_new()?;

    // Create Wayland display
    let display: Display<TermStack> = Display::new()?;

    // Initialize X11 backend
    let window_title = match config.theme {
        compositor::config::Theme::Light => "Column Compositor (Light)",
        compositor::config::Theme::Dark => "Column Compositor (Dark)",
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
    let ipc_socket_path = compositor::ipc::socket_path();
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
                        match compositor::ipc::read_ipc_request(stream) {
                            Ok((request, stream)) => {
                                match request {
                                    compositor::ipc::IpcRequest::Spawn(spawn_req) => {
                                        // Detect if this is a gui command that slipped through the fish integration
                                        if spawn_req.command.starts_with("gui ") || spawn_req.command == "gui" {
                                            tracing::warn!(
                                                command = %spawn_req.command,
                                                "Ignoring 'gui' command via regular spawn - use 'gui' function from shell integration"
                                            );
                                            // Don't spawn - this would cause an infinite loop
                                            // The gui function should handle this via gui_spawn IPC instead
                                        } else {
                                            tracing::info!(command = %spawn_req.command, "IPC spawn request queued");
                                            state.pending_spawn_requests.push(spawn_req);
                                        }
                                        // Spawn doesn't need ACK - it's fire-and-forget
                                    }
                                    compositor::ipc::IpcRequest::GuiSpawn(gui_req) => {
                                        tracing::info!(
                                            command = %gui_req.command,
                                            foreground = gui_req.foreground,
                                            "IPC GUI spawn request received"
                                        );
                                        // Safety: prevent gui command from recursively spawning
                                        if gui_req.command.starts_with("gui ") || gui_req.command == "gui" {
                                            tracing::warn!(
                                                command = %gui_req.command,
                                                "Ignoring recursive gui spawn - command starts with 'gui'"
                                            );
                                        } else {
                                            state.pending_gui_spawn_requests.push(gui_req);
                                        }
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
    initialize_xwayland(&mut compositor, &mut display, event_loop.handle());

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
    let mut frame_start = std::time::Instant::now();
    while compositor.running {
        let frame_elapsed = frame_start.elapsed();
        frame_start = std::time::Instant::now();
        if compositor.resizing.is_some() && frame_elapsed.as_millis() > 20 {
            tracing::warn!(frame_ms = frame_elapsed.as_millis(), "resize: slow frame");
        }

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
                    tracing::error!("failed to spawn initial terminal: {}", e);
                }
            }
        }

        // Monitor xwayland-satellite health and auto-restart on crash with backoff
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
                    let now = std::time::Instant::now();
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

        // Handle external window insert/resize events
        handle_external_window_events(&mut compositor, &mut terminal_manager);

        // Update cell heights for input event processing
        let window_heights = calculate_window_heights(&compositor, &terminal_manager);
        compositor.update_layout_heights(window_heights);

        // Update Space positions to match current terminal height and scroll
        // This ensures Space.element_under works correctly for click detection
        compositor.recalculate_layout();

        // Process X11 input events from channel
        let mut event_count = 0;
        let input_start = std::time::Instant::now();
        while let Ok(input_event) = x11_event_rx.try_recv() {
            event_count += 1;
            compositor.process_input_event_with_terminals(input_event, &mut terminal_manager);
        }
        let input_elapsed = input_start.elapsed();
        if event_count > 0 && compositor.resizing.is_some() {
            tracing::info!(
                event_count,
                input_ms = input_elapsed.as_millis(),
                "resize: processed input events"
            );
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
        handle_terminal_spawn(&mut compositor, &mut terminal_manager);

        // Handle command spawn requests from IPC (termstack)
        handle_ipc_spawn_requests(&mut compositor, &mut terminal_manager);

        // Handle GUI spawn requests from IPC (termstack gui)
        handle_gui_spawn_requests(&mut compositor, &mut terminal_manager);

        // Handle resize requests from IPC (termstack --resize)
        handle_ipc_resize_request(&mut compositor, &mut terminal_manager);

        // Handle key repeat for terminals
        handle_key_repeat(&mut compositor, &mut terminal_manager);

        // Handle focus and scroll requests from input
        handle_focus_and_scroll_requests(&mut compositor, &mut terminal_manager);

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
                tracing::warn!("Failed to get X11 surface buffer: {:?}", e);
                continue;
            }
        };

        // Render frame - bind the buffer to the renderer
        {
            let mut framebuffer = match renderer.bind(&mut buffer) {
                Ok(fb) => fb,
                Err(e) => {
                    tracing::warn!("Failed to bind buffer: {:?}", e);
                    continue;
                }
            };

            let scale = Scale::from(1.0);

            // Pre-render all terminal textures
            let prerender_start = std::time::Instant::now();
            prerender_terminals(&mut terminal_manager, &mut renderer);
            let prerender_elapsed = prerender_start.elapsed();
            if compositor.resizing.is_some() && prerender_elapsed.as_millis() > 5 {
                tracing::warn!(prerender_ms = prerender_elapsed.as_millis(), "resize: slow prerender");
            }

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

            // Cache actual heights for resize handle detection in input.rs
            compositor.cached_actual_heights = actual_heights.clone();

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
                                let popup_content_x = popup_position.x + parent_window_geo.loc.x + compositor::render::FOCUS_INDICATOR_WIDTH;
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
            tracing::warn!("Failed to submit X11 surface: {:?}", e);
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
fn handle_external_window_events(
    compositor: &mut TermStack,
    _terminal_manager: &mut TerminalManager,
) {
    // Handle new external window - heights are already managed in add_window,
    // just need to scroll and set keyboard focus if needed
    if let Some(window_idx) = compositor.new_external_window_index.take() {
        let needs_keyboard_focus = std::mem::take(&mut compositor.new_window_needs_keyboard_focus);

        tracing::info!(
            window_idx,
            cells_count = compositor.layout_nodes.len(),
            focused_index = ?compositor.focused_index(),
            needs_keyboard_focus,
            "handling new external window"
        );

        // If this is a foreground GUI window, give it keyboard focus
        if needs_keyboard_focus {
            compositor.update_keyboard_focus_for_focused_window();
            tracing::info!(window_idx, "set keyboard focus to foreground GUI window");
        }

        // Scroll to show the focused cell
        if let Some(focused_idx) = compositor.focused_index() {
            if let Some(new_scroll) = compositor.scroll_to_show_window_bottom(focused_idx) {
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
        // Skip processing if this cell is currently being resized by the user
        // (don't let stale commits overwrite the drag updates)
        let is_resizing = compositor.resizing.as_ref().map(|d| d.window_index);
        if is_resizing == Some(resized_idx) {
            tracing::info!(
                resized_idx,
                new_height,
                is_resizing = ?is_resizing,
                "SKIPPING external_window_resized processing during active resize drag"
            );
        } else {
            tracing::info!(
                resized_idx,
                new_height,
                is_resizing = ?is_resizing,
                "handling external window resize"
            );

            // Check if focused cell bottom is visible before resize
            let should_autoscroll = if let Some(focused_idx) = compositor.focused_index() {
                focused_idx >= resized_idx && is_window_bottom_visible(compositor, focused_idx)
            } else {
                false
            };

            if let Some(node) = compositor.layout_nodes.get_mut(resized_idx) {
                node.height = new_height;
            }

            // Only autoscroll if focused cell is at/below resized window AND bottom was visible
            if should_autoscroll {
                if let Some(focused_idx) = compositor.focused_index() {
                    if let Some(new_scroll) = compositor.scroll_to_show_window_bottom(focused_idx) {
                        tracing::info!(
                            resized_idx,
                            focused_idx,
                            new_scroll,
                            "scrolled to show focused cell after external window resize (bottom was visible)"
                        );
                    }
                }
            }
        }
    }
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
                if terminal_manager.get(*tid).map(|t| !t.is_visible()).unwrap_or(false) {
                    return 0;
                }
                // Use cached visual height if available
                if node.height > 0 {
                    return node.height;
                }
                // Fallback for new cells: terminal.height already includes title bar
                terminal_manager.get(*tid)
                    .map(|t| t.height as i32)
                    .unwrap_or(200)
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

/// Handle keyboard shortcut to spawn a new terminal.
fn handle_terminal_spawn(
    compositor: &mut TermStack,
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
            let new_heights = calculate_window_heights(compositor, terminal_manager);
            compositor.update_layout_heights(new_heights);

            // Scroll to show the new terminal
            if let Some(focused_idx) = compositor.focused_index() {
                if let Some(new_scroll) = compositor.scroll_to_show_window_bottom(focused_idx) {
                    tracing::info!(
                        id = id.0,
                        window_count = compositor.layout_nodes.len(),
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

/// Handle IPC spawn requests from termstack.
///
/// Processes pending spawn requests, sets up environment, and spawns
/// command terminals with proper parent hiding.
fn handle_ipc_spawn_requests(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
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

/// Handle GUI spawn requests from IPC (termstack gui).
///
/// Spawns GUI app commands with foreground/background mode support.
/// In foreground mode, the launching terminal is hidden until the GUI exits.
fn handle_gui_spawn_requests(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
) {
    use compositor::terminal_manager::VisibilityState;

    while let Some(request) = compositor.pending_gui_spawn_requests.pop() {
        // Get the launching terminal (currently focused)
        use compositor::state::FocusedWindow;
        let launching_terminal = compositor.focused_window.as_ref().and_then(|cell| match cell {
            FocusedWindow::Terminal(id) => Some(*id),
            FocusedWindow::External(_) => None,
        });

        // Modify environment for GUI apps
        let mut env = request.env.clone();
        if let Ok(wayland_display) = std::env::var("WAYLAND_DISPLAY") {
            env.insert("WAYLAND_DISPLAY".to_string(), wayland_display);
        }
        if let Ok(gdk_backend) = std::env::var("GDK_BACKEND") {
            env.insert("GDK_BACKEND".to_string(), gdk_backend);
        }
        if let Ok(qt_platform) = std::env::var("QT_QPA_PLATFORM") {
            env.insert("QT_QPA_PLATFORM".to_string(), qt_platform);
        }
        if let Ok(shell) = std::env::var("SHELL") {
            env.insert("SHELL".to_string(), shell);
        }

        // Create output terminal with WaitingForOutput visibility
        let parent = launching_terminal;
        match terminal_manager.spawn_command(&request.command, &request.cwd, &env, parent) {
            Ok(output_terminal_id) => {
                tracing::info!(
                    output_terminal_id = output_terminal_id.0,
                    launching_terminal = ?launching_terminal,
                    foreground = request.foreground,
                    command = %request.command,
                    "spawned GUI command terminal"
                );

                // Add output terminal to layout
                compositor.add_terminal(output_terminal_id);

                // Set up for window linking
                compositor.pending_window_output_terminal = Some(output_terminal_id);
                compositor.pending_window_command = Some(request.command.clone());
                compositor.pending_gui_foreground = request.foreground;

                // If foreground mode, hide launching terminal and track the session
                if request.foreground {
                    if let Some(launcher_id) = launching_terminal {
                        if let Some(launcher) = terminal_manager.get_mut(launcher_id) {
                            launcher.visibility = VisibilityState::HiddenForForegroundGui;
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
                    use compositor::state::FocusedWindow;
                    compositor.focused_window = Some(FocusedWindow::Terminal(launcher_id));
                    tracing::debug!(
                        launcher_id = launcher_id.0,
                        "restored terminal focus to launcher after gui_spawn"
                    );
                }
            }
            Err(e) => {
                tracing::error!(command = %request.command, "failed to spawn GUI command: {}", e);
            }
        }
    }
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

    tracing::info!(
        width = new_size.w,
        height = new_size.h,
        "compositor window resized, content updated"
    );
}

/// Handle focus change and scroll requests from input handlers.
///
/// This processes the `focus_change_requested` and `scroll_requested` fields
/// set by the input handler, applying the changes to compositor state.
fn handle_focus_and_scroll_requests(
    compositor: &mut TermStack,
    _terminal_manager: &mut TerminalManager,
) {
    // Handle focus change requests
    if compositor.focus_change_requested != 0 {
        if compositor.focus_change_requested > 0 {
            compositor.focus_next();
        } else {
            compositor.focus_prev();
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

/// Check if the bottom of a cell is currently visible in the viewport.
///
/// Returns true if the cell's bottom edge is currently visible (on-screen),
/// false if the user has scrolled up past it.
///
/// This is used to prevent autoscroll when the user intentionally scrolls up
/// to view earlier content while new content continues to flow in.
fn is_window_bottom_visible(compositor: &TermStack, window_idx: usize) -> bool {
    let cell_top_y: i32 = compositor
        .layout_nodes
        .iter()
        .take(window_idx)
        .map(|n| n.height)
        .sum();
    let window_height = compositor
        .layout_nodes
        .get(window_idx)
        .map(|n| n.height)
        .unwrap_or(0);
    let cell_bottom_y = cell_top_y + window_height;
    let viewport_height = compositor.output_size.h;

    // Calculate minimum scroll needed to show cell bottom
    let min_scroll_for_bottom = (cell_bottom_y - viewport_height).max(0) as f64;

    // Cell bottom is visible if current scroll >= minimum needed
    // (allowing small epsilon for floating point comparison)
    compositor.scroll_offset >= (min_scroll_for_bottom - 1.0)
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

/// Process a single spawn request, returning the new terminal ID if successful.
fn process_spawn_request(
    compositor: &mut TermStack,
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

    use compositor::state::FocusedWindow;
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
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
) {
    // Process PTY output and get sizing actions
    let sizing_actions = terminal_manager.process_all();

    // Handle sizing actions
    for (id, action) in sizing_actions {
        if let terminal::sizing::SizingAction::RequestGrowth { target_rows } = action {
            // Skip auto-growth if terminal was manually resized
            if terminal_manager.get(id).map(|t| t.manually_sized).unwrap_or(false) {
                tracing::debug!(id = id.0, "skipping growth request - terminal was manually resized");
                continue;
            }

            tracing::info!(id = id.0, target_rows, "processing growth request");
            terminal_manager.grow_terminal(id, target_rows);

            // If focused terminal grew, update cache and scroll (if bottom was visible)
            use compositor::state::FocusedWindow;
            let is_focused = matches!(compositor.focused_window.as_ref(), Some(FocusedWindow::Terminal(fid)) if *fid == id);
            if is_focused {
                if let Some(idx) = find_terminal_window_index(compositor, id) {
                    // Check if bottom was visible before resize
                    let was_bottom_visible = is_window_bottom_visible(compositor, idx);

                    if let Some(term) = terminal_manager.get(id) {
                        if let Some(node) = compositor.layout_nodes.get_mut(idx) {
                            node.height = term.height as i32;
                        }
                    }

                    // Only autoscroll if bottom was already visible
                    // This allows users to scroll up while content flows in
                    if was_bottom_visible {
                        compositor.scroll_to_show_window_bottom(idx);
                        tracing::debug!(
                            id = id.0,
                            idx,
                            "autoscrolled after terminal growth (bottom was visible)"
                        );
                    } else {
                        tracing::debug!(
                            id = id.0,
                            idx,
                            "skipped autoscroll after terminal growth (bottom not visible)"
                        );
                    }
                }
            }
        }
    }

    // Auto-resize terminals entering alternate screen mode
    auto_resize_alt_screen_terminals(compositor, terminal_manager);
}

/// Auto-resize terminals that entered alternate screen mode.
fn auto_resize_alt_screen_terminals(
    compositor: &mut TermStack,
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
    let char_height = terminal_manager.cell_height;

    for id in ids_to_resize {
        if let Some(term) = terminal_manager.get_mut(id) {
            let old_height = term.height;
            term.resize(max_rows, char_height);
            let new_height = term.height;

            tracing::info!(
                id = id.0,
                old_height,
                new_height,
                "auto-resized terminal for alternate screen"
            );

            // Update cached height
            if let Some(idx) = find_terminal_window_index(compositor, id) {
                if let Some(node) = compositor.layout_nodes.get_mut(idx) {
                    node.height = new_height as i32;
                }
            }
        }
    }
}

/// Find the cell index for a terminal ID.
fn find_terminal_window_index(compositor: &TermStack, id: TerminalId) -> Option<usize> {
    compositor.layout_nodes.iter().enumerate().find_map(|(i, node)| {
        if let StackWindow::Terminal(tid) = node.cell {
            if tid == id {
                return Some(i);
            }
        }
        None
    })
}

/// Handle IPC resize request from termstack --resize.
///
/// Resizes the focused terminal to full or content-based height and
/// sends ACK to unblock the termstack process.
fn handle_ipc_resize_request(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
) {
    let Some((resize_mode, ack_stream)) = compositor.pending_resize_request.take() else {
        return;
    };

    use compositor::state::FocusedWindow;
    let focused_id = match compositor.focused_window.as_ref() {
        Some(FocusedWindow::Terminal(id)) => *id,
        _ => {
            tracing::warn!("resize request but no focused terminal");
            compositor::ipc::send_ack(ack_stream);
            return;
        }
    };

    let char_height = terminal_manager.cell_height;
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

            // Calculate content rows from last non-empty line
            if let Some(term) = terminal_manager.get(focused_id) {
                let last_line = term.terminal.last_content_line();
                // last_line is 0-indexed, so +1 converts to row count
                let content_rows = (last_line + 1).max(MIN_TERMINAL_ROWS);
                tracing::info!(id = focused_id.0, last_line, content_rows, "resize to content");
                content_rows
            } else {
                MIN_TERMINAL_ROWS
            }
        }
    };

    if let Some(term) = terminal_manager.get_mut(focused_id) {
        tracing::info!(id = focused_id.0, ?resize_mode, new_rows, "resizing terminal via IPC");
        term.resize(new_rows, char_height);

        // Update cached height
        for node in compositor.layout_nodes.iter_mut() {
            if let StackWindow::Terminal(tid) = node.cell {
                if tid == focused_id {
                    node.height = term.height as i32;
                    break;
                }
            }
        }

        // Scroll to keep terminal visible
        if let Some(idx) = compositor.focused_index() {
            compositor.scroll_to_show_window_bottom(idx);
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
    compositor: &mut TermStack,
    terminal_manager: &TerminalManager,
) {
    // Collect (window_idx, term_id) pairs for terminals to promote
    let mut to_promote: Vec<(usize, TerminalId)> = Vec::new();

    for (window_idx, node) in compositor.layout_nodes.iter().enumerate() {
        if let StackWindow::External(entry) = &node.cell {
            if let Some(term_id) = entry.output_terminal {
                // Check if terminal already in cells
                let already_cell = compositor.layout_nodes.iter().any(|n| {
                    matches!(n.cell, StackWindow::Terminal(id) if id == term_id)
                });

                if !already_cell {
                    if let Some(term) = terminal_manager.get(term_id) {
                        // Promote if terminal has any content
                        if term.content_rows() > 0 {
                            to_promote.push((window_idx, term_id));
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
        // (window_idx + 1 puts it below the window in the column)
        let insert_idx = window_idx + 1;
        
        let height = terminal_manager.get(term_id)
            .map(|t| t.height as i32)
            .unwrap_or(0);
            
        compositor.layout_nodes.insert(insert_idx, compositor::state::LayoutNode {
            cell: StackWindow::Terminal(term_id),
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
/// For foreground GUI sessions, restores the launching terminal's visibility.
fn handle_output_terminal_cleanup(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
) {
    let cleanup_ids = std::mem::take(&mut compositor.pending_output_terminal_cleanup);

    for term_id in cleanup_ids {
        let has_had_output = terminal_manager.get(term_id)
            .map(|t| t.has_had_output())
            .unwrap_or(false);

        // Check if this was a foreground GUI session and restore the launcher
        if let Some((launcher_id, _window_was_linked)) = compositor.foreground_gui_sessions.remove(&term_id) {
            if let Some(launcher) = terminal_manager.get_mut(launcher_id) {
                launcher.visibility = launcher.visibility.on_gui_exit();
                tracing::info!(
                    launcher_id = launcher_id.0,
                    output_terminal_id = term_id.0,
                    "restored launching terminal visibility after foreground GUI closed"
                );
            }

            // Focus the restored launcher
            if let Some(idx) = find_terminal_window_index(compositor, launcher_id) {
                compositor.set_focus_by_index(idx);
                tracing::info!(
                    launcher_id = launcher_id.0,
                    index = idx,
                    "focused restored launcher after foreground GUI closed"
                );
            }
        }

        if has_had_output {
            // Terminal has had output - keep it visible
            tracing::info!(
                terminal_id = term_id.0,
                "output terminal has had output, keeping visible after window close"
            );
        } else {
            // Never had output - remove from layout and TerminalManager
            compositor.layout_nodes.retain(|n| {
                !matches!(n.cell, StackWindow::Terminal(id) if id == term_id)
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
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
) -> bool {
    let (dead, focus_changed_to) = terminal_manager.cleanup();

    // Remove dead terminals from compositor
    for dead_id in &dead {
        // Fallback trigger: If this was an output terminal for a foreground GUI
        // that never opened a window, restore the launcher
        if let Some((launcher_id, window_was_linked)) = compositor.foreground_gui_sessions.remove(dead_id) {
            if !window_was_linked {
                // No window was ever linked - this is the fallback case
                // (e.g., GUI command failed before opening a window)
                if let Some(launcher) = terminal_manager.get_mut(launcher_id) {
                    launcher.visibility = launcher.visibility.on_gui_exit();
                    tracing::info!(
                        launcher_id = launcher_id.0,
                        output_terminal_id = dead_id.0,
                        "fallback: restored launcher after output terminal exited without window"
                    );
                }

                // Focus the restored launcher
                if let Some(idx) = find_terminal_window_index(compositor, launcher_id) {
                    compositor.set_focus_by_index(idx);
                    tracing::info!(
                        launcher_id = launcher_id.0,
                        index = idx,
                        "focused restored launcher after fallback"
                    );
                }
            }
        }

        compositor.remove_terminal(*dead_id);
        tracing::info!(id = dead_id.0, "removed dead terminal from cells");
    }

    // Sync compositor focus if a command terminal exited
    if let Some(new_focus_id) = focus_changed_to {
        if let Some(idx) = find_terminal_window_index(compositor, new_focus_id) {
            compositor.set_focus_by_index(idx);
            tracing::info!(id = new_focus_id.0, index = idx, "synced compositor focus to parent terminal");

            // Update cached height for unhidden terminal (was 0 when hidden)
            if let Some(term) = terminal_manager.get(new_focus_id) {
                if let Some(node) = compositor.layout_nodes.get_mut(idx) {
                    node.height = term.height as i32;
                }
            }

            // Scroll to show the unhidden parent terminal
            if let Some(new_scroll) = compositor.scroll_to_show_window_bottom(idx) {
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

/// Initialize XWayland support with xwayland-satellite for running X11 applications
///
/// xwayland-satellite acts as the X11 window manager and presents X11 windows as
/// normal Wayland toplevels to the compositor.
fn initialize_xwayland(
    _compositor: &mut TermStack,
    display: &mut Display<TermStack>,
    loop_handle: smithay::reexports::calloop::LoopHandle<'static, TermStack>,
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
