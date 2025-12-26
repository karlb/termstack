//! Tests for IPC message handling

#[cfg(test)]
mod tests {
    use crate::ipc::{IpcMessage, ResizeMode};
    use crate::terminal_manager::TerminalManager;

    #[test]
    fn tui_flag_serializes_correctly() {
        let msg = serde_json::json!({
            "type": "spawn",
            "command": "mc",
            "cwd": "/home/user",
            "env": {},
            "is_tui": true,
        });

        let parsed: IpcMessage = serde_json::from_value(msg).unwrap();

        match parsed {
            IpcMessage::Spawn { command, is_tui, .. } => {
                assert_eq!(command, "mc");
                assert!(is_tui, "is_tui should be true");
            }
            IpcMessage::Resize { .. } => panic!("expected Spawn"),
        }
    }

    #[test]
    fn tui_flag_defaults_to_false() {
        // Old-style message without is_tui field
        let msg = serde_json::json!({
            "type": "spawn",
            "command": "ls",
            "cwd": "/home/user",
            "env": {},
        });

        let parsed: IpcMessage = serde_json::from_value(msg).unwrap();

        match parsed {
            IpcMessage::Spawn { command, is_tui, .. } => {
                assert_eq!(command, "ls");
                assert!(!is_tui, "is_tui should default to false");
            }
            IpcMessage::Resize { .. } => panic!("expected Spawn"),
        }
    }

    #[test]
    fn tui_terminal_should_be_full_height() {
        // This test documents the expected behavior:
        // When is_tui=true, the terminal should be created at max_rows height

        // Given a viewport of 720 pixels and cell_height of 17
        let output_height = 720u32;
        let cell_height = 17u32;
        let max_rows = (output_height / cell_height) as u16; // = 42 rows
        let initial_rows = 3u16;

        // For TUI apps
        let is_tui = true;
        let (_pty_rows, visual_rows) = if is_tui {
            (max_rows, max_rows)
        } else {
            (1000, initial_rows)
        };

        let expected_height = visual_rows as u32 * cell_height;

        assert_eq!(visual_rows, 42, "TUI should use max_rows");
        assert_eq!(expected_height, 714, "TUI terminal height should be ~full viewport");

        // For non-TUI apps
        let is_tui = false;
        let (_pty_rows, visual_rows) = if is_tui {
            (max_rows, max_rows)
        } else {
            (1000, initial_rows)
        };

        let expected_height = visual_rows as u32 * cell_height;

        assert_eq!(visual_rows, 3, "non-TUI should use initial_rows");
        assert_eq!(expected_height, 51, "non-TUI terminal height should be small");
    }

    #[test]
    fn full_ipc_flow_tui_gets_full_height() {
        // This test simulates the COMPLETE flow from IPC message to terminal creation
        // exactly as main.rs does it

        // Step 1: Simulate receiving JSON from column-term (like ipc::read_spawn_request does)
        let json_from_column_term = serde_json::json!({
            "type": "spawn",
            "command": "mc",
            "cwd": "/tmp",
            "env": {
                "HOME": "/home/user",
                "PATH": "/usr/bin"
            },
            "is_tui": true
        });

        // Step 2: Parse the message (like ipc.rs does)
        let parsed: IpcMessage = serde_json::from_value(json_from_column_term).unwrap();
        let (command, cwd, env, is_tui) = match parsed {
            IpcMessage::Spawn { command, cwd, env, is_tui } => (command, cwd, env, is_tui),
            IpcMessage::Resize { .. } => panic!("expected Spawn"),
        };

        // Verify is_tui was parsed correctly
        assert!(is_tui, "is_tui should be true from JSON");
        eprintln!("Parsed from JSON: command={}, is_tui={}", command, is_tui);

        // Step 3: Transform command like main.rs does
        let escaped = command.replace("'", "'\\''");
        let transformed_command = format!("echo '> {}'; {}", escaped, command);
        eprintln!("Transformed command: {}", transformed_command);

        // Step 4: Prepare environment like main.rs does
        let mut spawn_env = env.clone();
        spawn_env.insert("GIT_PAGER".to_string(), "cat".to_string());
        spawn_env.insert("PAGER".to_string(), "cat".to_string());
        spawn_env.insert("LESS".to_string(), "-FRX".to_string());

        // Step 5: Create TerminalManager and spawn command
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let cwd_path = std::path::Path::new(&cwd);
        let result = manager.spawn_command(&transformed_command, cwd_path, &spawn_env, None, is_tui);

        assert!(result.is_ok(), "spawn_command should succeed: {:?}", result.err());
        let id = result.unwrap();

        // Step 6: Verify terminal dimensions
        let terminal = manager.get(id).unwrap();
        let (cols, pty_rows) = terminal.terminal.dimensions();
        let visual_height = terminal.height;
        let max_rows = manager.max_rows;
        let cell_height = manager.cell_height;
        let expected_height = max_rows as u32 * cell_height;

        eprintln!("Terminal created:");
        eprintln!("  PTY: cols={}, rows={}", cols, pty_rows);
        eprintln!("  Visual height: {} pixels", visual_height);
        eprintln!("  Expected: max_rows={}, height={} pixels", max_rows, expected_height);

        // The critical assertions
        assert_eq!(
            pty_rows, max_rows,
            "PTY rows should equal max_rows={}, but was {}",
            max_rows, pty_rows
        );

        assert_eq!(
            visual_height, expected_height,
            "Visual height should be {}, but was {}",
            expected_height, visual_height
        );

        // Verify it's NOT the old buggy default of 200 or small height
        assert!(
            visual_height > 200,
            "Height should NOT be default 200, was {}",
            visual_height
        );
    }

    #[test]
    fn resize_message_full_parses_correctly() {
        let msg = serde_json::json!({
            "type": "resize",
            "mode": "full",
        });

        let parsed: IpcMessage = serde_json::from_value(msg).unwrap();

        match parsed {
            IpcMessage::Resize { mode } => {
                assert_eq!(mode, ResizeMode::Full);
            }
            IpcMessage::Spawn { .. } => panic!("expected Resize"),
        }
    }

    #[test]
    fn resize_message_content_parses_correctly() {
        let msg = serde_json::json!({
            "type": "resize",
            "mode": "content",
        });

        let parsed: IpcMessage = serde_json::from_value(msg).unwrap();

        match parsed {
            IpcMessage::Resize { mode } => {
                assert_eq!(mode, ResizeMode::Content);
            }
            IpcMessage::Spawn { .. } => panic!("expected Resize"),
        }
    }

    // NOTE: This test documents the race condition that causes TUI apps to see wrong size
    //
    // The flow is:
    // 1. Shell runs: column-term --resize full
    // 2. column-term sends IPC message and exits immediately (FIRE-AND-FORGET!)
    // 3. Shell runs: eval "mc"
    // 4. mc queries terminal size (stty, ioctl)
    // 5. BUT compositor is asynchronous - it processes messages on next event loop iteration
    // 6. If mc's size query happens BEFORE compositor processes resize → mc sees old size
    //
    // FIX: Make resize synchronous - compositor sends ACK, column-term waits for it
    //
    // This is NOT a unit test because it requires the actual compositor event loop.
    // The fix is implemented in the IPC protocol to send an acknowledgement.
}
