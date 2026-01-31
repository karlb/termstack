//! Column Compositor - A content-aware terminal compositor
//!
//! This compositor arranges terminal windows in a scrollable vertical column,
//! with windows dynamically sizing based on their content.
//!
//! # Backend Abstraction
//!
//! The compositor supports multiple rendering backends:
//! - **X11** (default): GPU-accelerated rendering using OpenGL/GLES
//! - **Headless** (feature): CPU-based software rendering for testing
//!
//! Backend selection is controlled by the `TERMSTACK_BACKEND` environment variable.

use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use smithay::backend::input::InputEvent;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::element::surface::render_elements_from_surface_tree;
use smithay::backend::renderer::element::{Element, Kind, RenderElement};
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::{Color32F, Frame, Renderer, Bind};
use smithay::backend::x11::{X11Event, X11Input};
use smithay::desktop::PopupKind;
use smithay::desktop::utils::send_frames_surface_tree;
use smithay::desktop::PopupManager;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::utils::Point;
use smithay::reexports::calloop::{EventLoop, generic::Generic, Interest, Mode as CalloopMode};
use smithay::reexports::wayland_server::{Display, Resource};
use smithay::utils::{Physical, Rectangle, Scale, Size, Transform};
use smithay::wayland::socket::ListeningSocketSource;

use crate::backend::{BackendType, select_backend};
use crate::config::Config;
use crate::render::{
    CellRenderData, prerender_terminals, prerender_title_bars,
    collect_window_data, build_render_data, log_frame_state, render_terminal, render_external,
    TitleBarCache,
};
use crate::state::{ClientState, StackWindow, TermStack};
use crate::xwayland_lifecycle;
use crate::terminal_manager::TerminalManager;
use crate::title_bar::{TitleBarRenderer, TITLE_BAR_HEIGHT};

/// Popup render data: (x, y, geo_offset_x, geo_offset_y, elements)
type PopupRenderData = Vec<(i32, i32, i32, i32, Vec<WaylandSurfaceRenderElement<GlesRenderer>>)>;

/// Main entry point for the compositor
///
/// Selects and runs the appropriate backend based on the `TERMSTACK_BACKEND`
/// environment variable:
/// - `x11` (default): GPU-accelerated X11 backend
/// - `headless`: CPU-based software rendering (requires `headless-backend` feature)
pub fn run_compositor() -> anyhow::Result<()> {
    match select_backend() {
        BackendType::X11 => run_compositor_x11(),
        BackendType::Headless => {
            #[cfg(feature = "headless-backend")]
            {
                run_compositor_headless()
            }
            #[cfg(not(feature = "headless-backend"))]
            {
                anyhow::bail!("Headless backend requested but `headless-backend` feature not enabled")
            }
        }
    }
}

#[cfg(feature = "headless-backend")]
fn run_compositor_headless() -> anyhow::Result<()> {
    use std::sync::Arc;
    use std::os::unix::net::UnixListener;

    tracing::info!("starting termstack with headless backend");

    // Load configuration
    let config = Config::load();

    // Create event loop
    let mut event_loop: EventLoop<TermStack> = EventLoop::try_new()?;

    // Create Wayland display
    let display: Display<TermStack> = Display::new()?;

    // Create a virtual output for layout calculations (no actual rendering)
    let mode = Mode {
        size: (1280, 800).into(),
        refresh: 60_000,
    };

    let output = Output::new(
        "headless".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "TermStack".to_string(),
            model: "Headless".to_string(),
        },
    );
    output.change_current_state(Some(mode), Some(Transform::Normal), None, Some((0, 0).into()));
    output.set_preferred(mode);

    let output_size: Size<i32, Physical> = Size::from((mode.size.w, mode.size.h));

    // Create compositor state (no renderer needed for headless)
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

    // Create listening socket for Wayland clients
    let listening_socket = ListeningSocketSource::new_auto()
        .expect("failed to create Wayland socket");

    let socket_name = listening_socket
        .socket_name()
        .to_string_lossy()
        .to_string();

    tracing::info!(?socket_name, "headless compositor listening on Wayland socket");

    // Set WAYLAND_DISPLAY for child processes
    std::env::set_var("WAYLAND_DISPLAY", &socket_name);

    // Force GTK and Qt apps to use Wayland backend
    std::env::set_var("GDK_BACKEND", "wayland");
    std::env::set_var("QT_QPA_PLATFORM", "wayland");

    // Insert socket source into event loop for new client connections
    event_loop.handle().insert_source(listening_socket, |client_stream, _, state| {
        tracing::info!("new Wayland client connected (headless)");
        state.display_handle.insert_client(client_stream, Arc::new(ClientState {
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

    // Set TERMSTACK_BIN so spawned terminals use matching CLI version
    let binary_path = std::env::current_exe()
        .expect("failed to determine binary path");
    std::env::set_var("TERMSTACK_BIN", &binary_path);

    tracing::info!(
        path = ?ipc_socket_path,
        "headless IPC socket created"
    );

    // Insert IPC socket source into event loop
    event_loop.handle().insert_source(
        Generic::new(ipc_listener, Interest::READ, CalloopMode::Level),
        |_, listener, state| {
            // Accept incoming connections
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        tracing::debug!("IPC connection received (headless)");
                        match crate::ipc::read_ipc_request(stream) {
                            Ok((request, stream)) => {
                                match request {
                                    crate::ipc::IpcRequest::Spawn(spawn_req) => {
                                        if spawn_req.foreground.is_some() {
                                            tracing::info!(
                                                command = %spawn_req.command,
                                                foreground = spawn_req.foreground,
                                                "IPC GUI spawn request queued (headless)"
                                            );
                                            state.pending_gui_spawn_requests.push(spawn_req);
                                        } else {
                                            tracing::info!(command = %spawn_req.command, "IPC terminal spawn request queued (headless)");
                                            state.pending_spawn_requests.push(spawn_req);
                                        }
                                    }
                                    crate::ipc::IpcRequest::Resize(mode) => {
                                        tracing::info!(?mode, "IPC resize request queued (headless)");
                                        state.pending_resize_request = Some((mode, stream));
                                    }
                                    crate::ipc::IpcRequest::Builtin(builtin_req) => {
                                        tracing::info!(
                                            command = %builtin_req.command,
                                            "IPC builtin request queued (headless)"
                                        );
                                        state.pending_builtin_requests.push(builtin_req);
                                    }
                                    crate::ipc::IpcRequest::QueryWindows => {
                                        // Build window info from layout_nodes
                                        let windows: Vec<crate::ipc::WindowInfo> = state.layout_nodes
                                            .iter()
                                            .enumerate()
                                            .map(|(i, node)| {
                                                let (is_external, command, actual_width) = match &node.cell {
                                                    crate::state::StackWindow::Terminal(_) => {
                                                        (false, String::new(), state.output_size.w)
                                                    }
                                                    crate::state::StackWindow::External(entry) => {
                                                        // Get actual window width from geometry
                                                        let geo = entry.window.geometry();
                                                        let width = if geo.size.w > 0 {
                                                            geo.size.w
                                                        } else {
                                                            state.output_size.w
                                                        };
                                                        (true, entry.command.clone(), width)
                                                    }
                                                };
                                                crate::ipc::WindowInfo {
                                                    index: i,
                                                    width: actual_width,
                                                    height: node.height,
                                                    is_external,
                                                    command,
                                                }
                                            })
                                            .collect();
                                        tracing::info!(window_count = windows.len(), "IPC query_windows response (headless)");
                                        crate::ipc::send_json_response(stream, &windows);
                                    }
                                }
                            }
                            Err(crate::ipc::IpcError::Timeout) => {
                                tracing::debug!("IPC read timeout (headless)");
                            }
                            Err(crate::ipc::IpcError::EmptyMessage) => {
                                tracing::debug!("IPC received empty message (headless)");
                            }
                            Err(e) => {
                                tracing::warn!(error = ?e, "IPC request parsing failed (headless)");
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => {
                        tracing::warn!(error = ?e, "IPC accept error (headless)");
                        break;
                    }
                }
            }
            Ok(smithay::reexports::calloop::PostAction::Continue)
        },
    ).expect("failed to insert IPC socket source");

    // Spawn initial terminal immediately in headless mode
    // (no XWayland to wait for)
    compositor.spawn_initial_terminal = true;

    // Create terminal manager for headless mode
    let terminal_theme = config.theme.to_terminal_theme();
    let mut terminal_manager = TerminalManager::new_with_size(
        output_size.w as u32,
        output_size.h as u32,
        terminal_theme,
        config.font_size,
    );

    tracing::info!("headless compositor entering main loop");

    // Main event loop (no rendering, just protocol dispatch)
    while compositor.running {
        // Spawn initial terminal if requested
        if compositor.spawn_initial_terminal {
            compositor.spawn_initial_terminal = false;
            match terminal_manager.spawn() {
                Ok(id) => {
                    compositor.add_terminal(id);
                    tracing::info!(id = id.0, "spawned initial terminal (headless)");
                }
                Err(e) => {
                    tracing::error!(error = ?e, "failed to spawn initial terminal (headless)");
                }
            }
        }

        // Handle external window insert/resize events
        crate::window_lifecycle::handle_external_window_events(&mut compositor);

        // Update cell heights for proper layout
        let window_heights = crate::window_height::calculate_window_heights(&compositor, &terminal_manager);
        compositor.update_layout_heights(window_heights);
        compositor.recalculate_layout();

        // Dispatch Wayland client requests
        display.dispatch_clients(&mut compositor)
            .expect("failed to dispatch clients");

        // Handle terminal spawn requests from IPC
        crate::spawn_handler::handle_ipc_spawn_requests(
            &mut compositor,
            &mut terminal_manager,
            crate::window_height::calculate_window_heights,
        );

        // Handle GUI spawn requests from IPC
        crate::spawn_handler::handle_gui_spawn_requests(
            &mut compositor,
            &mut terminal_manager,
            crate::window_height::calculate_window_heights,
        );

        // Handle builtin command requests from IPC
        crate::spawn_handler::handle_builtin_requests(
            &mut compositor,
            &mut terminal_manager,
            crate::window_height::calculate_window_heights,
        );

        // Handle resize requests from IPC
        crate::terminal_output::handle_ipc_resize_request(&mut compositor, &mut terminal_manager);

        // Process terminal PTY output
        crate::terminal_output::process_terminal_output(&mut compositor, &mut terminal_manager);

        // Promote output terminals that have content
        crate::terminal_output::promote_output_terminals(&mut compositor, &terminal_manager);

        // Handle cleanup of output terminals from closed windows
        crate::window_lifecycle::handle_output_terminal_cleanup(&mut compositor, &mut terminal_manager);

        // Cleanup dead terminals
        if crate::window_lifecycle::cleanup_and_sync_focus(&mut compositor, &mut terminal_manager) {
            break;
        }

        if !compositor.running {
            break;
        }

        // Send frame callbacks to Wayland clients (required for them to render)
        // In headless mode, we send these on a timer instead of after actual rendering
        for surface in compositor.xdg_shell_state.toplevel_surfaces() {
            send_frames_surface_tree(
                surface.wl_surface(),
                &output,
                Duration::ZERO,
                Some(Duration::ZERO),
                |_, _| Some(output.clone()),
            );
        }

        // Flush clients
        compositor.display_handle.flush_clients()?;

        // Dispatch calloop events with ~60fps timing
        event_loop
            .dispatch(Some(Duration::from_millis(16)), &mut compositor)
            .map_err(|e| anyhow::anyhow!("event loop error: {e}"))?;
    }

    tracing::info!("headless compositor shutting down");

    Ok(())
}

/// Run the compositor with the X11 backend
fn run_compositor_x11() -> anyhow::Result<()> {
    use smithay::backend::x11::{X11Backend, WindowBuilder};
    use smithay::backend::allocator::dmabuf::DmabufAllocator;
    use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
    use smithay::backend::egl::{EGLContext, EGLDisplay};
    use smithay::utils::DeviceFd;
    use std::collections::HashSet;
    use crate::cursor::CursorManager;
    use crate::icon::{set_window_class, set_window_icon};

    tracing::info!("starting termstack with X11 backend");

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

    // Set window icon and class BEFORE mapping (GNOME queries these on map)
    if let Err(e) = set_window_icon(&x11_handle.connection(), x11_window.id()) {
        tracing::warn!(?e, "failed to set window icon");
    }
    if let Err(e) = set_window_class(&x11_handle.connection(), x11_window.id()) {
        tracing::warn!(?e, "failed to set window class");
    }

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

    // Set TERMSTACK_BIN so spawned terminals use matching CLI version
    let binary_path = std::env::current_exe()
        .expect("failed to determine binary path");
    std::env::set_var("TERMSTACK_BIN", &binary_path);

    tracing::info!(
        path = ?ipc_socket_path,
        env_set = ?std::env::var("TERMSTACK_SOCKET"),
        binary = ?binary_path,
        "IPC socket created, TERMSTACK_SOCKET and TERMSTACK_BIN env vars set"
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
                                    crate::ipc::IpcRequest::Builtin(builtin_req) => {
                                        tracing::info!(
                                            command = %builtin_req.command,
                                            success = builtin_req.success,
                                            has_result = !builtin_req.result.is_empty(),
                                            "IPC builtin request queued"
                                        );
                                        state.pending_builtin_requests.push(builtin_req);
                                    }
                                    crate::ipc::IpcRequest::QueryWindows => {
                                        // Build window info from layout_nodes
                                        let windows: Vec<crate::ipc::WindowInfo> = state.layout_nodes
                                            .iter()
                                            .enumerate()
                                            .map(|(i, node)| {
                                                let (is_external, command, actual_width) = match &node.cell {
                                                    crate::state::StackWindow::Terminal(_) => {
                                                        (false, String::new(), state.output_size.w)
                                                    }
                                                    crate::state::StackWindow::External(entry) => {
                                                        // Get actual window width from geometry
                                                        let geo = entry.window.geometry();
                                                        let width = if geo.size.w > 0 {
                                                            geo.size.w
                                                        } else {
                                                            state.output_size.w
                                                        };
                                                        (true, entry.command.clone(), width)
                                                    }
                                                };
                                                crate::ipc::WindowInfo {
                                                    index: i,
                                                    width: actual_width,
                                                    height: node.height,
                                                    is_external,
                                                    command,
                                                }
                                            })
                                            .collect();
                                        tracing::info!(window_count = windows.len(), "IPC query_windows response");
                                        crate::ipc::send_json_response(stream, &windows);
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

    // Create terminal manager with output size, theme, and font size
    let terminal_theme = config.theme.to_terminal_theme();
    let mut terminal_manager = TerminalManager::new_with_size(
        output_size.w as u32,
        output_size.h as u32,
        terminal_theme,
        config.font_size,
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

    // Frame timing for render rate limiting
    // Skip renders when behind to avoid backlog, but always process events
    let mut last_render_time = Instant::now();
    const MIN_FRAME_TIME: Duration = Duration::from_millis(8); // ~120fps max

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
        crate::window_lifecycle::handle_external_window_events(&mut compositor);

        // Update cell heights for input event processing
        let window_heights = crate::window_height::calculate_window_heights(&compositor, &terminal_manager);
        compositor.update_layout_heights(window_heights);

        // Update Space positions to match current terminal height and scroll
        // This ensures Space.element_under works correctly for click detection
        compositor.recalculate_layout();

        // Drain all pending X11 input events before rendering.
        //
        // WORKAROUND: Smithay's X11Source uses a bounded sync_channel(5) which causes
        // scroll event backlog during fast touchpad scrolling. Events buffer in
        // RustConnection while the channel is full, then trickle through 5 at a time.
        // See: smithay/src/utils/x11rb.rs line 53
        //
        // CLEAN FIX: Fork Smithay and change sync_channel(5) to channel() (unbounded)
        // in src/utils/x11rb.rs. This allows all buffered events to drain immediately.
        //
        // This workaround uses small timeouts between drain attempts to give the event
        // thread time to refill the channel from RustConnection's buffer.
        let drain_start = Instant::now();
        let drain_max = Duration::from_millis(8);
        let mut consecutive_empty = 0;

        loop {
            // Small timeout lets event thread refill channel from RustConnection buffer
            event_loop
                .dispatch(Some(Duration::from_micros(200)), &mut compositor)
                .expect("event loop dispatch failed");

            let mut batch_count = 0;
            while let Ok(input_event) = x11_event_rx.try_recv() {
                batch_count += 1;
                compositor.process_input_event_with_terminals(input_event, &mut terminal_manager);
            }

            if batch_count == 0 {
                consecutive_empty += 1;
                // Break after 3 consecutive empty checks (~600µs of no events)
                if consecutive_empty >= 3 {
                    break;
                }
            } else {
                consecutive_empty = 0;
            }

            // Hard timeout safety
            if drain_start.elapsed() >= drain_max {
                break;
            }
        }

        // Apply accumulated scroll delta (once per frame)
        compositor.apply_pending_scroll();

        // Process pending PRIMARY selection paste (from middle-click)
        compositor.process_primary_selection_paste(&mut terminal_manager);

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
            crate::window_height::handle_compositor_resize(
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
            crate::window_height::calculate_window_heights,
        );

        // Handle command spawn requests from IPC (termstack)
        crate::spawn_handler::handle_ipc_spawn_requests(
            &mut compositor,
            &mut terminal_manager,
            crate::window_height::calculate_window_heights,
        );

        // Handle GUI spawn requests from IPC (termstack gui)
        crate::spawn_handler::handle_gui_spawn_requests(
            &mut compositor,
            &mut terminal_manager,
            crate::window_height::calculate_window_heights,
        );

        // Handle builtin command requests from IPC (termstack --builtin)
        crate::spawn_handler::handle_builtin_requests(
            &mut compositor,
            &mut terminal_manager,
            crate::window_height::calculate_window_heights,
        );

        // Handle resize requests from IPC (termstack --resize)
        crate::terminal_output::handle_ipc_resize_request(&mut compositor, &mut terminal_manager);

        // Handle key repeat for terminals
        crate::input_handler::handle_key_repeat(&mut compositor, &mut terminal_manager);

        // Handle focus change requests from input (scroll is applied immediately)
        crate::input_handler::handle_focus_change_requests(&mut compositor, &mut terminal_manager);

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

        // Frame rate limiting: wait until it's time to render
        // This prevents busy-looping while still processing events at a steady rate
        let now = Instant::now();
        let elapsed = now.duration_since(last_render_time);
        if elapsed < MIN_FRAME_TIME {
            // Wait for remaining time, then process any events that arrived
            let remaining = MIN_FRAME_TIME - elapsed;
            event_loop
                .dispatch(Some(remaining), &mut compositor)
                .expect("event loop dispatch failed");
            // Process events that arrived during the wait
            while let Ok(input_event) = x11_event_rx.try_recv() {
                compositor.process_input_event_with_terminals(input_event, &mut terminal_manager);
            }
            compositor.apply_pending_scroll();
            continue;
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
                        StackWindow::External(_) => {
                            if is_resizing {
                                if let Some(drag) = &compositor.resizing {
                                    // Being resized: use drag target for visual feedback
                                    return drag.target_height;
                                }
                            }
                            // Not being resized: use actual_heights from element geometry
                            // This handles both new windows (before first commit) and
                            // post-commit windows correctly.
                            actual_heights[i]
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
            crate::window_height::check_and_handle_height_changes(&mut compositor, actual_heights);

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

                    // Use source rectangle directly - Smithay handles coordinate systems
                    element.draw(&mut frame, src, dest, &[damage], &[]).ok();
                }
            }
        }

        // Submit the rendered buffer to X11
        if let Err(e) = x11_surface.submit() {
            tracing::warn!(error = ?e, "Failed to submit X11 surface");
        }

        // Update render timestamp for frame rate limiting
        last_render_time = now;

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