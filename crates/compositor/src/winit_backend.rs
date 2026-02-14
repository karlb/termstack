//! macOS display backend using winit + softbuffer
//!
//! Provides a full compositor experience on macOS by rendering terminals
//! and external Wayland client windows to a winit window via softbuffer
//! (CPU-based pixel presentation).

use std::os::unix::net::UnixListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::{EventLoop, generic::Generic, Interest, Mode as CalloopMode};
use smithay::reexports::wayland_server::{Display, Resource};
use smithay::utils::{Physical, Size, Transform};
use smithay::wayland::compositor::{
    with_surface_tree_downward, SubsurfaceCachedState, TraversalAction,
};
use smithay::wayland::shm::with_buffer_contents;
use smithay::wayland::socket::ListeningSocketSource;
use smithay::desktop::utils::send_frames_surface_tree;

use softbuffer::Surface;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop as WinitEventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::config::Config;
use crate::coords::ScreenY;
use crate::state::{ClientState, FocusedWindow, StackWindow, TermStack};
use crate::terminal_manager::TerminalManager;
use crate::title_bar::{TitleBarRenderer, CLOSE_BUTTON_WIDTH, TITLE_BAR_HEIGHT};

/// Minimum time between frames (~120 FPS cap)
const MIN_FRAME_TIME: Duration = Duration::from_millis(8);

/// Run the compositor with the winit backend (macOS)
pub fn run_compositor_winit() -> anyhow::Result<()> {
    tracing::info!("starting termstack with winit backend (macOS)");

    let event_loop = WinitEventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);

    let mut app = App {
        window: None,
        surface: None,
        compositor: None,
        display: None,
        calloop: None,
        terminal_manager: None,
        output: None,
        config: Config::load(),
        modifiers: ModifiersState::empty(),
        cursor_position: (0.0, 0.0),
        title_bar_renderer: None,
        last_render_time: Instant::now(),
    };

    event_loop.run_app(&mut app)?;

    Ok(())
}

struct App {
    // winit/softbuffer
    window: Option<Arc<Window>>,
    surface: Option<Surface<Arc<Window>, Arc<Window>>>,

    // Compositor state
    compositor: Option<TermStack>,
    display: Option<Display<TermStack>>,
    calloop: Option<EventLoop<'static, TermStack>>,
    terminal_manager: Option<TerminalManager>,
    output: Option<Output>,
    config: Config,
    title_bar_renderer: Option<TitleBarRenderer>,

    // Input state
    modifiers: ModifiersState,
    cursor_position: (f64, f64),

    // Frame timing
    last_render_time: Instant,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return; // Already initialized
        }

        // Ensure XDG_RUNTIME_DIR is set — Smithay's Wayland socket creation
        // and keyboard keymap file creation both require it.
        // On macOS this isn't set by default, so use the system temp directory.
        if std::env::var("XDG_RUNTIME_DIR").is_err() {
            let runtime_dir = std::env::temp_dir().join(format!("termstack-runtime-{}", std::process::id()));
            std::fs::create_dir_all(&runtime_dir).expect("failed to create runtime dir");
            std::env::set_var("XDG_RUNTIME_DIR", &runtime_dir);
            tracing::info!(?runtime_dir, "set XDG_RUNTIME_DIR for Smithay");
        }

        // Create winit window
        let attrs = WindowAttributes::default()
            .with_title("TermStack")
            .with_inner_size(winit::dpi::LogicalSize::new(1280u32, 800u32));

        let window = Arc::new(event_loop.create_window(attrs).expect("failed to create window"));

        // Create softbuffer context and surface
        let context = softbuffer::Context::new(window.clone()).expect("failed to create softbuffer context");
        let surface = Surface::new(&context, window.clone()).expect("failed to create softbuffer surface");

        let inner_size = window.inner_size();
        let output_width = inner_size.width;
        let output_height = inner_size.height;

        self.window = Some(window);
        self.surface = Some(surface);

        // Create calloop event loop
        let calloop: EventLoop<TermStack> = EventLoop::try_new().expect("failed to create calloop event loop");

        // Create Wayland display
        let display: Display<TermStack> = Display::new().expect("failed to create Wayland display");

        // Create output
        let mode = Mode {
            size: (output_width as i32, output_height as i32).into(),
            refresh: 60_000,
        };

        let output = Output::new(
            "winit".to_string(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "TermStack".to_string(),
                model: "Winit".to_string(),
            },
        );
        output.change_current_state(Some(mode), Some(Transform::Normal), None, Some((0, 0).into()));
        output.set_preferred(mode);

        let output_size: Size<i32, Physical> = Size::from((output_width as i32, output_height as i32));

        // Create compositor state
        let (mut compositor, display) = TermStack::new(
            display,
            calloop.handle(),
            output_size,
            self.config.csd_apps.clone(),
            self.config.max_gui_windows,
        );

        // Add output to compositor
        compositor.space.map_output(&output, (0, 0));
        let _output_global = output.create_global::<TermStack>(&compositor.display_handle);

        // Create listening socket for Wayland clients
        let listening_socket = ListeningSocketSource::new_auto()
            .expect("failed to create Wayland socket");

        let socket_name = listening_socket
            .socket_name()
            .to_string_lossy()
            .to_string();

        tracing::info!(?socket_name, "winit compositor listening on Wayland socket");

        // Set environment variables for child processes
        std::env::set_var("WAYLAND_DISPLAY", &socket_name);

        // Insert socket source into calloop
        calloop.handle().insert_source(listening_socket, |client_stream, _, state| {
            tracing::info!("new Wayland client connected (winit)");
            if let Err(e) = state.display_handle.insert_client(client_stream, Arc::new(ClientState {
                compositor_state: Default::default(),
            })) {
                tracing::error!(error = ?e, "Failed to insert Wayland client");
            }
        }).expect("failed to insert Wayland socket source");

        // Create IPC socket
        let ipc_socket_path = crate::ipc::socket_path();
        let _ = std::fs::remove_file(&ipc_socket_path);
        let ipc_listener = UnixListener::bind(&ipc_socket_path)
            .expect("failed to create IPC socket");
        ipc_listener.set_nonblocking(true).expect("failed to set IPC socket nonblocking");

        std::env::set_var("TERMSTACK_SOCKET", &ipc_socket_path);

        let binary_path = std::env::current_exe()
            .unwrap_or_else(|e| {
                tracing::warn!(error = ?e, "Failed to determine binary path");
                std::path::PathBuf::from("termstack")
            });
        std::env::set_var("TERMSTACK_BIN", &binary_path);

        tracing::info!(
            path = ?ipc_socket_path,
            binary = ?binary_path,
            "winit IPC socket created"
        );

        // Insert IPC socket source into calloop
        calloop.handle().insert_source(
            Generic::new(ipc_listener, Interest::READ, CalloopMode::Level),
            |_, listener, state| {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            tracing::debug!("IPC connection received (winit)");
                            match crate::ipc::read_ipc_request(stream) {
                                Ok((request, stream)) => {
                                    match request {
                                        crate::ipc::IpcRequest::Spawn(spawn_req) => {
                                            if spawn_req.foreground.is_some() {
                                                state.pending_gui_spawn_requests.push(spawn_req);
                                            } else {
                                                state.pending_spawn_requests.push(spawn_req);
                                            }
                                        }
                                        crate::ipc::IpcRequest::Resize(mode) => {
                                            state.pending_resize_request = Some((mode, stream));
                                        }
                                        crate::ipc::IpcRequest::Builtin(builtin_req) => {
                                            state.pending_builtin_requests.push(builtin_req);
                                        }
                                        crate::ipc::IpcRequest::QueryWindows => {
                                            let windows: Vec<crate::ipc::WindowInfo> = state.layout_nodes
                                                .iter()
                                                .enumerate()
                                                .map(|(i, node)| {
                                                    let (is_external, command) = match &node.cell {
                                                        StackWindow::Terminal(_) => (false, String::new()),
                                                        StackWindow::External(entry) => (true, entry.command.clone()),
                                                    };
                                                    crate::ipc::WindowInfo {
                                                        index: i,
                                                        width: state.output_size.w,
                                                        height: node.height,
                                                        is_external,
                                                        command,
                                                    }
                                                })
                                                .collect();
                                            if let Err(e) = crate::ipc::send_json_response(stream, &windows) {
                                                tracing::warn!(error = ?e, "Failed to send query_windows response");
                                            }
                                        }
                                    }
                                }
                                Err(crate::ipc::IpcError::Timeout) => {}
                                Err(crate::ipc::IpcError::EmptyMessage) => {}
                                Err(e) => {
                                    tracing::warn!(error = ?e, "IPC request parsing failed (winit)");
                                }
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(e) => {
                            tracing::warn!(error = ?e, "IPC accept error (winit)");
                            break;
                        }
                    }
                }
                Ok(smithay::reexports::calloop::PostAction::Continue)
            },
        ).expect("failed to insert IPC socket source");

        // Create terminal manager
        let terminal_theme = self.config.theme.to_terminal_theme();
        let mut terminal_manager = TerminalManager::new_with_size(
            output_width,
            output_height,
            terminal_theme,
            self.config.font_size,
        );
        terminal_manager.set_max_terminals(self.config.max_terminals);
        terminal_manager.set_max_dead_terminals(self.config.max_dead_terminals);
        terminal_manager.set_dead_terminal_ttl(Duration::from_secs(self.config.dead_terminal_ttl_minutes * 60));

        // Create title bar renderer
        self.title_bar_renderer = TitleBarRenderer::new(terminal_theme);
        if self.title_bar_renderer.is_none() {
            tracing::warn!("Title bar renderer unavailable - no font found");
        }

        // Spawn initial terminal
        match terminal_manager.spawn() {
            Ok(id) => {
                compositor.add_terminal(id);
                compositor.enforce_terminal_limit(&mut terminal_manager);
                tracing::info!(id = id.0, "spawned initial terminal (winit)");
            }
            Err(e) => {
                tracing::error!(error = ?e, "failed to spawn initial terminal (winit)");
            }
        }

        self.compositor = Some(compositor);
        self.display = Some(display);
        self.calloop = Some(calloop);
        self.terminal_manager = Some(terminal_manager);
        self.output = Some(output);

        tracing::info!("winit compositor initialized");
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _window_id: WindowId, event: WindowEvent) {
        let Some(compositor) = &mut self.compositor else { return };
        let Some(terminal_manager) = &mut self.terminal_manager else { return };

        match event {
            WindowEvent::CloseRequested => {
                compositor.running = false;
                event_loop.exit();
            }

            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                let ctrl = self.modifiers.control_key();
                let shift = self.modifiers.shift_key();

                // Compositor keybindings (Ctrl+Shift+...)
                if ctrl && shift {
                    match &event.logical_key {
                        Key::Character(s) => match s.as_str() {
                            "q" | "Q" => {
                                compositor.running = false;
                                event_loop.exit();
                                return;
                            }
                            "j" | "J" => {
                                compositor.focus_change_requested = 1;
                                return;
                            }
                            "k" | "K" => {
                                compositor.focus_change_requested = -1;
                                return;
                            }
                            _ => {}
                        },
                        Key::Named(NamedKey::Enter) => {
                            compositor.spawn_terminal_requested = true;
                            return;
                        }
                        _ => {}
                    }

                    // Font size change
                    match &event.logical_key {
                        Key::Character(s) if s == "+" || s == "=" => {
                            compositor.pending_font_size_delta = 1.0;
                            return;
                        }
                        Key::Character(s) if s == "-" || s == "_" => {
                            compositor.pending_font_size_delta = -1.0;
                            return;
                        }
                        _ => {}
                    }

                    // Consume unmatched Ctrl+Shift combos so they don't leak to the terminal
                    return;
                }

                // Send key to focused terminal
                let bytes = winit_key_to_bytes(&event.logical_key, ctrl);
                if !bytes.is_empty() {
                    if let Some(terminal) = terminal_manager.get_focused_mut(compositor.focused_window.as_ref()) {
                        if !terminal.has_exited() {
                            if let Err(e) = terminal.write(&bytes) {
                                tracing::error!(?e, "failed to write to terminal");
                            }
                        }
                    }
                }
            }

            WindowEvent::Resized(new_size) => {
                if new_size.width == 0 || new_size.height == 0 {
                    return;
                }
                let new_output_size = Size::from((new_size.width as i32, new_size.height as i32));
                crate::window_height::handle_compositor_resize(
                    compositor,
                    terminal_manager,
                    new_output_size,
                );
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = (position.x, position.y);

                // Handle resize drag motion
                if let Some(ref mut drag) = compositor.resizing {
                    let screen_y = position.y as i32;
                    let delta = screen_y - drag.start_screen_y;
                    let new_height = (drag.start_height + delta).max(crate::state::MIN_WINDOW_HEIGHT);
                    drag.target_height = new_height;

                    if let Some(node) = compositor.layout_nodes.get_mut(drag.window_index) {
                        node.height = new_height;
                        // Resize terminal to match drag
                        if let StackWindow::Terminal(tid) = node.cell {
                            let cell_height = terminal_manager.cell_height;
                            if let Some(term) = terminal_manager.get_mut(tid) {
                                let content_height = new_height - if term.show_title_bar { TITLE_BAR_HEIGHT as i32 } else { 0 };
                                let new_rows = (content_height as u32 / cell_height).max(1) as u16;
                                term.resize(new_rows, cell_height);
                                term.manually_sized = true;
                            }
                        }
                    }
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                let screen_y = self.cursor_position.1;

                match state {
                    ElementState::Pressed => {
                        if button == MouseButton::Left {
                            // Check for resize handle first
                            if let Some(handle_idx) = compositor.find_resize_handle_at(ScreenY::new(screen_y)) {
                                let node = &compositor.layout_nodes[handle_idx];
                                let identity = match &node.cell {
                                    StackWindow::Terminal(id) => FocusedWindow::Terminal(*id),
                                    StackWindow::External(e) => {
                                        FocusedWindow::External(e.surface.wl_surface().id())
                                    }
                                };
                                compositor.resizing = Some(crate::state::ResizeDrag {
                                    window_index: handle_idx,
                                    window_identity: identity,
                                    start_screen_y: screen_y as i32,
                                    start_height: node.height,
                                    target_height: node.height,
                                    last_configure_time: std::time::Instant::now(),
                                    last_sent_height: None,
                                });
                                return;
                            }

                            // Click-to-focus: find window at click position
                            if let Some(index) = compositor.window_at_screen_y(ScreenY::new(screen_y)) {
                                // Check for close button click on title bar
                                if let StackWindow::Terminal(tid) = compositor.layout_nodes[index].cell {
                                    if let Some(term) = terminal_manager.get(tid) {
                                        if term.show_title_bar {
                                            // Compute window top Y from layout
                                            let window_top: i32 = compositor.layout_nodes[..index]
                                                .iter()
                                                .map(|n| n.height)
                                                .sum::<i32>()
                                                - compositor.scroll_offset as i32;
                                            let click_in_title_bar = (screen_y as i32) < window_top + TITLE_BAR_HEIGHT as i32;
                                            let click_in_close_zone = self.cursor_position.0 >= (compositor.output_size.w as u32 - CLOSE_BUTTON_WIDTH) as f64;

                                            if click_in_title_bar && click_in_close_zone {
                                                compositor.layout_nodes.remove(index);
                                                compositor.invalidate_focused_index_cache();
                                                terminal_manager.remove(tid);
                                                compositor.update_focus_after_removal(index);
                                                return;
                                            }
                                        }
                                    }
                                }

                                compositor.set_focus_by_index(index);

                                // Scroll to show focused window
                                compositor.scroll_to_show_window_bottom(index);
                            }
                        }
                    }
                    ElementState::Released => {
                        if button == MouseButton::Left {
                            if let Some(drag) = compositor.resizing.take() {
                                // Mark terminal dirty for re-render
                                if let Some(node) = compositor.layout_nodes.get(drag.window_index) {
                                    if let StackWindow::Terminal(tid) = node.cell {
                                        if let Some(term) = terminal_manager.get_mut(tid) {
                                            term.mark_dirty();
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let pixels = match delta {
                    MouseScrollDelta::PixelDelta(pos) => pos.y,
                    MouseScrollDelta::LineDelta(_, lines) => lines as f64 * 100.0,
                };

                if self.modifiers.shift_key() {
                    // Shift+Scroll: terminal scrollback
                    let lines = match delta {
                        MouseScrollDelta::LineDelta(_, lines) => (lines * 3.0) as i32,
                        MouseScrollDelta::PixelDelta(pos) => (pos.y / 5.0) as i32,
                    };
                    if lines != 0 {
                        if let Some(index) = compositor.window_at_screen_y(ScreenY::new(self.cursor_position.1)) {
                            if let StackWindow::Terminal(tid) = compositor.layout_nodes[index].cell {
                                if let Some(term) = terminal_manager.get_mut(tid) {
                                    term.terminal.scroll_display(lines);
                                    term.mark_dirty();
                                }
                            }
                        }
                    }
                } else {
                    // Negate: winit positive = scroll up gesture, compositor positive = scroll down
                    compositor.pending_scroll_delta -= pixels;
                }
            }

            WindowEvent::RedrawRequested => {
                self.render_frame();
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        let Some(compositor) = &mut self.compositor else { return };
        let Some(display) = &mut self.display else { return };
        let Some(calloop) = &mut self.calloop else { return };
        let Some(terminal_manager) = &mut self.terminal_manager else { return };
        let Some(output) = &self.output else { return };

        if !compositor.running {
            return;
        }

        // 1. Dispatch calloop events (IPC + Wayland socket) - non-blocking
        calloop.dispatch(Some(Duration::ZERO), compositor)
            .expect("calloop dispatch failed");

        // 2. Dispatch Wayland clients
        display.dispatch_clients(compositor)
            .expect("failed to dispatch clients");

        // 3. Handle focus change requests
        crate::input_handler::handle_focus_change_requests(compositor, terminal_manager);

        // 4. Handle external window events
        crate::window_lifecycle::handle_external_window_events(compositor);

        // 5. Handle IPC spawn requests
        crate::spawn_handler::handle_ipc_spawn_requests(
            compositor,
            terminal_manager,
            crate::window_height::calculate_window_heights,
        );

        // 6. Handle GUI spawn requests
        crate::spawn_handler::handle_gui_spawn_requests(
            compositor,
            terminal_manager,
            crate::window_height::calculate_window_heights,
        );

        // 7. Handle builtin requests
        crate::spawn_handler::handle_builtin_requests(
            compositor,
            terminal_manager,
            crate::window_height::calculate_window_heights,
        );

        // 8. Handle resize requests from IPC
        crate::terminal_output::handle_ipc_resize_request(compositor, terminal_manager);

        // 9. Process terminal PTY output
        crate::terminal_output::process_terminal_output(compositor, terminal_manager);

        // 10. Promote output terminals
        crate::terminal_output::promote_output_terminals(compositor, terminal_manager);

        // 11. Handle output terminal cleanup
        crate::window_lifecycle::handle_output_terminal_cleanup(compositor, terminal_manager);

        // 12. Handle launcher restoration
        crate::window_lifecycle::handle_launcher_restoration(compositor, terminal_manager);

        // 13. Cleanup dead terminals
        if crate::window_lifecycle::cleanup_and_sync_focus(compositor, terminal_manager) {
            // All terminals gone - exit
            compositor.running = false;
        }

        // 14. Handle terminal spawn requests (Ctrl+Shift+Enter)
        crate::window_lifecycle::handle_terminal_spawn(
            compositor,
            terminal_manager,
            crate::window_height::calculate_window_heights,
        );

        // 15. Handle font size changes
        if compositor.pending_font_size_delta != 0.0 {
            let delta = compositor.pending_font_size_delta;
            compositor.pending_font_size_delta = 0.0;
            let new_size = (terminal_manager.font_size() + delta).clamp(6.0, 72.0);
            terminal_manager.set_font_size(
                new_size,
                compositor.output_size.w as u32,
                compositor.output_size.h as u32,
            );
            tracing::info!(new_size, "font size changed (winit)");
        }

        // 16. Apply accumulated scroll delta
        compositor.apply_pending_scroll();

        // 17. Calculate and update window heights
        let window_heights = crate::window_height::calculate_window_heights(compositor, terminal_manager);
        crate::window_height::check_and_handle_height_changes(compositor, window_heights);
        compositor.recalculate_layout();

        // 17. Send frame callbacks to Wayland clients
        for surface in compositor.xdg_shell_state.toplevel_surfaces() {
            send_frames_surface_tree(
                surface.wl_surface(),
                output,
                Duration::ZERO,
                Some(Duration::ZERO),
                |_, _| Some(output.clone()),
            );
        }

        // 18. Flush clients
        if let Err(e) = compositor.display_handle.flush_clients() {
            tracing::warn!(error = ?e, "failed to flush Wayland clients");
        }

        // 19. Process pending clipboard operations
        compositor.process_primary_selection_paste(terminal_manager);
        compositor.timeout_stale_clipboard_reads();
        compositor.timeout_stale_pending_window();

        // 20. Request redraw (throttled to avoid burning CPU)
        if self.last_render_time.elapsed() >= MIN_FRAME_TIME {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }
}

impl App {
    fn render_frame(&mut self) {
        let Some(compositor) = &mut self.compositor else { return };
        let Some(terminal_manager) = &mut self.terminal_manager else { return };
        let Some(surface) = &mut self.surface else { return };
        let Some(window) = &self.window else { return };

        let inner_size = window.inner_size();
        let width = inner_size.width;
        let height = inner_size.height;

        if width == 0 || height == 0 {
            return;
        }

        // Resize softbuffer surface
        if let Err(e) = surface.resize(
            std::num::NonZeroU32::new(width).unwrap(),
            std::num::NonZeroU32::new(height).unwrap(),
        ) {
            tracing::warn!(error = ?e, "failed to resize softbuffer surface");
            return;
        }

        let mut buffer = match surface.buffer_mut() {
            Ok(buf) => buf,
            Err(e) => {
                tracing::warn!(error = ?e, "failed to get softbuffer buffer");
                return;
            }
        };

        // Clear with background color
        let bg_color = if self.config.theme == crate::config::Theme::Dark {
            0x001A1A1A // Dark background (no alpha for softbuffer)
        } else {
            0x00FFFFFF
        };
        buffer.fill(bg_color);

        // Render each visible terminal
        let focused_index = compositor.focused_index();
        let mut content_y: i32 = -(compositor.scroll_offset as i32);

        for (i, node) in compositor.layout_nodes.iter().enumerate() {
            let window_height = node.height;
            if window_height <= 0 {
                continue;
            }

            let is_focused = focused_index == Some(i);

            // Skip if entirely off-screen
            if content_y >= height as i32 || content_y + window_height <= 0 {
                content_y += window_height;
                continue;
            }

            match &node.cell {
                StackWindow::Terminal(tid) => {
                    if let Some(terminal) = terminal_manager.get_mut(*tid) {
                        if !terminal.is_visible() {
                            content_y += window_height;
                            continue;
                        }

                        // Render title bar if applicable
                        let title_bar_y = content_y;
                        let mut terminal_content_y = content_y;

                        if terminal.show_title_bar {
                            if let Some(ref mut tb_renderer) = self.title_bar_renderer {
                                let title = &terminal.title;
                                let (tb_pixels, _tb_w, tb_h) = tb_renderer.render(title, width);

                                // Blit title bar (BGRA bytes → softbuffer u32 pixels)
                                blit_bgra_to_surface(
                                    &tb_pixels,
                                    width,
                                    tb_h,
                                    &mut buffer,
                                    width,
                                    height,
                                    0,
                                    title_bar_y,
                                );
                            }
                            terminal_content_y += TITLE_BAR_HEIGHT as i32;
                        }

                        // Render terminal content
                        let content_height = window_height - if terminal.show_title_bar { TITLE_BAR_HEIGHT as i32 } else { 0 };
                        if content_height > 0 {
                            terminal.terminal.render(
                                terminal.width,
                                content_height as u32,
                                !terminal.has_exited(),
                            );
                            let term_buffer = terminal.terminal.buffer();

                            // Blit terminal (ARGB u32 → softbuffer u32 pixels, strip alpha)
                            blit_argb_to_surface(
                                term_buffer,
                                terminal.width,
                                content_height as u32,
                                &mut buffer,
                                width,
                                height,
                                0,
                                terminal_content_y,
                            );
                        }

                        // Draw focus indicator
                        if is_focused {
                            draw_focus_indicator(
                                &mut buffer,
                                width,
                                height,
                                content_y,
                                window_height,
                            );
                        }
                    }
                }
                StackWindow::External(entry) => {
                    let wl_surface = entry.surface.wl_surface();

                    // Render title bar for SSD windows
                    let mut window_content_y = content_y;
                    if !entry.uses_csd {
                        if let Some(ref mut tb_renderer) = self.title_bar_renderer {
                            let (tb_pixels, _tb_w, tb_h) =
                                tb_renderer.render(&entry.command, width);
                            blit_bgra_to_surface(
                                &tb_pixels,
                                width,
                                tb_h,
                                &mut buffer,
                                width,
                                height,
                                0,
                                content_y,
                            );
                        }
                        window_content_y += TITLE_BAR_HEIGHT as i32;
                    }

                    // Blit the Wayland surface tree
                    blit_surface_tree(
                        wl_surface,
                        &mut buffer,
                        width,
                        height,
                        0,
                        window_content_y,
                    );

                    // Draw focus indicator
                    if is_focused {
                        draw_focus_indicator(
                            &mut buffer,
                            width,
                            height,
                            content_y,
                            window_height,
                        );
                    }
                }
            }

            content_y += window_height;
        }

        // Present the frame
        if let Err(e) = buffer.present() {
            tracing::warn!(error = ?e, "failed to present softbuffer frame");
        }

        self.last_render_time = Instant::now();
    }
}

/// Blit BGRA byte buffer onto softbuffer surface at given position
#[allow(clippy::too_many_arguments)]
fn blit_bgra_to_surface(
    src: &[u8],
    src_width: u32,
    src_height: u32,
    dst: &mut [u32],
    dst_width: u32,
    dst_height: u32,
    dst_x: i32,
    dst_y: i32,
) {
    for row in 0..src_height {
        let screen_y = dst_y + row as i32;
        if screen_y < 0 || screen_y >= dst_height as i32 {
            continue;
        }

        for col in 0..src_width {
            let screen_x = dst_x + col as i32;
            if screen_x < 0 || screen_x >= dst_width as i32 {
                continue;
            }

            let src_idx = ((row * src_width + col) * 4) as usize;
            if src_idx + 3 >= src.len() {
                continue;
            }

            let b = src[src_idx] as u32;
            let g = src[src_idx + 1] as u32;
            let r = src[src_idx + 2] as u32;
            // Strip alpha for softbuffer (expects 0x00RRGGBB)
            let pixel = (r << 16) | (g << 8) | b;

            let dst_idx = (screen_y as u32 * dst_width + screen_x as u32) as usize;
            if dst_idx < dst.len() {
                dst[dst_idx] = pixel;
            }
        }
    }
}

/// Blit ARGB u32 buffer onto softbuffer surface at given position
#[allow(clippy::too_many_arguments)]
fn blit_argb_to_surface(
    src: &[u32],
    src_width: u32,
    src_height: u32,
    dst: &mut [u32],
    dst_width: u32,
    dst_height: u32,
    dst_x: i32,
    dst_y: i32,
) {
    for row in 0..src_height {
        let screen_y = dst_y + row as i32;
        if screen_y < 0 || screen_y >= dst_height as i32 {
            continue;
        }

        let src_row_start = (row * src_width) as usize;
        let dst_row_start = screen_y as usize * dst_width as usize;

        // Calculate visible column range
        let col_start = if dst_x < 0 { (-dst_x) as u32 } else { 0 };
        let col_end = src_width.min((dst_width as i32 - dst_x).max(0) as u32);

        for col in col_start..col_end {
            let src_idx = src_row_start + col as usize;
            if src_idx >= src.len() {
                break;
            }

            let pixel = src[src_idx];
            // Strip alpha: ARGB → 0x00RRGGBB
            let out = pixel & 0x00FFFFFF;

            let dst_idx = dst_row_start + (dst_x + col as i32) as usize;
            if dst_idx < dst.len() {
                dst[dst_idx] = out;
            }
        }
    }
}

/// Draw a green focus indicator on the left edge
fn draw_focus_indicator(
    buffer: &mut [u32],
    buf_width: u32,
    buf_height: u32,
    y: i32,
    height: i32,
) {
    let indicator_width = crate::layout::FOCUS_INDICATOR_WIDTH;
    let green = 0x0000CC00; // Green, no alpha

    for row in 0..height {
        let screen_y = y + row;
        if screen_y < 0 || screen_y >= buf_height as i32 {
            continue;
        }
        for col in 0..indicator_width {
            if col >= buf_width as i32 {
                break;
            }
            let idx = screen_y as usize * buf_width as usize + col as usize;
            if idx < buffer.len() {
                buffer[idx] = green;
            }
        }
    }
}

/// Blit a single Wayland SHM surface's buffer onto the softbuffer framebuffer.
///
/// Takes `&SurfaceData` directly (instead of `&WlSurface`) to avoid deadlocking
/// when called from inside `with_surface_tree_downward`, which already holds the
/// surface data lock.
#[allow(clippy::too_many_arguments)]
fn blit_surface_from_data(
    surface_data: &smithay::wayland::compositor::SurfaceData,
    dst: &mut [u32],
    dst_width: u32,
    dst_height: u32,
    dst_x: i32,
    dst_y: i32,
) {
    let Some(buffer) = surface_data
        .data_map
        .get::<RendererSurfaceStateUserData>()
        .and_then(|state| {
            let guard = state.lock().ok()?;
            guard.buffer().cloned()
        })
    else {
        return;
    };

    let _ = with_buffer_contents(&buffer, |ptr, len, data| {
        let src_width = data.width as u32;
        let src_height = data.height as u32;
        let stride = data.stride as usize;

        for row in 0..src_height {
            let screen_y = dst_y + row as i32;
            if screen_y < 0 || screen_y >= dst_height as i32 {
                continue;
            }

            let src_row_offset = row as usize * stride;
            let dst_row_start = screen_y as usize * dst_width as usize;

            for col in 0..src_width {
                let screen_x = dst_x + col as i32;
                if screen_x < 0 || screen_x >= dst_width as i32 {
                    continue;
                }

                let src_byte_offset = src_row_offset + col as usize * 4;
                if src_byte_offset + 3 >= len {
                    break;
                }

                // SAFETY: we checked bounds above and ptr points to the SHM pool
                let pixel = unsafe {
                    (ptr.add(src_byte_offset) as *const u32).read_unaligned()
                };
                // ARGB8888 → 0x00RRGGBB for softbuffer
                let dst_idx = dst_row_start + screen_x as usize;
                if dst_idx < dst.len() {
                    dst[dst_idx] = pixel & 0x00FFFFFF;
                }
            }
        }
    });
}

/// Walk the surface tree of a Wayland surface and blit each surface at its
/// accumulated position (handling subsurface offsets).
fn blit_surface_tree(
    wl_surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    dst: &mut [u32],
    dst_width: u32,
    dst_height: u32,
    base_x: i32,
    base_y: i32,
) {
    with_surface_tree_downward(
        wl_surface,
        (base_x, base_y),
        |_surface, states, &(x, y)| {
            let offset = states
                .cached_state
                .get::<SubsurfaceCachedState>()
                .current()
                .location;
            TraversalAction::DoChildren((x + offset.x, y + offset.y))
        },
        |_surface, states, &(x, y)| {
            blit_surface_from_data(states, dst, dst_width, dst_height, x, y);
        },
        |_, _, _| true,
    );
}

/// Convert a winit key event to terminal bytes
fn winit_key_to_bytes(key: &Key, ctrl: bool) -> Vec<u8> {
    // Handle control characters
    if ctrl {
        if let Key::Character(s) = key {
            let c = s.chars().next().unwrap_or('\0');
            let code = match c.to_ascii_lowercase() {
                'a' => Some(1),
                'b' => Some(2),
                'c' => Some(3),
                'd' => Some(4),
                'e' => Some(5),
                'f' => Some(6),
                'g' => Some(7),
                'h' => Some(8),
                'i' => Some(9),
                'j' => Some(10),
                'k' => Some(11),
                'l' => Some(12),
                'm' => Some(13),
                'n' => Some(14),
                'o' => Some(15),
                'p' => Some(16),
                'q' => Some(17),
                'r' => Some(18),
                's' => Some(19),
                't' => Some(20),
                'u' => Some(21),
                'v' => Some(22),
                'w' => Some(23),
                'x' => Some(24),
                'y' => Some(25),
                'z' => Some(26),
                '[' => Some(27),
                '\\' => Some(28),
                ']' => Some(29),
                '^' => Some(30),
                '_' => Some(31),
                _ => None,
            };
            if let Some(byte) = code {
                return vec![byte];
            }
        }
    }

    match key {
        Key::Character(s) => s.as_bytes().to_vec(),

        Key::Named(named) => match named {
            NamedKey::Enter => vec![b'\r'],
            NamedKey::Backspace => vec![0x7f],
            NamedKey::Tab => vec![b'\t'],
            NamedKey::Escape => vec![0x1b],
            NamedKey::Space => vec![b' '],

            // Arrow keys
            NamedKey::ArrowUp => vec![0x1b, b'[', b'A'],
            NamedKey::ArrowDown => vec![0x1b, b'[', b'B'],
            NamedKey::ArrowRight => vec![0x1b, b'[', b'C'],
            NamedKey::ArrowLeft => vec![0x1b, b'[', b'D'],

            // Home/End
            NamedKey::Home => vec![0x1b, b'[', b'H'],
            NamedKey::End => vec![0x1b, b'[', b'F'],

            // Page Up/Down
            NamedKey::PageUp => vec![0x1b, b'[', b'5', b'~'],
            NamedKey::PageDown => vec![0x1b, b'[', b'6', b'~'],

            // Insert/Delete
            NamedKey::Insert => vec![0x1b, b'[', b'2', b'~'],
            NamedKey::Delete => vec![0x1b, b'[', b'3', b'~'],

            // Function keys
            NamedKey::F1 => vec![0x1b, b'O', b'P'],
            NamedKey::F2 => vec![0x1b, b'O', b'Q'],
            NamedKey::F3 => vec![0x1b, b'O', b'R'],
            NamedKey::F4 => vec![0x1b, b'O', b'S'],
            NamedKey::F5 => vec![0x1b, b'[', b'1', b'5', b'~'],
            NamedKey::F6 => vec![0x1b, b'[', b'1', b'7', b'~'],
            NamedKey::F7 => vec![0x1b, b'[', b'1', b'8', b'~'],
            NamedKey::F8 => vec![0x1b, b'[', b'1', b'9', b'~'],
            NamedKey::F9 => vec![0x1b, b'[', b'2', b'0', b'~'],
            NamedKey::F10 => vec![0x1b, b'[', b'2', b'1', b'~'],
            NamedKey::F11 => vec![0x1b, b'[', b'2', b'3', b'~'],
            NamedKey::F12 => vec![0x1b, b'[', b'2', b'4', b'~'],

            _ => vec![],
        },

        _ => vec![],
    }
}

