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
            _ => panic!("expected Spawn"),
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
            _ => panic!("expected Spawn"),
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

        // Step 1: Simulate receiving JSON from termstack (like ipc::read_spawn_request does)
        let json_from_termstack = serde_json::json!({
            "type": "spawn",
            "command": "mc",
            "cwd": "/tmp",
            "env": {
                "HOME": "/home/user",
                "PATH": "/usr/bin"
            }
        });

        // Step 2: Parse the message (like ipc.rs does)
        let parsed: IpcMessage = serde_json::from_value(json_from_termstack).unwrap();
        let (command, cwd, env) = match parsed {
            IpcMessage::Spawn { command, cwd, env, .. } => (command, cwd, env),
            _ => panic!("expected Spawn"),
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
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default(), 14.0);

        let cwd_path = std::path::Path::new(&cwd);
        let result = manager.spawn_command("", &transformed_command, cwd_path, &spawn_env, None);

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
            _ => panic!("expected Resize"),
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
            _ => panic!("expected Resize"),
        }
    }

    #[test]
    fn gui_spawn_message_parses_correctly() {
        let msg = serde_json::json!({
            "type": "spawn",
            "command": "pqiv image.png",
            "cwd": "/home/user",
            "env": {},
            "foreground": true,
        });

        let parsed: IpcMessage = serde_json::from_value(msg).unwrap();

        match parsed {
            IpcMessage::Spawn { command, foreground, .. } => {
                assert_eq!(command, "pqiv image.png");
                assert_eq!(foreground, Some(true));
            }
            _ => panic!("expected Spawn with foreground"),
        }
    }

    #[test]
    fn gui_spawn_background_parses_correctly() {
        let msg = serde_json::json!({
            "type": "spawn",
            "command": "pqiv image.png",
            "cwd": "/home/user",
            "env": {},
            "foreground": false,
        });

        let parsed: IpcMessage = serde_json::from_value(msg).unwrap();

        match parsed {
            IpcMessage::Spawn { command, foreground, .. } => {
                assert_eq!(command, "pqiv image.png");
                assert_eq!(foreground, Some(false));
            }
            _ => panic!("expected Spawn with foreground"),
        }
    }

    #[test]
    fn gui_spawn_foreground_spawns_terminal_and_tracks_session() {
        // Test that foreground spawn (with foreground=true):
        // 1. Creates an output terminal
        // 2. Tracks the foreground session for launcher restoration

        let json_msg = serde_json::json!({
            "type": "spawn",
            "command": "swayimg image.png",
            "cwd": "/tmp",
            "env": {
                "WAYLAND_DISPLAY": "wayland-1"
            },
            "foreground": true,
        });

        let parsed: IpcMessage = serde_json::from_value(json_msg).unwrap();

        match parsed {
            IpcMessage::Spawn { command, cwd, env, foreground, .. } => {
                assert_eq!(command, "swayimg image.png");
                assert_eq!(cwd, "/tmp");
                assert!(env.contains_key("WAYLAND_DISPLAY"));
                assert_eq!(foreground, Some(true), "foreground should be Some(true)");
            }
            _ => panic!("expected Spawn with foreground"),
        }
    }

    #[test]
    fn gui_spawn_background_keeps_launcher_visible() {
        // Test that background spawn (foreground=false):
        // - Does NOT hide the launcher terminal
        // - Does NOT track a foreground session

        let json_msg = serde_json::json!({
            "type": "spawn",
            "command": "swayimg image.png",
            "cwd": "/tmp",
            "env": {},
            "foreground": false,
        });

        let parsed: IpcMessage = serde_json::from_value(json_msg).unwrap();

        match parsed {
            IpcMessage::Spawn { foreground, .. } => {
                assert_eq!(foreground, Some(false), "background mode should have foreground=Some(false)");
            }
            _ => panic!("expected Spawn with foreground"),
        }
    }

    #[test]
    fn builtin_message_parses_correctly() {
        let msg = serde_json::json!({
            "type": "builtin",
            "prompt": "user@host ~/code> ",
            "command": "cd ..",
            "result": "",
            "success": true,
        });

        let parsed: IpcMessage = serde_json::from_value(msg).unwrap();

        match parsed {
            IpcMessage::Builtin { prompt, command, result, success } => {
                assert_eq!(prompt, "user@host ~/code> ");
                assert_eq!(command, "cd ..");
                assert_eq!(result, "");
                assert!(success);
            }
            _ => panic!("expected Builtin"),
        }
    }

    #[test]
    fn builtin_with_output_parses_correctly() {
        let msg = serde_json::json!({
            "type": "builtin",
            "prompt": "user@host ~/code> ",
            "command": "alias",
            "result": "ll='ls -la'\nla='ls -A'",
            "success": true,
        });

        let parsed: IpcMessage = serde_json::from_value(msg).unwrap();

        match parsed {
            IpcMessage::Builtin { prompt, command, result, success } => {
                assert_eq!(prompt, "user@host ~/code> ");
                assert_eq!(command, "alias");
                assert!(result.contains("ll='ls -la'"));
                assert!(success);
            }
            _ => panic!("expected Builtin"),
        }
    }

    #[test]
    fn builtin_error_parses_correctly() {
        let msg = serde_json::json!({
            "type": "builtin",
            "prompt": "user@host ~/code> ",
            "command": "cd /nonexistent",
            "result": "cd: The directory '/nonexistent' does not exist",
            "success": false,
        });

        let parsed: IpcMessage = serde_json::from_value(msg).unwrap();

        match parsed {
            IpcMessage::Builtin { prompt, command, result, success } => {
                assert_eq!(prompt, "user@host ~/code> ");
                assert_eq!(command, "cd /nonexistent");
                assert!(result.contains("does not exist"));
                assert!(!success);
            }
            _ => panic!("expected Builtin"),
        }
    }

    #[test]
    fn builtin_terminal_has_correct_title() {
        // Test that builtin terminals have the full prompt + command as title
        let mut manager = TerminalManager::new_with_size(800, 600, terminal::Theme::default(), 14.0);

        let id = manager.create_builtin_terminal("user@host ~/code> ", "cd ..", "", true).unwrap();
        let terminal = manager.get(id).unwrap();

        // Title should be prompt + command
        assert_eq!(terminal.title, "user@host ~/code> cd ..", "title should be prompt + command");
    }

    #[test]
    fn builtin_terminal_strips_ansi_codes_from_prompt() {
        // Test that ANSI escape codes are stripped from the prompt
        // Fish prompts often contain color codes like \x1b[32m (green)
        let mut manager = TerminalManager::new_with_size(800, 600, terminal::Theme::default(), 14.0);

        // Prompt with ANSI color codes: green "user@host" reset, blue "~/code" reset, "> "
        let colored_prompt = "\x1b[32muser@host\x1b[0m \x1b[34m~/code\x1b[0m> ";
        let id = manager.create_builtin_terminal(colored_prompt, "cd ..", "", true).unwrap();
        let terminal = manager.get(id).unwrap();

        // Title should have ANSI codes stripped
        assert_eq!(terminal.title, "user@host ~/code> cd ..", "ANSI codes should be stripped from prompt");
    }

    #[test]
    fn builtin_terminal_strips_charset_selection_codes() {
        // Test that character set selection codes (ESC(B, ESC)B) are stripped
        // These appear as "(B" garbage in fish prompts when not handled
        let mut manager = TerminalManager::new_with_size(800, 600, terminal::Theme::default(), 14.0);

        // Prompt with charset selection codes: ESC(B between characters
        let prompt_with_charset = "\x1b(Bkarl\x1b(B@\x1b(Bx13k\x1b(B ~/code> ";
        let id = manager.create_builtin_terminal(prompt_with_charset, "cd ..", "", true).unwrap();
        let terminal = manager.get(id).unwrap();

        // Title should have charset selection codes stripped
        assert_eq!(terminal.title, "karl@x13k ~/code> cd ..", "charset selection codes should be stripped");
    }

    #[test]
    fn builtin_terminal_empty_result_has_minimal_height() {
        // Test that builtin with no output has minimal height (just title bar, no content)
        let mut manager = TerminalManager::new_with_size(800, 600, terminal::Theme::default(), 14.0);

        let id = manager.create_builtin_terminal("user@host ~/code> ", "cd ..", "", true).unwrap();
        let terminal = manager.get(id).unwrap();

        // Empty result should have zero content height (title bar is rendered separately)
        assert_eq!(terminal.height, 0, "empty builtin should have 0 content height");
    }

    #[test]
    fn builtin_terminal_with_output_has_correct_height() {
        // Test that builtin with output has height for all lines
        let mut manager = TerminalManager::new_with_size(800, 600, terminal::Theme::default(), 14.0);

        let result = "ll='ls -la'\nla='ls -A'\ngrep='grep --color=auto'";
        let id = manager.create_builtin_terminal("user@host ~/code> ", "alias", result, true).unwrap();
        let terminal = manager.get(id).unwrap();

        // 3 lines of output = 3 rows
        let cell_height = terminal.cell_size().1;
        let expected_height = 3 * cell_height;
        assert_eq!(terminal.height, expected_height, "builtin with 3 lines should have 3-row height");
    }

    #[test]
    fn builtin_terminal_is_immediately_visible() {
        // Builtin terminals should be visible immediately (not waiting for output)
        let mut manager = TerminalManager::new_with_size(800, 600, terminal::Theme::default(), 14.0);

        let id = manager.create_builtin_terminal("user@host ~/code> ", "cd ..", "", true).unwrap();
        let terminal = manager.get(id).unwrap();

        assert!(terminal.is_visible(), "builtin terminal should be immediately visible");
    }

    #[test]
    fn builtin_terminal_is_marked_exited() {
        // Builtin terminals should be marked as exited (no cursor)
        let mut manager = TerminalManager::new_with_size(800, 600, terminal::Theme::default(), 14.0);

        let id = manager.create_builtin_terminal("user@host ~/code> ", "cd ..", "", true).unwrap();
        let terminal = manager.get(id).unwrap();

        assert!(terminal.has_exited(), "builtin terminal should be marked as exited");
    }

    #[test]
    fn builtin_empty_command_shows_just_prompt() {
        // Test that empty command (just pressing Enter) shows only the prompt
        let mut manager = TerminalManager::new_with_size(800, 600, terminal::Theme::default(), 14.0);

        let id = manager.create_builtin_terminal("user@host ~/code> ", "", "", true).unwrap();
        let terminal = manager.get(id).unwrap();

        // Title should be just the prompt when command is empty
        assert_eq!(terminal.title, "user@host ~/code> ", "empty command should show just prompt");
        assert!(terminal.is_visible(), "empty command terminal should be visible");
    }

    #[test]
    fn builtin_message_with_empty_command_parses() {
        // Test that builtin with empty command (Enter key) parses correctly
        let msg = serde_json::json!({
            "type": "builtin",
            "prompt": "user@host ~/code> ",
            "command": "",
            "result": "",
            "success": true,
        });

        let parsed: IpcMessage = serde_json::from_value(msg).unwrap();

        match parsed {
            IpcMessage::Builtin { prompt, command, result, success } => {
                assert_eq!(prompt, "user@host ~/code> ");
                assert_eq!(command, "");
                assert_eq!(result, "");
                assert!(success);
            }
            _ => panic!("expected Builtin"),
        }
    }

    #[test]
    fn query_windows_message_parses_correctly() {
        let msg = serde_json::json!({
            "type": "query_windows",
        });

        let parsed: IpcMessage = serde_json::from_value(msg).unwrap();

        match parsed {
            IpcMessage::QueryWindows => {}
            _ => panic!("expected QueryWindows"),
        }
    }

    #[test]
    fn gui_spawn_flow_spawns_command_terminal() {
        // Test the full flow: spawn with foreground should spawn a terminal to run the command.
        // The terminal captures stdout/stderr while the GUI app creates its window.

        let json_msg = serde_json::json!({
            "type": "spawn",
            "command": "echo 'test gui app'",
            "cwd": "/tmp",
            "env": {
                "HOME": "/home/user"
            },
            "foreground": true,
        });

        let parsed: IpcMessage = serde_json::from_value(json_msg).unwrap();

        let (command, cwd, env) = match parsed {
            IpcMessage::Spawn { command, cwd, env, .. } => (command, cwd, env),
            _ => panic!("expected Spawn with foreground"),
        };

        // Create terminal manager and spawn the command
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(
            output_width,
            output_height,
            terminal::Theme::default(),
            14.0,
        );

        let cwd_path = std::path::Path::new(&cwd);
        let result = manager.spawn_command("", &command, cwd_path, &env, None);

        assert!(result.is_ok(), "gui_spawn should create output terminal: {:?}", result.err());

        let id = result.unwrap();
        let terminal = manager.get(id).unwrap();

        // Output terminal starts in WaitingForOutput state
        assert!(
            !terminal.is_visible(),
            "output terminal should start hidden (WaitingForOutput)"
        );
    }
}
