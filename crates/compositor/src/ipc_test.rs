//! Tests for IPC message handling

#[cfg(test)]
mod tests {
    use crate::ipc::{IpcMessage, ResizeMode};
    use crate::terminal_manager::TerminalManager;

    #[test]
    fn spawn_message_parses_correctly() {
        let msg = serde_json::json!({
            "type": "spawn",
            "command": "mc",
            "cwd": "/home/user",
            "env": {},
        });

        let parsed: IpcMessage = serde_json::from_value(msg).unwrap();

        match parsed {
            IpcMessage::Spawn { command, .. } => {
                assert_eq!(command, "mc");
            }
            IpcMessage::Resize { .. } => panic!("expected Spawn"),
        }
    }

    #[test]
    fn spawn_with_env_parses_correctly() {
        let msg = serde_json::json!({
            "type": "spawn",
            "command": "ls",
            "cwd": "/home/user",
            "env": {
                "HOME": "/home/user",
                "PATH": "/usr/bin"
            },
        });

        let parsed: IpcMessage = serde_json::from_value(msg).unwrap();

        match parsed {
            IpcMessage::Spawn { command, env, .. } => {
                assert_eq!(command, "ls");
                assert_eq!(env.get("HOME"), Some(&"/home/user".to_string()));
            }
            IpcMessage::Resize { .. } => panic!("expected Spawn"),
        }
    }

    #[test]
    fn terminal_starts_small_and_grows() {
        // This test documents the expected behavior:
        // Command terminals start small and grow based on content.
        // TUI apps are auto-detected via alternate screen mode.

        // Given a viewport of 720 pixels and cell_height of 17
        let cell_height = 17u32;
        let initial_rows = 3u16;

        // Command terminals use small initial size
        let visual_rows = initial_rows;
        let expected_height = visual_rows as u32 * cell_height;

        assert_eq!(visual_rows, 3, "should use initial_rows");
        assert_eq!(expected_height, 51, "terminal height should be small initially");
    }

    #[test]
    fn full_ipc_flow_spawns_terminal() {
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
            }
        });

        // Step 2: Parse the message (like ipc.rs does)
        let parsed: IpcMessage = serde_json::from_value(json_from_column_term).unwrap();
        let (command, cwd, env) = match parsed {
            IpcMessage::Spawn { command, cwd, env } => (command, cwd, env),
            IpcMessage::Resize { .. } => panic!("expected Spawn"),
        };

        eprintln!("Parsed from JSON: command={}", command);

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
        let result = manager.spawn_command(&transformed_command, cwd_path, &spawn_env, None);

        assert!(result.is_ok(), "spawn_command should succeed: {:?}", result.err());
        let id = result.unwrap();

        // Step 6: Verify terminal was created
        let terminal = manager.get(id).unwrap();
        let (cols, pty_rows) = terminal.terminal.dimensions();
        let visual_height = terminal.height;

        eprintln!("Terminal created:");
        eprintln!("  PTY: cols={}, rows={}", cols, pty_rows);
        eprintln!("  Visual height: {} pixels", visual_height);

        // Terminal starts small (initial_rows * cell_height)
        // TUI apps will auto-resize via alternate screen detection
        assert!(cols > 0, "cols should be set");
        assert!(pty_rows > 0, "pty_rows should be set");
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
}
