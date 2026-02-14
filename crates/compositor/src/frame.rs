//! Unified per-frame processing
//!
//! All backends call `process_frame()` once per iteration to run the shared
//! compositor logic (spawn handling, terminal output, window lifecycle, layout).
//! Backend-specific concerns (display dispatch, calloop dispatch, rendering,
//! frame callbacks) remain in each backend.

use crate::state::TermStack;
use crate::terminal_manager::TerminalManager;

/// Result of processing a single frame.
pub struct FrameResult {
    /// True when every terminal has exited and the compositor should shut down.
    pub all_terminals_exited: bool,
}

/// Run one frame of shared compositor logic.
///
/// Backends should call this after dispatching calloop and Wayland clients,
/// and before rendering / sending frame callbacks.
pub fn process_frame(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
    height_calculator: fn(&TermStack, &TerminalManager) -> Vec<i32>,
) -> FrameResult {
    // 1. Clear stale drag state if no pointer buttons are pressed
    //    (handles lost release events when window loses focus mid-drag)
    compositor.clear_stale_drag_state(compositor.pointer_buttons_pressed > 0);

    // 2. Cancel pending resizes from unresponsive clients
    compositor.cancel_stale_pending_resizes();

    // 3. Cleanup popup internal resources
    compositor.popup_manager.cleanup();

    // 4. Handle external window insert/resize events
    crate::window_lifecycle::handle_external_window_events(compositor);

    // 5. Handle focus change requests from input
    crate::input_handler::handle_focus_change_requests(compositor, terminal_manager);

    // 6–8. Handle spawn requests from IPC
    crate::spawn_handler::handle_ipc_spawn_requests(
        compositor,
        terminal_manager,
        height_calculator,
    );
    crate::spawn_handler::handle_gui_spawn_requests(
        compositor,
        terminal_manager,
        height_calculator,
    );
    crate::spawn_handler::handle_builtin_requests(
        compositor,
        terminal_manager,
        height_calculator,
    );

    // 9. Handle resize requests from IPC
    crate::terminal_output::handle_ipc_resize_request(compositor, terminal_manager);

    // 10. Handle key repeat for terminals
    crate::input_handler::handle_key_repeat(compositor, terminal_manager);

    // 11. Process terminal PTY output and handle sizing actions
    crate::terminal_output::process_terminal_output(compositor, terminal_manager);

    // 12. Promote output terminals that have content
    crate::terminal_output::promote_output_terminals(compositor, terminal_manager);

    // 13. Handle cleanup of output terminals from closed windows
    crate::window_lifecycle::handle_output_terminal_cleanup(compositor, terminal_manager);

    // 14. Handle restoration of launchers when output terminals are already gone
    crate::window_lifecycle::handle_launcher_restoration(compositor, terminal_manager);

    // 15. Cleanup dead terminals and check if all have exited
    let all_terminals_exited =
        crate::window_lifecycle::cleanup_and_sync_focus(compositor, terminal_manager);

    // 16. Handle terminal spawn requests (Ctrl+Shift+Enter)
    crate::window_lifecycle::handle_terminal_spawn(
        compositor,
        terminal_manager,
        height_calculator,
    );

    // 17. Handle font size changes
    if compositor.pending_font_size_delta != 0.0 {
        let delta = compositor.pending_font_size_delta;
        compositor.pending_font_size_delta = 0.0;

        let new_font_size = (terminal_manager.font_size() + delta).clamp(6.0, 72.0);
        terminal_manager.set_font_size(
            new_font_size,
            compositor.output_size.w as u32,
            compositor.output_size.h as u32,
        );
    }

    // 18. Apply accumulated scroll delta
    compositor.apply_pending_scroll();

    // 19. Calculate and update window heights, auto-scroll if needed
    let window_heights = height_calculator(compositor, terminal_manager);
    crate::window_height::check_and_handle_height_changes(compositor, window_heights);

    // 20. Recalculate layout positions
    compositor.recalculate_layout();

    // 21. Process pending PRIMARY selection paste (from middle-click)
    compositor.process_primary_selection_paste(terminal_manager);

    // 22–23. Timeout stale state
    compositor.timeout_stale_clipboard_reads();
    compositor.timeout_stale_pending_window();

    // 24. Validate state invariants in debug builds
    #[cfg(debug_assertions)]
    compositor.validate_state(terminal_manager);

    FrameResult {
        all_terminals_exited,
    }
}
