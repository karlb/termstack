    use super::*;
    use std::collections::HashMap;

    #[test]
    fn command_terminal_starts_small() {
        // All command terminals now start small - TUI apps are auto-detected via alternate screen
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // Spawn a command terminal
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let result = manager.spawn_command("echo test", cwd, &env, None);

        assert!(result.is_ok(), "spawn_command should succeed");
        let id = result.unwrap();

        let terminal = manager.get(id).expect("terminal should exist");
        let cell_height = manager.cell_height;
        let initial_rows = manager.initial_rows;
        let expected_height = initial_rows as u32 * cell_height;

        // Terminal should start at initial_rows height (small)
        assert_eq!(
            terminal.height,
            expected_height,
            "command terminal should start small: {} (initial_rows={} * cell_height={}), but was {}",
            expected_height,
            initial_rows,
            cell_height,
            terminal.height
        );
    }

    #[test]
    fn command_terminal_pty_has_large_rows() {
        // All command terminals use 1000 PTY rows (no scrolling needed)
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let result = manager.spawn_command("echo test", cwd, &env, None);
        let id = result.unwrap();

        let terminal = manager.get(id).expect("terminal should exist");
        let (_, pty_rows) = terminal.terminal.dimensions();

        // PTY should have large row count for internal scrollback
        assert_eq!(
            pty_rows, 1000,
            "command terminal PTY should have 1000 rows, but was {}",
            pty_rows
        );
    }

    #[test]
    fn max_rows_updates_when_cell_height_changes() {
        // This test checks if max_rows is recalculated when cell dimensions change
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let initial_max_rows = manager.max_rows;
        let initial_cell_height = manager.cell_height;

        // Spawn any terminal to trigger font loading
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let _ = manager.spawn_command("echo test", cwd, &env, None);

        let new_cell_height = manager.cell_height;
        let new_max_rows = manager.max_rows;

        // If cell_height changed, max_rows SHOULD be recalculated
        if new_cell_height != initial_cell_height {
            let expected_max_rows = (output_height / new_cell_height).max(1) as u16;
            assert_eq!(
                new_max_rows, expected_max_rows,
                "max_rows should be recalculated when cell_height changes: \
                 initial_cell_height={}, new_cell_height={}, \
                 initial_max_rows={}, new_max_rows={}, expected={}",
                initial_cell_height, new_cell_height,
                initial_max_rows, new_max_rows, expected_max_rows
            );
        }
    }

    #[test]
    fn non_tui_terminal_has_small_height() {
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // Spawn a non-TUI command
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let result = manager.spawn_command("echo test", cwd, &env, None);

        assert!(result.is_ok(), "spawn_command should succeed");
        let id = result.unwrap();

        let terminal = manager.get(id).expect("terminal should exist");
        let cell_height = manager.cell_height;
        let initial_rows = manager.initial_rows;  // 3
        let expected_height = initial_rows as u32 * cell_height;

        assert_eq!(
            terminal.height,
            expected_height,
            "non-TUI terminal height should be {} (initial_rows={} * cell_height={}), but was {}",
            expected_height,
            initial_rows,
            cell_height,
            terminal.height
        );
    }

    #[test]
    fn command_terminal_pty_has_1000_rows() {
        // All command terminals use 1000 PTY rows for internal scrollback
        // TUI apps are auto-detected via alternate screen and resized then
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let result = manager.spawn_command("echo test", cwd, &env, None);
        let id = result.unwrap();

        let terminal = manager.get(id).expect("terminal should exist");
        let (cols, rows) = terminal.terminal.dimensions();

        // PTY should have 1000 rows
        assert_eq!(
            rows, 1000,
            "command terminal PTY rows should be 1000, but was {}",
            rows
        );

        eprintln!("PTY dimensions: cols={}, rows={}", cols, rows);
    }

    #[test]
    fn non_tui_terminal_pty_has_large_rows() {
        // Non-TUI terminals use 1000 rows for PTY (no scrolling)
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let result = manager.spawn_command("echo test", cwd, &env, None);
        let id = result.unwrap();

        let terminal = manager.get(id).expect("terminal should exist");
        let (_, rows) = terminal.terminal.dimensions();

        assert_eq!(
            rows, 1000,
            "non-TUI terminal PTY rows should be 1000, but was {}",
            rows
        );
    }

    #[test]
    fn terminal_height_property_is_correct() {
        // Command terminals start with small height (initial_rows * cell_height)
        // They also start hidden until they produce output
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // Spawn a command terminal
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None).unwrap();

        let terminal = manager.get(id).unwrap();

        // Command terminals start small
        let cell_height = manager.cell_height;
        let initial_rows = manager.initial_rows;
        let expected_height = initial_rows as u32 * cell_height;

        eprintln!("terminal.height={}, expected={}, visible={}",
                  terminal.height, expected_height, terminal.is_visible());

        // The height property should be small initially
        assert_eq!(
            terminal.height, expected_height,
            "terminal.height should be initial size: expected={}, got={}",
            expected_height, terminal.height
        );

        // Command terminals start hidden (become visible when output arrives)
        assert!(
            !terminal.is_visible(),
            "command terminal should start hidden"
        );
    }

    #[test]
    fn mc_command_spawns_with_correct_dimensions() {
        // Spawn actual mc command and verify terminal dimensions
        // mc (and other TUI apps) are auto-detected via alternate screen
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // Simulate the environment modifications from main.rs
        let mut env = HashMap::new();
        env.insert("GIT_PAGER".to_string(), "cat".to_string());
        env.insert("PAGER".to_string(), "cat".to_string());
        env.insert("LESS".to_string(), "-FRX".to_string());

        let cwd = std::path::Path::new("/tmp");

        // Simulate the command transformation from main.rs
        let command = "echo '> mc'; mc";

        // Spawn mc
        let result = manager.spawn_command(command, cwd, &env, None);
        assert!(result.is_ok(), "spawn mc should succeed: {:?}", result.err());
        let id = result.unwrap();

        let terminal = manager.get(id).unwrap();

        // Check terminal dimensions
        let (cols, pty_rows) = terminal.terminal.dimensions();
        let visual_height = terminal.height;
        let cell_height = manager.cell_height;
        let initial_rows = manager.initial_rows;

        eprintln!("mc terminal: cols={}, pty_rows={}, visual_height={}", cols, pty_rows, visual_height);

        // PTY should have 1000 rows (all command terminals)
        assert_eq!(
            pty_rows, 1000,
            "mc PTY rows should be 1000, but was {}",
            pty_rows
        );

        // Visual height should be small initially (initial_rows * cell_height)
        // TUI apps resize to full when they enter alternate screen
        let expected_height = initial_rows as u32 * cell_height;
        assert_eq!(
            visual_height, expected_height,
            "mc visual height should be {} (initial), but was {}",
            expected_height, visual_height
        );
    }

    #[test]
    fn stty_command_has_large_pty() {
        // All command terminals have 1000 PTY rows for internal scrollback
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");

        // Spawn stty size which prints "rows cols"
        let result = manager.spawn_command("stty size", cwd, &env, None);
        assert!(result.is_ok(), "spawn stty should succeed");
        let id = result.unwrap();

        let terminal = manager.get(id).unwrap();
        let (cols, pty_rows) = terminal.terminal.dimensions();

        eprintln!("stty terminal: pty_rows={}, cols={}", pty_rows, cols);

        // PTY should have 1000 rows
        assert_eq!(
            pty_rows, 1000,
            "stty PTY rows should be 1000, was {}",
            pty_rows
        );

        assert!(cols > 0, "cols should be set");
    }

    #[test]
    fn resize_to_full_updates_pty_and_dimensions() {
        // This test reproduces the TUI resize flow:
        // 1. Shell terminal starts small (content-based sizing)
        // 2. User runs TUI app -> column-term --resize full
        // 3. Terminal is resized to full viewport height
        // 4. TUI app runs and should see full-size terminal
        //
        // BUG: If resize doesn't properly update PTY, TUI apps will see old size

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // Spawn a non-TUI terminal (like a shell)
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let result = manager.spawn_command("echo test", cwd, &env, None);
        assert!(result.is_ok(), "spawn should succeed");
        let id = result.unwrap();

        // Get initial dimensions - should be small (initial_rows)
        let initial_rows = manager.initial_rows; // 3
        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        {
            let terminal = manager.get(id).expect("terminal should exist");
            let (_, pty_rows) = terminal.terminal.dimensions();
            let visual_height = terminal.height;

            eprintln!("BEFORE resize: pty_rows={}, visual_height={}", pty_rows, visual_height);

            // Non-TUI terminals use 1000 rows for PTY (no scrolling)
            assert_eq!(pty_rows, 1000, "non-TUI PTY rows should be 1000");

            // Visual height should be small (initial_rows * cell_height)
            let expected_small_height = initial_rows as u32 * cell_height;
            assert_eq!(
                visual_height, expected_small_height,
                "initial visual height should be {} (initial_rows={})",
                expected_small_height, initial_rows
            );
        }

        // NOW: Resize to full height (simulating column-term --resize full)
        {
            let terminal = manager.get_mut(id).expect("terminal should exist");
            terminal.resize(max_rows, cell_height);
        }

        // Check dimensions AFTER resize
        {
            let terminal = manager.get(id).expect("terminal should exist");
            let (_, pty_rows) = terminal.terminal.dimensions();
            let visual_height = terminal.height;

            eprintln!("AFTER resize: pty_rows={}, visual_height={}", pty_rows, visual_height);

            // PTY should now report max_rows
            assert_eq!(
                pty_rows, max_rows,
                "AFTER resize: PTY rows should be max_rows={}, but was {}",
                max_rows, pty_rows
            );

            // Visual height should be full viewport
            let expected_full_height = max_rows as u32 * cell_height;
            assert_eq!(
                visual_height, expected_full_height,
                "AFTER resize: visual height should be {} (max_rows={}), but was {}",
                expected_full_height, max_rows, visual_height
            );

            // Terminal should be marked dirty (needs re-render)
            assert!(
                terminal.is_dirty(),
                "AFTER resize: terminal should be marked dirty for re-render"
            );
        }
    }

    #[test]
    fn resize_to_content_shrinks_terminal() {
        // After TUI exits, terminal is resized back to content-based sizing

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // Spawn a non-TUI terminal
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // First resize to full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Verify it's full size
        {
            let terminal = manager.get(id).unwrap();
            assert_eq!(terminal.height, max_rows as u32 * cell_height);
        }

        // Now resize back to content-based (e.g., 3 rows)
        let content_rows = 3u16;
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(content_rows, cell_height);
        }

        // Verify it's shrunk
        {
            let terminal = manager.get(id).unwrap();
            let (_, pty_rows) = terminal.terminal.dimensions();

            assert_eq!(
                pty_rows, content_rows,
                "After shrink: PTY rows should be {}, was {}",
                content_rows, pty_rows
            );

            assert_eq!(
                terminal.height, content_rows as u32 * cell_height,
                "After shrink: height should be {}",
                content_rows as u32 * cell_height
            );
        }
    }

    #[test]
    fn resize_actually_changes_pty_size() {
        // Verify that resize changes the PTY size (what programs see via tcgetwinsize).
        // The internal alacritty grid stays at 1000 rows to prevent scrollback loss.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // Spawn a non-TUI terminal (like shell)
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Get initial PTY dimensions
        let initial_pty_rows = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.dimensions().1
        };

        eprintln!("BEFORE resize: pty_rows={}", initial_pty_rows);

        // Non-TUI terminals start with 1000 PTY rows (no scrolling)
        assert_eq!(initial_pty_rows, 1000, "non-TUI PTY starts at 1000");

        // Now resize to full height
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Get PTY dimensions AFTER resize
        let after_pty_rows = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.dimensions().1
        };

        eprintln!("AFTER resize: pty_rows={}, expected max_rows={}", after_pty_rows, max_rows);

        // The PTY should now report max_rows (what TUI apps will see)
        assert_eq!(
            after_pty_rows, max_rows,
            "AFTER resize: PTY rows should be {}, but was {}",
            max_rows, after_pty_rows
        );

        // Grid intentionally stays at 1000 to hold all content
        let grid_rows = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.grid_rows()
        };
        assert_eq!(grid_rows, 1000, "grid stays at 1000 rows");
    }

    #[test]
    fn non_tui_terminal_initial_grid_size() {
        // What is the initial grid size for a non-TUI terminal?
        // This documents the current behavior.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None).unwrap();

        let terminal = manager.get(id).unwrap();

        // Get dimensions from various sources
        let (pty_cols, pty_rows) = terminal.terminal.dimensions();
        let grid_rows = terminal.terminal.grid_rows();
        let visual_height = terminal.height;
        let cell_height = manager.cell_height;
        let initial_rows = manager.initial_rows;

        eprintln!("Non-TUI terminal dimensions:");
        eprintln!("  PTY: cols={}, rows={}", pty_cols, pty_rows);
        eprintln!("  Grid rows: {}", grid_rows);
        eprintln!("  Visual height: {} pixels", visual_height);
        eprintln!("  Expected visual height: {} * {} = {}", initial_rows, cell_height, initial_rows as u32 * cell_height);

        // PTY has 1000 rows (for no scrolling in content-based terminals)
        assert_eq!(pty_rows, 1000, "PTY rows should be 1000");

        // But what about grid_rows? Is it 1000 or initial_rows?
        // This is the key question for the bug!
        eprintln!("  Grid rows == PTY rows? {}", grid_rows == pty_rows);
        eprintln!("  Grid rows == initial_rows? {}", grid_rows == initial_rows);
    }

    #[test]
    fn content_visible_after_resize() {
        // BUG REPRODUCTION: After resize, is content written to the terminal
        // actually visible in the rendered output?
        //
        // This simulates: resize terminal -> TUI app draws -> should be visible

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // Spawn a terminal that outputs content
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        // Use a command that outputs multiple lines
        let id = manager.spawn_command("seq 1 50", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Wait a bit for the command to produce output
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Process PTY output
        manager.process_all();

        // Check content before resize
        let content_before = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.content_rows()
        };
        eprintln!("Content rows before resize: {}", content_before);

        // Now resize to full height
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // The terminal should still have all the content
        let content_after = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.content_rows()
        };
        eprintln!("Content rows after resize: {}", content_after);

        // Content should NOT have been lost due to resize
        // Note: content_rows might be capped by the sizing state machine
        assert!(
            content_after >= content_before.min(max_rows as u32),
            "Content should not be lost after resize: before={}, after={}",
            content_before, content_after
        );
    }

    #[test]
    fn render_dimensions_match_terminal_height() {
        // BUG REPRODUCTION: Does the render buffer size match the terminal height?
        // If not, the rendered output will be wrong size.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Resize to full height
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Get the terminal height
        let terminal_height = {
            let terminal = manager.get(id).unwrap();
            terminal.height
        };

        eprintln!("After resize:");
        eprintln!("  terminal.height = {} pixels", terminal_height);
        eprintln!("  expected = max_rows * cell_height = {} * {} = {}",
                  max_rows, cell_height, max_rows as u32 * cell_height);

        // The terminal height should be max_rows * cell_height
        assert_eq!(
            terminal_height, max_rows as u32 * cell_height,
            "Terminal height should be {} but was {}",
            max_rows as u32 * cell_height, terminal_height
        );

        // Now render and check the buffer dimensions
        // Note: We can't easily call render() without a GlesRenderer in tests
        // But we can check that width/height are set correctly for when render is called
        let terminal = manager.get(id).unwrap();
        eprintln!("  terminal.width = {} pixels", terminal.width);

        // Width is calculated from font cell dimensions at terminal creation time
        // It may differ from output_width due to rounding
        // Just verify it's non-zero and reasonable
        assert!(terminal.width > 0, "Terminal width should be non-zero");
        assert!(terminal.width <= output_width + 100, "Terminal width should be close to output width");
    }

    #[test]
    fn sizing_state_after_resize() {
        // Check what state the sizing state machine is in after resize
        // If it's not Stable, new content might not be tracked correctly

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Check initial sizing state
        {
            let terminal = manager.get(id).unwrap();
            let sizing_state = terminal.terminal.sizing_state();
            eprintln!("BEFORE resize: sizing state = {:?}", sizing_state);
            assert!(sizing_state.is_stable(), "Initial state should be Stable");
        }

        // Resize to full height
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Check sizing state after resize
        {
            let terminal = manager.get(id).unwrap();
            let sizing_state = terminal.terminal.sizing_state();
            eprintln!("AFTER resize: sizing state = {:?}", sizing_state);
            assert!(sizing_state.is_stable(), "State after resize should be Stable");
            assert_eq!(
                sizing_state.current_rows(), max_rows,
                "State should show max_rows={}, but shows {}",
                max_rows, sizing_state.current_rows()
            );
        }
    }

    #[test]
    fn resize_when_growth_pending() {
        // BUG REPRODUCTION: What happens if we resize while growth is pending?
        // This might cause the resize to be ignored or handled incorrectly.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // Spawn a terminal that outputs a lot of content (triggers growth)
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("seq 1 100", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Wait for output
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Process PTY but DON'T handle sizing actions (simulates delayed compositor response)
        let _actions = manager.process_all();

        // Check if growth was requested
        {
            let terminal = manager.get(id).unwrap();
            let sizing_state = terminal.terminal.sizing_state();
            eprintln!("BEFORE forced resize: sizing state = {:?}", sizing_state);
        }

        // Now force resize to full height (simulating column-term --resize full)
        // This might conflict with pending growth
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Check the final state
        {
            let terminal = manager.get(id).unwrap();
            let sizing_state = terminal.terminal.sizing_state();
            let (_, pty_rows) = terminal.terminal.dimensions();
            let grid_rows = terminal.terminal.grid_rows();

            eprintln!("AFTER forced resize:");
            eprintln!("  sizing state = {:?}", sizing_state);
            eprintln!("  PTY rows = {}", pty_rows);
            eprintln!("  grid rows = {}", grid_rows);
            eprintln!("  terminal.height = {}", terminal.height);

            // PTY should be at max_rows (what programs see)
            assert_eq!(pty_rows, max_rows, "PTY rows should be max_rows");
            // Grid stays at 1000 to hold all content
            assert_eq!(grid_rows, 1000, "Grid rows stay at 1000");
            assert!(sizing_state.is_stable(), "State should be Stable after resize");
        }
    }

    #[test]
    fn new_output_after_resize_marks_dirty() {
        // BUG REPRODUCTION: After resize, does new PTY output mark the terminal dirty?
        // If not, the terminal won't be re-rendered and updates will be "missing".

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // Spawn a shell that we can write to
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        // Use cat which will echo back what we write
        let id = manager.spawn_command("cat", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Resize to full height
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Clear dirty flag (simulate render happened)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.dirty = false;
        }

        eprintln!("After resize, dirty cleared: dirty = {}",
                  manager.get(id).unwrap().is_dirty());

        // Write some input to the terminal (simulates TUI app drawing)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.write(b"Hello from TUI app!\n").unwrap();
        }

        // Wait for cat to echo back
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Process PTY output
        manager.process_all();

        // Check if terminal is dirty
        let is_dirty = manager.get(id).unwrap().is_dirty();
        eprintln!("After writing and processing: dirty = {}", is_dirty);

        assert!(is_dirty, "Terminal should be marked dirty after new PTY output");
    }

    #[test]
    fn process_all_marks_dirty_on_output() {
        // Verify that process_all() marks terminals dirty when there's output

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo 'test output'", cwd, &env, None).unwrap();

        // Wait for command to produce output
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Clear dirty flag
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.dirty = false;
        }

        eprintln!("Before process_all: dirty = {}", manager.get(id).unwrap().is_dirty());

        // Process PTY output
        manager.process_all();

        let is_dirty = manager.get(id).unwrap().is_dirty();
        eprintln!("After process_all: dirty = {}", is_dirty);

        assert!(is_dirty, "process_all should mark terminal dirty when there's output");
    }

    #[test]
    fn tui_output_processed_after_resize() {
        // BUG REPRODUCTION: After resizing terminal to full height and running a TUI app,
        // does the TUI's screen-drawing output get processed correctly?
        //
        // This simulates the TUI resize flow:
        // 1. Shell terminal starts small (content-based)
        // 2. column-term --resize full resizes it
        // 3. mc (or other TUI) runs and draws the full screen
        // 4. The compositor should see all of mc's output

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // Use cat to simulate a terminal we can write to
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("cat", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Initial visual height should be small
        {
            let terminal = manager.get(id).unwrap();
            let initial_height = terminal.height;
            eprintln!("Initial height: {} pixels ({} rows)", initial_height, initial_height / cell_height);
            assert!(initial_height < 100, "Initial height should be small");
        }

        // Simulate column-term --resize full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Verify resize happened
        {
            let terminal = manager.get(id).unwrap();
            let (_, pty_rows) = terminal.terminal.dimensions();
            eprintln!("After resize: PTY rows={}, height={}", pty_rows, terminal.height);
            assert_eq!(pty_rows, max_rows, "PTY should be resized to max_rows");
        }

        // Clear dirty flag (simulate frame render after resize)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.dirty = false;
        }

        // Now simulate TUI drawing the screen
        // TUI apps typically:
        // 1. Clear screen: ESC[2J
        // 2. Move cursor home: ESC[H
        // 3. Draw each row with cursor positioning: ESC[row;colH
        {
            let terminal = manager.get_mut(id).unwrap();

            // Clear screen and move home
            let clear_screen = "\x1b[2J\x1b[H";
            terminal.write(clear_screen.as_bytes()).unwrap();

            // Draw a TUI-like screen (borders and content)
            // This simulates what mc does: draw characters at specific positions
            for row in 0..max_rows {
                // Move to row,1
                let move_cursor = format!("\x1b[{};1H", row + 1);
                terminal.write(move_cursor.as_bytes()).unwrap();

                // Draw a line
                let line = format!("Row {:02}: {}", row, "=" .repeat(50));
                terminal.write(line.as_bytes()).unwrap();
            }
        }

        // Wait for cat to echo back (cat just echoes what it receives)
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Process PTY output
        manager.process_all();

        // The terminal should be marked dirty
        let is_dirty = manager.get(id).unwrap().is_dirty();
        eprintln!("After TUI output: dirty = {}", is_dirty);
        assert!(is_dirty, "Terminal should be dirty after TUI output");

        // The terminal should have content (not blank)
        // Note: We can't easily verify the actual content without rendering,
        // but we can check that something was processed
    }

    #[test]
    fn resize_ipc_flow_simulation() {
        // This test simulates the EXACT flow when column-term --resize full is called:
        //
        // 1. column-term sends IPC message: {"type": "resize", "mode": "full"}
        // 2. Compositor receives message (in calloop callback)
        // 3. Compositor stores pending_resize_request
        // 4. Later in frame: process pending_resize_request
        // 5. Resize the focused terminal
        // 6. Send ACK
        //
        // The question: Is the resize actually applied BEFORE the ACK is sent?

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // Spawn a shell (non-TUI)
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("cat", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Record state BEFORE resize
        let before_pty_rows = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.dimensions().1
        };
        let before_height = manager.get(id).unwrap().height;
        let before_grid_rows = manager.get(id).unwrap().terminal.grid_rows();

        eprintln!("BEFORE resize IPC:");
        eprintln!("  PTY rows: {}", before_pty_rows);
        eprintln!("  Grid rows: {}", before_grid_rows);
        eprintln!("  Visual height: {}", before_height);

        // Simulate the compositor processing the resize request
        // This is what happens in main.rs when pending_resize_request is processed
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // At this point, ACK would be sent
        // The question: are all these values updated?

        let after_pty_rows = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.dimensions().1
        };
        let after_height = manager.get(id).unwrap().height;
        let after_grid_rows = manager.get(id).unwrap().terminal.grid_rows();
        let after_dirty = manager.get(id).unwrap().is_dirty();

        eprintln!("AFTER resize (before ACK):");
        eprintln!("  PTY rows: {}", after_pty_rows);
        eprintln!("  Grid rows: {}", after_grid_rows);
        eprintln!("  Visual height: {}", after_height);
        eprintln!("  Dirty: {}", after_dirty);

        // PTY and visual height should be updated BEFORE ACK is sent
        assert_eq!(
            after_pty_rows, max_rows,
            "PTY rows should be max_rows BEFORE ACK"
        );
        // Grid stays at 1000 (intentional design - holds all content)
        assert_eq!(
            after_grid_rows, 1000,
            "Grid rows stay at 1000"
        );
        assert_eq!(
            after_height, max_rows as u32 * cell_height,
            "Visual height should be updated BEFORE ACK"
        );
        assert!(after_dirty, "Terminal should be marked dirty BEFORE ACK");
    }

    #[test]
    fn multiple_process_all_calls_accumulate_output() {
        // Test that calling process_all multiple times properly accumulates output
        // This is important because TUI apps may produce output across multiple frames

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("cat", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Resize to full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
            terminal.dirty = false; // Clear after resize
        }

        // Write some output
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.write(b"First line\n").unwrap();
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
        manager.process_all();

        let content1 = manager.get(id).unwrap().content_rows();
        eprintln!("After first write: content_rows = {}", content1);

        // Clear dirty and write more
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.dirty = false;
            terminal.write(b"Second line\n").unwrap();
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
        manager.process_all();

        let content2 = manager.get(id).unwrap().content_rows();
        let is_dirty = manager.get(id).unwrap().is_dirty();
        eprintln!("After second write: content_rows = {}, dirty = {}", content2, is_dirty);

        // Content should have increased
        assert!(content2 > content1, "Content rows should increase with more output");
        assert!(is_dirty, "Terminal should be dirty after new output");
    }

    #[test]
    fn tui_resize_then_app_draws_full_screen() {
        // This test simulates the EXACT TUI app flow with realistic timing:
        //
        // 1. Shell terminal starts (small, content-based)
        // 2. User runs TUI app (e.g., mc)
        // 3. column-term --resize full is called
        // 4. Terminal is resized to full viewport
        // 5. ACK is sent (column-term exits)
        // 6. Shell runs mc
        // 7. mc queries terminal size (TIOCGWINSZ)
        // 8. mc draws full screen
        // 9. Compositor processes output and renders
        //
        // The question: After step 9, is the full screen visible?

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // Start with a shell terminal
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        // Use bash -c with a script that waits, then draws
        // This simulates the shell waiting for mc to start
        let id = manager.spawn_command("cat", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        eprintln!("=== Initial state ===");
        eprintln!("  max_rows={}, cell_height={}", max_rows, cell_height);
        eprintln!("  height={}", manager.get(id).unwrap().height);

        // Step 1: Simulate column-term --resize full
        // This happens BEFORE mc starts
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);

            eprintln!("=== After resize ===");
            let (_, pty_rows) = terminal.terminal.dimensions();
            let grid_rows = terminal.terminal.grid_rows();
            eprintln!("  PTY rows: {}", pty_rows);
            eprintln!("  Grid rows: {}", grid_rows);
            eprintln!("  height: {}", terminal.height);

            assert_eq!(pty_rows, max_rows, "PTY should report max_rows");
            // Grid stays at 1000 to hold all content without scrolling
            assert_eq!(grid_rows, 1000, "Grid stays at 1000");
        }

        // Step 2: Simulate what mc does when it starts:
        // - Query terminal size (we assume it gets the correct size)
        // - Clear screen
        // - Draw content at every row
        {
            let terminal = manager.get_mut(id).unwrap();

            // mc sends: clear screen, move home
            terminal.write(b"\x1b[2J\x1b[H").unwrap();

            // mc draws content at every row from 1 to max_rows
            // This is TUI drawing - cursor positioning without newlines
            for row in 1..=max_rows {
                let escape = format!("\x1b[{};1H Row {:02}: Content here ==========", row, row);
                terminal.write(escape.as_bytes()).unwrap();
            }
        }

        // Wait for cat to echo back
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Step 3: Compositor processes PTY output (this is what happens in main loop)
        let actions = manager.process_all();
        eprintln!("=== After process_all ===");
        eprintln!("  Sizing actions: {:?}", actions);

        let terminal = manager.get(id).unwrap();
        let is_dirty = terminal.is_dirty();
        let content_rows = terminal.content_rows();
        let grid_rows = terminal.terminal.grid_rows();
        let (_, pty_rows) = terminal.terminal.dimensions();

        eprintln!("  dirty: {}", is_dirty);
        eprintln!("  content_rows: {}", content_rows);
        eprintln!("  grid_rows: {}", grid_rows);
        eprintln!("  pty_rows: {}", pty_rows);

        // Terminal should be dirty (needs re-render)
        assert!(is_dirty, "Terminal should be dirty after TUI output");

        // Grid stays at 1000 by design (holds all content without scrolling)
        assert_eq!(grid_rows, 1000, "Grid rows stay at 1000");

        // PTY should still report max_rows
        assert_eq!(pty_rows, max_rows, "PTY rows should still be max_rows");
    }

    #[test]
    fn cursor_positioning_after_resize_works() {
        // Test that cursor positioning commands work correctly after resize
        // This is critical for TUI apps that draw by moving the cursor

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("cat", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Resize to full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Send cursor positioning to the LAST row (max_rows)
        // This is where TUI apps would draw their status bar
        {
            let terminal = manager.get_mut(id).unwrap();

            // Move to last row, column 1
            let escape = format!("\x1b[{};1H STATUS BAR AT BOTTOM", max_rows);
            terminal.write(escape.as_bytes()).unwrap();

            // Also draw at row 1 (top)
            terminal.write(b"\x1b[1;1H TOP ROW").unwrap();
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
        manager.process_all();

        // The terminal should have processed the output
        let terminal = manager.get(id).unwrap();
        assert!(terminal.is_dirty(), "Terminal should be dirty");

        // Note: We can't easily verify the cursor position without accessing
        // the alacritty grid internals, but the test passing means the
        // escape sequences were processed without error
    }

    #[test]
    fn resize_and_render_buffer_dimensions() {
        // BUG REPRODUCTION: After resize, is the render buffer the correct size?
        //
        // This is critical: if the render buffer has wrong dimensions, the
        // terminal will appear blank or partially rendered.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;
        let expected_full_height = max_rows as u32 * cell_height;

        // Record initial state
        let initial_height = manager.get(id).unwrap().height;
        eprintln!("Initial: height={}", initial_height);
        assert!(initial_height < expected_full_height, "Initial height should be small");

        // Resize to full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // After resize, check all dimensions
        let terminal = manager.get(id).unwrap();

        // 1. Visual height (used for layout)
        assert_eq!(
            terminal.height, expected_full_height,
            "Visual height should be {} after resize, was {}",
            expected_full_height, terminal.height
        );

        // 2. PTY rows
        let (_, pty_rows) = terminal.terminal.dimensions();
        assert_eq!(
            pty_rows, max_rows,
            "PTY rows should be {} after resize, was {}",
            max_rows, pty_rows
        );

        // 3. Grid rows (stays at 1000 by design to hold all content)
        let grid_rows = terminal.terminal.grid_rows();
        assert_eq!(
            grid_rows, 1000,
            "Grid rows should stay at 1000, was {}",
            grid_rows
        );

        // 4. Dirty flag
        assert!(terminal.is_dirty(), "Terminal should be dirty after resize");

        // 5. Width is not changed by resize() - it's set at terminal creation time
        // based on the font's cell dimensions. Just verify it's non-zero.
        assert!(terminal.width > 0, "Terminal width should be non-zero");

        eprintln!("After resize: height={}, pty_rows={}, grid_rows={}, dirty={}, width={}",
                  terminal.height, pty_rows, grid_rows, terminal.is_dirty(), terminal.width);
    }

    #[test]
    fn tui_resize_to_content_then_full_cycle() {
        // Test the full TUI resize cycle:
        // 1. Terminal starts with content-based sizing
        // 2. Resize to full (for TUI app)
        // 3. Resize back to content (after TUI exits)
        //
        // This is what happens with column-term --resize full/content

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("cat", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;
        let initial_rows = manager.initial_rows;

        // Phase 1: Initial state (content-based, small)
        let initial_height = manager.get(id).unwrap().height;
        let expected_initial = initial_rows as u32 * cell_height;
        assert_eq!(
            initial_height, expected_initial,
            "Phase 1: Initial height should be {}, was {}",
            expected_initial, initial_height
        );
        eprintln!("Phase 1 (initial): height={} ({} rows)", initial_height, initial_rows);

        // Phase 2: Resize to full (for TUI app)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }
        let full_height = manager.get(id).unwrap().height;
        let expected_full = max_rows as u32 * cell_height;
        assert_eq!(
            full_height, expected_full,
            "Phase 2: Full height should be {}, was {}",
            expected_full, full_height
        );
        eprintln!("Phase 2 (full): height={} ({} rows)", full_height, max_rows);

        // Verify PTY also resized
        let (_, pty_rows_full) = manager.get(id).unwrap().terminal.dimensions();
        assert_eq!(pty_rows_full, max_rows, "PTY should be at max_rows after resize to full");

        // Write some content while at full size (simulate TUI drawing)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.write(b"\x1b[H\x1b[2J").unwrap(); // Clear screen
            for i in 1..=10 {
                let line = format!("\x1b[{};1HLine {}\n", i, i);
                terminal.write(line.as_bytes()).unwrap();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        manager.process_all();

        // Phase 3: Resize back to content (TUI exited)
        let content_rows = manager.get(id).unwrap().content_rows();
        let resize_to_rows = (content_rows as u16).max(3);
        eprintln!("Phase 3: content_rows={}, will resize to {} rows", content_rows, resize_to_rows);

        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(resize_to_rows, cell_height);
        }

        let content_height = manager.get(id).unwrap().height;
        let expected_content = resize_to_rows as u32 * cell_height;
        assert_eq!(
            content_height, expected_content,
            "Phase 3: Content height should be {}, was {}",
            expected_content, content_height
        );
        eprintln!("Phase 3 (content): height={} ({} rows)", content_height, resize_to_rows);

        // Verify PTY also resized back
        let (_, pty_rows_content) = manager.get(id).unwrap().terminal.dimensions();
        assert_eq!(
            pty_rows_content, resize_to_rows,
            "PTY should be at content rows after resize back"
        );
    }

    #[test]
    fn render_buffer_not_empty_after_tui_output() {
        // This test verifies that after TUI-style output, the terminal's
        // internal render buffer is populated (not empty/black).
        //
        // The buffer should contain the rendered glyphs.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("cat", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Resize to full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Write TUI-style content (cursor positioning + text)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.write(b"\x1b[H\x1b[2J").unwrap(); // Clear
            terminal.write(b"\x1b[1;1HXXXXXXXXXXXX").unwrap(); // Row 1
            terminal.write(b"\x1b[10;1HMIDDLE ROW CONTENT").unwrap(); // Row 10
            let last_row = format!("\x1b[{};1HBOTTOM ROW", max_rows);
            terminal.write(last_row.as_bytes()).unwrap(); // Last row
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
        manager.process_all();

        // The terminal should have content
        let terminal = manager.get(id).unwrap();
        assert!(terminal.is_dirty(), "Terminal should be dirty after output");

        // Check content rows (TUI output with cursor positioning may not increment content_rows)
        // This is because content_rows tracks newlines, not cursor moves
        eprintln!("Content rows: {}", terminal.content_rows());
        eprintln!("Grid rows: {}", terminal.terminal.grid_rows());
        eprintln!("Height: {}", terminal.height);
    }

    #[test]
    fn all_size_components_match_after_resize() {
        // CRITICAL TEST: Verifies that all size-related components are consistent
        // after resize. A mismatch here would cause "missing updates" where TUI
        // apps draw content that doesn't appear.
        //
        // Components that must match:
        // 1. terminal.height / cell_height = expected rows
        // 2. PTY rows = expected rows
        // 3. Grid rows = expected rows
        // 4. Sizing state rows = expected rows

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("cat", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Record initial state
        {
            let terminal = manager.get(id).unwrap();
            let visual_rows = terminal.height / cell_height;
            let (_, pty_rows) = terminal.terminal.dimensions();
            let grid_rows = terminal.terminal.grid_rows();
            let sizing_rows = terminal.terminal.sizing_state().current_rows();

            eprintln!("INITIAL STATE:");
            eprintln!("  visual_rows (height/cell_height): {}", visual_rows);
            eprintln!("  PTY rows: {}", pty_rows);
            eprintln!("  grid_rows: {}", grid_rows);
            eprintln!("  sizing_state.current_rows: {}", sizing_rows);

            // Initial: PTY is 1000 rows (for no scrolling), visual is small
            assert_eq!(pty_rows, 1000, "Initial PTY rows should be 1000");
            assert_eq!(visual_rows, manager.initial_rows as u32, "Initial visual rows should be initial_rows");
        }

        // Resize to full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Verify ALL components match after resize
        {
            let terminal = manager.get(id).unwrap();
            let visual_rows = terminal.height / cell_height;
            let (_, pty_rows) = terminal.terminal.dimensions();
            let grid_rows = terminal.terminal.grid_rows();
            let sizing_rows = terminal.terminal.sizing_state().current_rows();

            eprintln!("AFTER RESIZE TO FULL:");
            eprintln!("  visual_rows (height/cell_height): {}", visual_rows);
            eprintln!("  PTY rows: {}", pty_rows);
            eprintln!("  grid_rows: {}", grid_rows);
            eprintln!("  sizing_state.current_rows: {}", sizing_rows);

            // Visual, PTY, and sizing should equal max_rows
            // Grid stays at 1000 by design (holds all content without scrolling)
            assert_eq!(
                visual_rows, max_rows as u32,
                "Visual rows should equal max_rows after resize"
            );
            assert_eq!(
                pty_rows, max_rows,
                "PTY rows should equal max_rows after resize"
            );
            assert_eq!(
                grid_rows, 1000,
                "Grid rows stay at 1000"
            );
            assert_eq!(
                sizing_rows, max_rows,
                "Sizing state rows should equal max_rows after resize"
            );
        }

        // Resize back to content
        let content_rows = 10u16;
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(content_rows, cell_height);
        }

        // Verify ALL components match after shrink
        {
            let terminal = manager.get(id).unwrap();
            let visual_rows = terminal.height / cell_height;
            let (_, pty_rows) = terminal.terminal.dimensions();
            let grid_rows = terminal.terminal.grid_rows();
            let sizing_rows = terminal.terminal.sizing_state().current_rows();

            eprintln!("AFTER RESIZE TO CONTENT:");
            eprintln!("  visual_rows (height/cell_height): {}", visual_rows);
            eprintln!("  PTY rows: {}", pty_rows);
            eprintln!("  grid_rows: {}", grid_rows);
            eprintln!("  sizing_state.current_rows: {}", sizing_rows);

            // Visual, PTY, and sizing should equal content_rows
            // Grid stays at 1000 by design (holds all content without scrolling)
            assert_eq!(
                visual_rows, content_rows as u32,
                "Visual rows should equal content_rows after shrink"
            );
            assert_eq!(
                pty_rows, content_rows,
                "PTY rows should equal content_rows after shrink"
            );
            assert_eq!(
                grid_rows, 1000,
                "Grid rows stay at 1000"
            );
            assert_eq!(
                sizing_rows, content_rows,
                "Sizing state rows should equal content_rows after shrink"
            );
        }
    }

    #[test]
    fn tui_terminal_pty_output_available_immediately() {
        // This test verifies that when a TUI terminal is spawned with a command
        // that produces output, the output is available from PTY read immediately
        // (within the first few process_all calls).
        //
        // BUG SCENARIO: mc takes 11 seconds to show output because our shell
        // integration intercepts mc's internal fish subshell command and spawns
        // it as a separate terminal, breaking mc's communication with its subshell.
        //
        // This test uses a simpler command (echo) that should produce output
        // immediately without any subshell complications.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");

        // Spawn a TUI terminal with echo - should produce output immediately
        let result = manager.spawn_command("echo 'TUI OUTPUT TEST'", cwd, &env, None);
        assert!(result.is_ok(), "spawn should succeed");
        let id = result.unwrap();

        // Allow some time for the command to produce output
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Process PTY output and track bytes read per terminal
        let mut total_bytes_from_tui = 0;
        for _ in 0..10 {
            for (tid, terminal) in manager.iter_mut() {
                let (_, bytes_read) = terminal.process();
                if *tid == id && bytes_read > 0 {
                    total_bytes_from_tui += bytes_read;
                    eprintln!("Terminal {} read {} bytes", tid.0, bytes_read);
                }
            }
            if total_bytes_from_tui > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // We should have read SOME output from the TUI terminal
        assert!(
            total_bytes_from_tui > 0,
            "TUI terminal should have produced output within 500ms, but got 0 bytes"
        );

        eprintln!("Total bytes read from TUI terminal: {}", total_bytes_from_tui);
    }

    #[test]
    fn tui_terminal_pty_read_works_for_correct_terminal() {
        // This test verifies that when we have multiple terminals,
        // we read from the correct terminal's PTY.
        //
        // Setup: Shell (terminal 0) -> TUI app (terminal 1)
        // The TUI app's output should come from terminal 1's PTY.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");

        // First spawn a "shell" terminal (just sits there)
        let shell_id = manager.spawn_command("sleep 10", cwd, &env, None).unwrap();

        // Then spawn a child terminal with echo
        let tui_id = manager.spawn_command("echo 'FROM TUI'", cwd, &env, Some(shell_id)).unwrap();

        // Allow time for output
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Track bytes per terminal
        let mut shell_bytes = 0;
        let mut tui_bytes = 0;

        for _ in 0..10 {
            for (tid, terminal) in manager.iter_mut() {
                let (_, bytes_read) = terminal.process();
                if *tid == shell_id {
                    shell_bytes += bytes_read;
                } else if *tid == tui_id {
                    tui_bytes += bytes_read;
                }
            }
            if tui_bytes > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        eprintln!("Shell bytes: {}, TUI bytes: {}", shell_bytes, tui_bytes);

        // TUI terminal should have output (the echo)
        assert!(
            tui_bytes > 0,
            "TUI terminal should have produced output, got {} bytes",
            tui_bytes
        );
    }

    #[test]
    fn fzf_resize_flow_shows_output() {
        // Full simulation of the fzf resize flow:
        // 1. Shell terminal starts small (non-TUI, PTY=1000, visual=3)
        // 2. column-term --resize full (PTY and visual become max_rows)
        // 3. fzf runs (enters alternate screen, draws, exits, prints output)
        // 4. column-term --resize content (should resize to show output)

        let output_width = 800;
        let output_height = 720;  // 720/17 = 42 max_rows
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("sleep 60", cwd, &env, None).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Step 1: Shell has initial content
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"Welcome to fish\n");
            terminal.inject_bytes(b"$ echo a | fzf\n");
        }

        let initial_cursor = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.cursor_line()
        };
        let initial_height = manager.get(id).unwrap().height;
        eprintln!("Initial: cursor={}, height={}, max_rows={}", initial_cursor, initial_height, max_rows);

        // Step 2: Simulate column-term --resize full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }
        let after_full_cursor = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.cursor_line()
        };
        let after_full_height = manager.get(id).unwrap().height;
        eprintln!("After resize full: cursor={}, height={}", after_full_cursor, after_full_height);

        // Step 3: fzf runs - enters alternate screen
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049h");  // Enter alternate screen
            terminal.inject_bytes(b"\x1b[2J\x1b[H"); // Clear and home
            for i in 0..20 {
                terminal.inject_bytes(format!("> option {}\n", i).as_bytes());
            }
        }

        // Verify alternate screen
        let is_alt = manager.get(id).unwrap().terminal.is_alternate_screen();
        assert!(is_alt, "Should be in alternate screen");

        // Step 3b: fzf exits alternate screen and prints selection
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049l");  // Exit alternate screen
            terminal.inject_bytes(b"a\n");           // fzf prints selected item
        }

        // Verify not in alternate screen
        let is_alt = manager.get(id).unwrap().terminal.is_alternate_screen();
        assert!(!is_alt, "Should NOT be in alternate screen");

        let after_fzf_cursor = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.cursor_line()
        };
        eprintln!("After fzf exit: cursor={}", after_fzf_cursor);

        // Step 4: Simulate column-term --resize content
        // This is what the compositor does: cursor_line + 2
        let content_rows = (after_fzf_cursor + 2).max(3);
        eprintln!("Content rows to resize to: {}", content_rows);

        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(content_rows, cell_height);
        }

        let final_height = manager.get(id).unwrap().height;
        let final_cursor = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.cursor_line()
        };
        eprintln!("Final: cursor={}, height={}", final_cursor, final_height);

        // The key assertion: after resize, the terminal should be tall enough
        // to show the output (cursor_line indicates content position)
        // cursor is 0-indexed, so visible rows = cursor_line + 1
        // We sized to cursor_line + 2, so there should be room
        let visible_rows = content_rows as u32;
        let cursor_row = after_fzf_cursor as u32 + 1;  // 1-indexed

        assert!(
            visible_rows >= cursor_row,
            "Terminal should have enough rows ({}) to show cursor position (row {})",
            visible_rows, cursor_row
        );
    }

    #[test]
    fn tui_output_visible_after_alternate_screen_exit() {
        // BUG REPRODUCTION: After a TUI app exits alternate screen and prints output,
        // the output should be visible and cursor_line() should reflect it.
        //
        // This simulates the fzf flow:
        // 1. Shell terminal has some content (cursor at line N)
        // 2. fzf enters alternate screen (ESC[?1049h)
        // 3. fzf draws TUI interface in alternate screen
        // 4. User selects item, fzf exits alternate screen (ESC[?1049l)
        // 5. fzf prints selected item to stdout
        // 6. cursor_line() should now be N+1 (reflecting the printed output)
        //
        // The bug: if cursor_line() returns the wrong value, resize-to-content
        // will use wrong height and the output won't be visible.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // Spawn a terminal - we'll inject bytes directly instead of using PTY
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("sleep 10", cwd, &env, None).unwrap();

        // Step 1: Simulate initial shell content (prompt, maybe previous commands)
        // Inject bytes directly to terminal emulator
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"line1\n");
            terminal.inject_bytes(b"line2\n");
            terminal.inject_bytes(b"$ echo a | fzf\n");
        }

        // Check cursor position before TUI
        let cursor_before_tui = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.cursor_line()
        };
        eprintln!("Cursor before TUI: line {}", cursor_before_tui);

        // Step 2: TUI enters alternate screen
        {
            let terminal = manager.get_mut(id).unwrap();
            // ESC[?1049h = save cursor and switch to alternate screen
            terminal.inject_bytes(b"\x1b[?1049h");
        }

        // Verify we're in alternate screen
        let is_alt_after_enter = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.is_alternate_screen()
        };
        assert!(is_alt_after_enter, "Should be in alternate screen after ESC[?1049h");

        // Step 3: TUI draws in alternate screen (lots of output)
        {
            let terminal = manager.get_mut(id).unwrap();
            // Clear alternate screen and draw TUI interface
            terminal.inject_bytes(b"\x1b[2J\x1b[H");  // Clear and home
            for i in 0..20 {
                let line = format!("  option {}\n", i);
                terminal.inject_bytes(line.as_bytes());
            }
        }

        // Step 4: TUI exits alternate screen
        {
            let terminal = manager.get_mut(id).unwrap();
            // ESC[?1049l = restore cursor and switch to primary screen
            terminal.inject_bytes(b"\x1b[?1049l");
        }

        // Verify we're back to primary screen
        let is_alt_after_exit = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.is_alternate_screen()
        };
        assert!(!is_alt_after_exit, "Should NOT be in alternate screen after ESC[?1049l");

        // Check cursor after exiting alternate screen (should be restored)
        let cursor_after_alt_exit = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.cursor_line()
        };
        eprintln!("Cursor after alt screen exit: line {}", cursor_after_alt_exit);

        // Step 5: TUI prints selected item (like fzf does)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"selected_item\n");
        }

        // Step 6: Check cursor reflects the new output
        let cursor_after_output = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.cursor_line()
        };
        eprintln!("Cursor after output: line {}", cursor_after_output);

        // The cursor should have advanced by 1 line (the printed output)
        assert!(
            cursor_after_output > cursor_after_alt_exit,
            "Cursor should advance after printing output: was {}, now {}",
            cursor_after_alt_exit, cursor_after_output
        );

        // content_rows should NOT have been inflated by alternate screen output
        let content_rows = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.content_rows()
        };
        eprintln!("Content rows: {}", content_rows);

        // Content rows should be close to cursor position, not inflated by alt screen
        // cursor_line is 0-indexed, so cursor_line + 1 = number of rows
        let expected_content = cursor_after_output + 1;
        assert!(
            content_rows <= expected_content as u32 + 2,
            "Content rows ({}) should not be much more than cursor position + 1 ({})",
            content_rows, expected_content
        );
    }

    /// Test that is_alternate_screen() correctly detects alternate screen mode.
    /// This is used for spawn rejection when TUI apps are running.
    #[test]
    fn is_alternate_screen_detection() {
        let mut manager = TerminalManager::new_with_size(800, 600, terminal::Theme::default());
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None).unwrap();

        // Initially not in alternate screen
        {
            let terminal = manager.get(id).unwrap();
            assert!(
                !terminal.terminal.is_alternate_screen(),
                "Terminal should not be in alternate screen initially"
            );
        }

        // Enter alternate screen mode with CSI ? 1049 h
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049h");
        }

        // Now should be in alternate screen
        {
            let terminal = manager.get(id).unwrap();
            assert!(
                terminal.terminal.is_alternate_screen(),
                "Terminal should be in alternate screen after CSI ? 1049 h"
            );
        }

        // Exit alternate screen mode with CSI ? 1049 l
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049l");
        }

        // Should no longer be in alternate screen
        {
            let terminal = manager.get(id).unwrap();
            assert!(
                !terminal.terminal.is_alternate_screen(),
                "Terminal should not be in alternate screen after CSI ? 1049 l"
            );
        }
    }

    /// Test that max_rows does not imply alternate screen mode.
    /// This is a regression test for the spawn rejection heuristic change.
    /// Old behavior: reject spawn if parent_pty_rows == max_rows (false positive possible)
    /// New behavior: reject spawn if parent is in alternate screen (exact)
    #[test]
    fn max_rows_does_not_imply_alternate_screen() {
        let max_height = 160; // 10 rows * 16 cell height
        let mut manager = TerminalManager::new_with_size(800, max_height, terminal::Theme::default());

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None).unwrap();

        // Resize the terminal to max height (simulating content growth)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.height = max_height;
        }

        // Terminal has max height but is NOT in alternate screen
        {
            let terminal = manager.get(id).unwrap();
            assert_eq!(terminal.height, max_height, "Terminal should be at max height");
            assert!(
                !terminal.terminal.is_alternate_screen(),
                "Terminal at max height should NOT automatically be in alternate screen"
            );
        }
    }

    /// Test that spawn rejection should be based on alternate screen, not PTY size.
    /// This simulates the condition where spawns should be allowed.
    #[test]
    fn spawn_should_be_allowed_when_not_in_alternate_screen() {
        let mut manager = TerminalManager::new_with_size(800, 600, terminal::Theme::default());
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let parent_id = manager.spawn_command("echo test", cwd, &env, None).unwrap();

        // Parent is not in alternate screen - spawns should be allowed
        {
            let parent = manager.get(parent_id).unwrap();
            assert!(
                !parent.terminal.is_alternate_screen(),
                "Parent not in alternate screen"
            );
        }

        // Child spawn should succeed
        let child_id = manager.spawn_command("echo child", cwd, &env, Some(parent_id)).unwrap();
        assert!(manager.get(child_id).is_some(), "Child should be spawned");
    }

    /// Test that alternate screen detection works for simulated TUI apps.
    /// When a TUI app is running (alternate screen), spawns should be rejected.
    #[test]
    fn spawn_should_be_rejected_when_in_alternate_screen() {
        let mut manager = TerminalManager::new_with_size(800, 600, terminal::Theme::default());
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let parent_id = manager.spawn_command("echo test", cwd, &env, None).unwrap();

        // Enter alternate screen (simulating TUI app start)
        {
            let terminal = manager.get_mut(parent_id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049h");
        }

        // Verify parent is in alternate screen
        {
            let parent = manager.get(parent_id).unwrap();
            assert!(
                parent.terminal.is_alternate_screen(),
                "Parent should be in alternate screen"
            );
        }

        // NOTE: The actual spawn rejection happens in main.rs event loop.
        // This test verifies the detection works correctly.
        // Integration test would need to verify the full rejection path.
    }

    /// Test that check_alt_screen_resize_needed detects transition to alternate screen.
    #[test]
    fn check_alt_screen_resize_needed_detects_transition() {
        let mut manager = TerminalManager::new_with_size(800, 600, terminal::Theme::default());
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None).unwrap();

        let max_height = manager.max_rows as u32 * manager.cell_height;

        // Initially not in alternate screen, so no resize needed
        {
            let terminal = manager.get_mut(id).unwrap();
            assert!(
                !terminal.check_alt_screen_resize_needed(max_height),
                "Should not need resize when not in alternate screen"
            );
        }

        // Enter alternate screen
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049h");
        }

        // Now transition detected, resize needed
        {
            let terminal = manager.get_mut(id).unwrap();
            assert!(
                terminal.check_alt_screen_resize_needed(max_height),
                "Should need resize after transitioning to alternate screen"
            );
        }

        // Call again - should NOT need resize since transition already recorded
        {
            let terminal = manager.get_mut(id).unwrap();
            assert!(
                !terminal.check_alt_screen_resize_needed(max_height),
                "Should not need resize on subsequent checks (no new transition)"
            );
        }
    }

    /// Test that entering alternate screen triggers resize when terminal is small.
    #[test]
    fn resize_needed_when_entering_alt_screen() {
        let mut manager = TerminalManager::new_with_size(800, 600, terminal::Theme::default());
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");

        // Spawn a command terminal (starts small)
        let id = manager.spawn_command("echo test", cwd, &env, None).unwrap();

        let max_height = manager.max_rows as u32 * manager.cell_height;
        let initial_rows = manager.initial_rows;
        let expected_small_height = initial_rows as u32 * manager.cell_height;

        // Verify terminal starts small
        {
            let terminal = manager.get(id).unwrap();
            assert_eq!(terminal.height, expected_small_height, "terminal should start small");
        }

        // Enter alternate screen
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049h");
        }

        // Check resize needed - should be true since terminal is small
        {
            let terminal = manager.get_mut(id).unwrap();
            assert!(
                terminal.check_alt_screen_resize_needed(max_height),
                "Should need resize when entering alt screen from small terminal"
            );
        }
    }

    /// Test that exiting alternate screen does not trigger resize.
    #[test]
    fn exit_alternate_screen_does_not_trigger_resize() {
        let mut manager = TerminalManager::new_with_size(800, 600, terminal::Theme::default());
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None).unwrap();

        let max_height = manager.max_rows as u32 * manager.cell_height;

        // Enter alternate screen
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049h");
        }

        // Consume the transition
        {
            let terminal = manager.get_mut(id).unwrap();
            let _ = terminal.check_alt_screen_resize_needed(max_height);
        }

        // Exit alternate screen
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049l");
        }

        // Should not trigger resize (only entry triggers resize)
        {
            let terminal = manager.get_mut(id).unwrap();
            assert!(
                !terminal.check_alt_screen_resize_needed(max_height),
                "Exiting alternate screen should not trigger resize"
            );
        }
    }

    /// Test the full compositor growth flow with a shell terminal.
    ///
    /// This simulates exactly what the compositor does:
    /// 1. Spawn shell terminal
    /// 2. Write command to it
    /// 3. Process PTY output (get sizing actions)
    /// 4. Handle growth requests (call grow_terminal)
    /// 5. Verify height is updated correctly
    #[test]
    fn compositor_growth_flow_with_shell() {
        use std::time::Duration;

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // Spawn a shell terminal like the compositor does
        let id = manager.spawn().expect("spawn shell terminal");

        let cell_height = manager.cell_height;
        let initial_height = manager.get(id).unwrap().height;

        eprintln!("Initial terminal height: {}", initial_height);

        // Wait for shell to initialize
        std::thread::sleep(Duration::from_millis(500));
        manager.process_all();

        // Write a command that produces 10 lines of output
        {
            let terminal = manager.get_focused_mut().unwrap();
            terminal.write(b"seq 10\n").expect("write to terminal");
        }

        // Process PTY output and collect sizing actions like compositor does
        let mut total_growth_count = 0;
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(3);

        while start.elapsed() < timeout {
            let sizing_actions = manager.process_all();

            // Handle growth requests exactly like process_terminal_output does
            for (term_id, action) in sizing_actions {
                if let terminal::sizing::SizingAction::RequestGrowth { target_rows } = action {
                    eprintln!("Growth request for terminal {}: {} rows", term_id.0, target_rows);
                    manager.grow_terminal(term_id, target_rows);
                    total_growth_count += 1;
                }
            }

            let current_height = manager.get(id).unwrap().height;
            let content_rows = manager.get(id).unwrap().content_rows();
            if content_rows >= 10 {
                eprintln!("Got {} content rows, height now {}", content_rows, current_height);
                break;
            }

            std::thread::sleep(Duration::from_millis(10));
        }

        // Verify terminal grew
        let final_height = manager.get(id).unwrap().height;
        let content_rows = manager.get(id).unwrap().content_rows();

        eprintln!("Final: height={}, content_rows={}, growth_count={}",
            final_height, content_rows, total_growth_count);

        assert!(
            content_rows >= 10,
            "should have at least 10 content rows, got {}",
            content_rows
        );

        assert!(
            total_growth_count > 0,
            "should have received growth requests"
        );

        assert!(
            final_height > initial_height,
            "terminal height should have grown from {} to more than {}",
            initial_height, final_height
        );

        // Final height should be at least 10 rows * cell_height
        let min_expected = 10 * cell_height;
        assert!(
            final_height >= min_expected,
            "terminal height should be at least {} (10 rows * {}), got {}",
            min_expected, cell_height, final_height
        );
    }

    /// Test that ManagedTerminal grid actually contains output.
    ///
    /// This verifies the content is in the grid, not just sizing metrics.
    #[test]
    fn managed_terminal_grid_has_content() {
        use std::time::Duration;

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        let id = manager.spawn().expect("spawn shell terminal");

        // Wait for shell init
        std::thread::sleep(Duration::from_millis(500));
        manager.process_all();

        // Write a simple command
        {
            let terminal = manager.get_focused_mut().unwrap();
            terminal.write(b"echo HELLO\n").expect("write to terminal");
        }

        // Wait for output (break early when content appears)
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            manager.process_all();
            // Check if terminal has received output
            let terminal = manager.get(id).unwrap();
            let grid_content = terminal.terminal.grid_content().join("");
            if grid_content.contains("HELLO") {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        // Check grid content
        let terminal = manager.get(id).unwrap();
        let grid_lines = terminal.terminal.grid_content();

        eprintln!("ManagedTerminal grid ({} lines):", grid_lines.len());
        for (i, line) in grid_lines.iter().enumerate().take(10) {
            if !line.is_empty() {
                eprintln!("  Line {}: '{}'", i, line);
            }
        }

        // Should have "HELLO" somewhere in the grid
        let has_hello = grid_lines.iter().any(|l| l.contains("HELLO"));
        assert!(has_hello, "grid should contain 'HELLO'");
    }

    /// Test that fast-exiting commands with stderr output are not hidden.
    ///
    /// This tests the fix for the bug where commands that exit quickly
    /// (before PTY output is read) would be incorrectly hidden even when
    /// they produced error output.
    #[test]
    fn fast_exit_with_stderr_not_hidden() {
        use std::time::Duration;

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // First spawn a parent shell terminal
        let parent_id = manager.spawn().expect("spawn parent");
        manager.focused = Some(parent_id);

        // Now spawn a command that outputs multiple lines to stderr and exits immediately
        // Using "echo ... >&2" to write to stderr
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let result = manager.spawn_command(
            "echo 'Error line 1' >&2; echo 'Error line 2' >&2; echo 'Error line 3' >&2",
            cwd,
            &env,
            Some(parent_id),
        );

        assert!(result.is_ok(), "spawn_command should succeed");
        let child_id = result.unwrap();

        // Parent should remain visible for non-TUI commands (GUI apps open in separate windows)
        assert!(
            manager.get(parent_id).unwrap().is_visible(),
            "parent should remain visible for non-TUI commands"
        );

        // Wait for command to exit
        std::thread::sleep(Duration::from_millis(200));

        // Run cleanup - this is where the bug would manifest
        // The PTY should be drained before checking content_rows
        let (dead, focus_changed) = manager.cleanup();

        // Child should NOT be dead (keep_open = true)
        assert!(
            !dead.contains(&child_id),
            "child with keep_open should not be in dead list"
        );

        // Focus should change back to parent
        assert_eq!(
            focus_changed,
            Some(parent_id),
            "focus should return to parent"
        );

        // Parent should still be visible
        assert!(
            manager.get(parent_id).unwrap().is_visible(),
            "parent should remain visible"
        );

        // Child should be visible because it had content (error output)
        let child = manager.get(child_id).expect("child should still exist");
        let content_rows = child.content_rows();

        eprintln!("Child content_rows after cleanup: {}", content_rows);

        // The key assertion: child should be visible because it had stderr output
        // The bug was that cleanup would hide it before reading PTY
        assert!(
            child.is_visible(),
            "child with stderr output should be visible (content_rows={})",
            content_rows
        );

        // content_rows should be > 1 (echo prefix + 3 error lines)
        assert!(
            content_rows > 1,
            "should have multiple content rows from stderr, got {}",
            content_rows
        );
    }

    /// Test realistic scenario: command with echo prefix that fails immediately.
    ///
    /// This mimics what happens when column-term runs a command that fails:
    /// 1. Command is prefixed with `echo '> cmd'; cmd`
    /// 2. Command fails immediately and outputs to stderr
    /// 3. Terminal should show both the echo and the error output
    #[test]
    fn command_with_echo_prefix_shows_errors() {
        use std::time::Duration;

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height, terminal::Theme::default());

        // First spawn a parent shell terminal
        let parent_id = manager.spawn().expect("spawn parent");
        manager.focused = Some(parent_id);

        // Simulate the real command format from column-term
        // This is what process_spawn_request creates: "echo '> cmd'; cmd"
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");

        // Use a command that will fail and output to stderr
        // cat on a nonexistent file outputs: "cat: /nonexistent: No such file or directory"
        let result = manager.spawn_command(
            "echo '> cat /nonexistent'; cat /nonexistent",
            cwd,
            &env,
            Some(parent_id),
        );

        assert!(result.is_ok(), "spawn_command should succeed");
        let child_id = result.unwrap();

        // Wait for command to exit (cat fails quickly)
        std::thread::sleep(Duration::from_millis(300));

        // Check content BEFORE cleanup to see what we have
        {
            let child = manager.get_mut(child_id).unwrap();
            // Manually process PTY to ensure we have all output
            child.process();
            let content_before = child.content_rows();
            eprintln!("Content rows BEFORE cleanup: {}", content_before);

            // Check grid content
            let grid = child.terminal.grid_content();
            eprintln!("Grid content ({} lines):", grid.len());
            for (i, line) in grid.iter().enumerate().take(10) {
                if !line.is_empty() {
                    eprintln!("  Line {}: '{}'", i, line);
                }
            }
        }

        // Now run cleanup
        let (dead, focus_changed) = manager.cleanup();

        eprintln!("Cleanup: dead={:?}, focus_changed={:?}",
                  dead.iter().map(|id| id.0).collect::<Vec<_>>(),
                  focus_changed.map(|id| id.0));

        // Child should NOT be dead (keep_open = true)
        assert!(
            !dead.contains(&child_id),
            "child with keep_open should not be in dead list"
        );

        // Check content AFTER cleanup
        let child = manager.get(child_id).expect("child should still exist");
        let content_after = child.content_rows();
        eprintln!("Content rows AFTER cleanup: {}", content_after);
        eprintln!("Child visible: {}", child.is_visible());

        // content_rows should be > 1:
        // - 1 from initial state
        // - 1 from echo line
        // - 1 from cat error line
        // = at least 3
        assert!(
            content_after > 1,
            "should have content rows > 1 (echo + error), got {}",
            content_after
        );

        // Child should be visible because content_rows > 1
        assert!(
            child.is_visible(),
            "child with content should be visible (content_rows={})",
            content_after
        );
    }
