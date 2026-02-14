//! Shared setup code for all backends
//!
//! Extracts the common output creation, terminal manager creation,
//! Wayland socket, IPC socket, and env var setup that was duplicated
//! across the X11, headless, and winit backends.

use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode as CalloopMode};
use smithay::utils::{Physical, Size, Transform};
use smithay::wayland::socket::ListeningSocketSource;

use crate::config::Config;
use crate::state::{ClientState, TermStack};
use crate::terminal_manager::TerminalManager;

/// Create a Smithay output with standard configuration.
pub fn create_output(name: &str, width: i32, height: i32) -> (Output, Mode, Size<i32, Physical>) {
    let mode = Mode {
        size: (width, height).into(),
        refresh: 60_000,
    };

    let output = Output::new(
        name.to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "TermStack".to_string(),
            model: name.to_string(),
        },
    );
    output.change_current_state(Some(mode), Some(Transform::Normal), None, Some((0, 0).into()));
    output.set_preferred(mode);

    let output_size = Size::from((width, height));
    (output, mode, output_size)
}

/// Create a terminal manager from config, sized for the given output.
pub fn create_terminal_manager(config: &Config, width: u32, height: u32) -> TerminalManager {
    let terminal_theme = config.theme.to_terminal_theme();
    let mut terminal_manager =
        TerminalManager::new_with_size(width, height, terminal_theme, config.font_size);
    terminal_manager.set_max_terminals(config.max_terminals);
    terminal_manager.set_max_dead_terminals(config.max_dead_terminals);
    terminal_manager
        .set_dead_terminal_ttl(Duration::from_secs(config.dead_terminal_ttl_minutes * 60));
    terminal_manager
}

/// Create a Wayland listening socket, insert it into the calloop event loop,
/// and set `WAYLAND_DISPLAY` for child processes.
///
/// Returns the socket name.
pub fn setup_wayland_socket(
    calloop_handle: &LoopHandle<'static, TermStack>,
) -> anyhow::Result<String> {
    let listening_socket = ListeningSocketSource::new_auto()
        .map_err(|e| anyhow::anyhow!("Failed to create Wayland socket: {e:?}"))?;

    let socket_name = listening_socket
        .socket_name()
        .to_string_lossy()
        .to_string();

    tracing::info!(?socket_name, "listening on Wayland socket");

    std::env::set_var("WAYLAND_DISPLAY", &socket_name);

    calloop_handle
        .insert_source(listening_socket, |client_stream, _, state| {
            tracing::info!("new Wayland client connected");
            if let Err(e) = state.display_handle.insert_client(
                client_stream,
                Arc::new(ClientState {
                    compositor_state: Default::default(),
                }),
            ) {
                tracing::error!(error = ?e, "Failed to insert Wayland client");
            }
        })
        .map_err(|e| anyhow::anyhow!("Failed to insert Wayland socket source: {e:?}"))?;

    Ok(socket_name)
}

/// Create an IPC socket for `termstack` CLI commands, insert the accept handler
/// into the calloop event loop, and set `TERMSTACK_SOCKET` and `TERMSTACK_BIN`.
///
/// Returns the IPC socket path.
pub fn setup_ipc_socket(
    calloop_handle: &LoopHandle<'static, TermStack>,
) -> anyhow::Result<PathBuf> {
    let ipc_socket_path = crate::ipc::socket_path();
    let _ = std::fs::remove_file(&ipc_socket_path); // Clean up old socket

    let ipc_listener = UnixListener::bind(&ipc_socket_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            anyhow::anyhow!(
                "IPC socket already in use: {:?}\n\
                 Another termstack compositor may be running, or a stale socket exists.\n\
                 Try: rm {:?}",
                ipc_socket_path,
                ipc_socket_path
            )
        } else {
            anyhow::anyhow!(
                "Failed to create IPC socket at {:?}: {}",
                ipc_socket_path,
                e
            )
        }
    })?;
    ipc_listener
        .set_nonblocking(true)
        .map_err(|e| anyhow::anyhow!("Failed to set IPC socket nonblocking: {}", e))?;

    std::env::set_var("TERMSTACK_SOCKET", &ipc_socket_path);

    let binary_path = std::env::current_exe().unwrap_or_else(|e| {
        tracing::warn!(
            error = ?e,
            "Failed to determine binary path, using 'termstack' from PATH"
        );
        PathBuf::from("termstack")
    });
    std::env::set_var("TERMSTACK_BIN", &binary_path);

    tracing::info!(
        path = ?ipc_socket_path,
        binary = ?binary_path,
        "IPC socket created, TERMSTACK_SOCKET and TERMSTACK_BIN set"
    );

    calloop_handle
        .insert_source(
            Generic::new(ipc_listener, Interest::READ, CalloopMode::Level),
            |_, listener, state| {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            tracing::debug!("IPC connection received");
                            match crate::ipc::read_ipc_request(stream) {
                                Ok((request, stream)) => match request {
                                    crate::ipc::IpcRequest::Spawn(spawn_req) => {
                                        // Guard against gui command loops
                                        if spawn_req.command.starts_with("gui ")
                                            || spawn_req.command == "gui"
                                        {
                                            tracing::warn!(
                                                command = %spawn_req.command,
                                                "Ignoring 'gui' command - use 'gui' function from shell integration"
                                            );
                                        } else if spawn_req.foreground.is_some() {
                                            tracing::info!(
                                                command = %spawn_req.command,
                                                foreground = spawn_req.foreground,
                                                "IPC GUI spawn request queued"
                                            );
                                            state.pending_gui_spawn_requests.push(spawn_req);
                                        } else {
                                            tracing::info!(
                                                command = %spawn_req.command,
                                                "IPC terminal spawn request queued"
                                            );
                                            state.pending_spawn_requests.push(spawn_req);
                                        }
                                    }
                                    crate::ipc::IpcRequest::Resize(mode) => {
                                        tracing::info!(?mode, "IPC resize request queued");
                                        state.pending_resize_request = Some((mode, stream));
                                    }
                                    crate::ipc::IpcRequest::Builtin(builtin_req) => {
                                        tracing::info!(
                                            command = %builtin_req.command,
                                            "IPC builtin request queued"
                                        );
                                        state.pending_builtin_requests.push(builtin_req);
                                    }
                                    crate::ipc::IpcRequest::QueryWindows => {
                                        let windows: Vec<crate::ipc::WindowInfo> = state
                                            .layout_nodes
                                            .iter()
                                            .enumerate()
                                            .map(|(i, node)| {
                                                let (is_external, command, actual_width) =
                                                    match &node.cell {
                                                        crate::state::StackWindow::Terminal(_) => (
                                                            false,
                                                            String::new(),
                                                            state.output_size.w,
                                                        ),
                                                        crate::state::StackWindow::External(
                                                            entry,
                                                        ) => {
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
                                        tracing::info!(
                                            window_count = windows.len(),
                                            "IPC query_windows response"
                                        );
                                        if let Err(e) =
                                            crate::ipc::send_json_response(stream, &windows)
                                        {
                                            tracing::warn!(
                                                error = ?e,
                                                "Failed to send query_windows response"
                                            );
                                        }
                                    }
                                },
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
        )
        .map_err(|e| anyhow::anyhow!("Failed to insert IPC socket source: {e:?}"))?;

    Ok(ipc_socket_path)
}

/// Set toolkit environment variables so GTK/Qt apps use the Wayland backend.
pub fn set_toolkit_env_vars() {
    std::env::set_var("GDK_BACKEND", "wayland");
    std::env::set_var("QT_QPA_PLATFORM", "wayland");
}
