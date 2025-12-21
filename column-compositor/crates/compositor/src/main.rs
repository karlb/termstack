//! Column Compositor - A content-aware terminal compositor
//!
//! This compositor arranges terminal windows in a scrollable vertical column,
//! with windows dynamically sizing based on their content.

mod config;
mod input;
mod layout;
mod state;
mod terminal_manager;

use std::time::Duration;

use smithay::backend::winit::{self, WinitEvent};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{Color32F, Frame, Renderer, Texture};
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{AsRenderElements, Element, RenderElement};
use smithay::desktop::utils::send_frames_surface_tree;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::{Physical, Point, Logical, Rectangle, Scale, Size, Transform};
use smithay::wayland::socket::ListeningSocketSource;

use config::Config;
use state::{ClientState, ColumnCompositor};
use terminal_manager::TerminalManager;

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
    if let Err(e) = terminal_manager.spawn() {
        tracing::error!("failed to spawn initial terminal: {}", e);
    }

    // Main event loop
    while compositor.running {
        // Update terminal_total_height BEFORE processing input events
        // so click detection uses the correct positions
        let mut current_terminal_height: i32 = 0;
        for id in terminal_manager.ids() {
            if let Some(term) = terminal_manager.get(id) {
                if let Some(tex) = term.get_texture() {
                    current_terminal_height += tex.size().h;
                }
            }
        }
        compositor.terminal_total_height = current_terminal_height;

        // Dispatch winit events
        let _ = winit_event_loop.dispatch_new_events(|event| {
            tracing::trace!("winit event: {:?}", std::mem::discriminant(&event));
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

            if let Err(e) = terminal_manager.spawn() {
                tracing::error!("failed to spawn terminal: {}", e);
            } else {
                // Calculate scroll position AFTER spawning to show new terminal
                let total_height = terminal_manager.total_height();
                let visible_height = compositor.output_size.h;
                let terminal_count = terminal_manager.count();

                // Log each terminal's position
                let mut y = 0i32;
                for id in terminal_manager.ids() {
                    if let Some(term) = terminal_manager.get(id) {
                        tracing::info!(id = id.0, y, height = term.height,
                                      "terminal position");
                        y += term.height as i32;
                    }
                }

                if total_height > visible_height {
                    compositor.scroll_offset = (total_height - visible_height) as f64;
                }
                tracing::info!(terminal_count, total_height, visible_height,
                              scroll = compositor.scroll_offset,
                              focused = ?terminal_manager.focused,
                              "spawned terminal, scrolling to show");
            }
        }

        // Handle focus change requests
        if compositor.focus_change_requested != 0 {
            let changed = if compositor.focus_change_requested > 0 {
                terminal_manager.focus_next()
            } else {
                terminal_manager.focus_prev()
            };
            compositor.focus_change_requested = 0;

            // Scroll to show the newly focused terminal
            if changed {
                if let Some((y, _height)) = terminal_manager.focused_position() {
                    let visible_height = compositor.output_size.h;
                    let total_height = terminal_manager.total_height();
                    let max_scroll = (total_height - visible_height).max(0) as f64;
                    // Scroll so focused terminal is at top
                    compositor.scroll_offset = (y as f64).clamp(0.0, max_scroll);
                }
            }
        }

        // Handle scroll requests
        if compositor.scroll_requested != 0.0 {
            let total_height = terminal_manager.total_height();
            let visible_height = compositor.output_size.h;
            let max_scroll = (total_height - visible_height).max(0) as f64;
            compositor.scroll_offset = (compositor.scroll_offset + compositor.scroll_requested)
                .clamp(0.0, max_scroll);
            compositor.scroll_requested = 0.0;
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
        if !dead.is_empty() {
            // If all terminals died, quit
            if terminal_manager.count() == 0 {
                tracing::info!("all terminals exited, shutting down");
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

            // Pre-compute window render elements before starting frame
            let scale = Scale::from(1.0);
            // Use the terminal_total_height computed at start of loop (for consistent click detection)
            let terminal_total_height = compositor.terminal_total_height;
            let mut window_y = -(compositor.scroll_offset as i32) + terminal_total_height;

            let window_elements: Vec<(i32, i32, Vec<WaylandSurfaceRenderElement<GlesRenderer>>)> = compositor.windows
                .iter()
                .map(|entry| {
                    let window = &entry.window;
                    let window_height = entry.state.target_height() as i32;
                    let y = window_y;
                    window_y += window_height;

                    // Only compute elements if visible
                    let elements = if y + window_height > 0 && y < physical_size.h {
                        let location: Point<i32, Logical> = Point::from((0, y));
                        window.render_elements(renderer, location.to_physical_precise_round(scale), scale, 1.0)
                    } else {
                        Vec::new()
                    };
                    (y, window_height, elements)
                })
                .collect();

            let mut frame = renderer.render(&mut framebuffer, physical_size, Transform::Normal)
                .map_err(|e| anyhow::anyhow!("render error: {e:?}"))?;

            // Clear with background color
            frame.clear(bg_color, &[damage])
                .map_err(|e| anyhow::anyhow!("clear error: {e:?}"))?;

            // Render terminals
            let focused_id = terminal_manager.focused;
            let mut y_offset: i32 = -(compositor.scroll_offset as i32);
            for id in terminal_manager.ids() {
                if let Some(terminal) = terminal_manager.get(id) {
                    if let Some(texture) = terminal.get_texture() {
                        let tex_size = texture.size();

                        // Only render if visible
                        if y_offset + tex_size.h > 0 && y_offset < physical_size.h {
                            // Render the texture with vertical flip to compensate for
                            // OpenGL's Y-up coordinate system
                            frame.render_texture_at(
                                texture,
                                Point::from((0, y_offset)),
                                1,     // texture_scale
                                1.0,   // output_scale
                                Transform::Flipped180,  // Flip for correct orientation
                                &[damage],  // damage
                                &[],   // opaque_regions
                                1.0,   // alpha
                            ).ok();

                            // Draw focus indicator on top (2px green border at top)
                            let is_focused = Some(id) == focused_id;
                            if is_focused && y_offset >= 0 {
                                let border_height = 2;
                                let focus_damage = Rectangle::from_loc_and_size(
                                    (0, y_offset),
                                    (physical_size.w, border_height),
                                );
                                frame.clear(Color32F::new(0.0, 0.8, 0.0, 1.0), &[focus_damage]).ok();
                            }
                        }

                        y_offset += tex_size.h;
                    }
                }
            }

            // Render external Wayland windows after terminals
            // To flip content vertically for OpenGL's Y-up coordinate system,
            // we flip the source Y coordinates (read buffer from bottom to top)
            for (_y, _window_height, elements) in window_elements {
                for element in elements {
                    let geo = element.geometry(scale);
                    let src = element.src();

                    // Flip source Y: read from (0, h) to (0, 0) instead of (0, 0) to (0, h)
                    let flipped_src = Rectangle::from_loc_and_size(
                        (src.loc.x, src.loc.y + src.size.h),
                        (src.size.w, -src.size.h),
                    );

                    element.draw(&mut frame, flipped_src, geo, &[damage], &[]).ok();
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
