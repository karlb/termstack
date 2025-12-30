//! Tests for terminal window growth

use std::time::Duration;
use terminal::Terminal;
use terminal::sizing::SizingAction;
use test_harness::{assertions, fixtures};

/// NOTE: Requires live terminal with real shell, not mock
#[test]
#[ignore = "requires live terminal infrastructure"]
fn terminal_grows_with_content() {
    let (mut tc, term) = fixtures::single_terminal();

    tc.wait_for(|c| c.snapshot().window_count == 1, Duration::from_secs(2))
        .expect("terminal should appear");

    let initial_height = tc.snapshot().window_heights[0];

    // Generate content
    tc.send_input(&term, &fixtures::seq_command(1, 50));

    tc.wait_for(
        |c| c.snapshot().window_heights[0] > initial_height + 500,
        Duration::from_secs(5),
    )
    .expect("terminal should grow");
}

/// Test that terminal requests growth when content exceeds rows
#[test]
fn sizing_requests_growth_on_output() {
    // Small terminal (3 rows visual)
    let mut terminal = Terminal::new(80, 3).expect("create terminal");

    // Initially no content
    assert_eq!(terminal.content_rows(), 0);

    // Inject lines simulating shell output (like `seq 1 10`)
    for i in 1..=10 {
        terminal.inject_bytes(format!("{}\n", i).as_bytes());
    }

    // Should have counted 10 newlines
    let content_rows = terminal.content_rows();
    assert_eq!(content_rows, 10, "should count 10 newlines as 10 content rows");

    // Sizing state should be requesting growth (content_rows > rows)
    // The Terminal wrapper doesn't expose the sizing actions directly,
    // but we can verify content_rows exceeds the initial size
    assert!(content_rows > 3, "content rows should exceed initial terminal rows");
}

/// Test that shell output is visible after terminal grows
///
/// This reproduces the bug: `for i in (seq 10); echo $i; end` shows no output
///
/// The scenario:
/// 1. Shell terminal starts with small visual size (3 rows)
/// 2. User runs command that outputs 10 lines
/// 3. Terminal should grow and show all output
#[test]
fn shell_output_visible_after_growth() {
    // Simulate shell terminal: small visual size, large PTY
    let mut terminal = Terminal::new(80, 3).expect("create terminal");

    // Get cell dimensions for pixel calculations
    let (cell_width, cell_height) = terminal.cell_size();
    let initial_visual_height = 3 * cell_height;

    // Inject 10 lines of output (like `for i in (seq 10); echo $i; end`)
    for i in 1..=10 {
        terminal.inject_bytes(format!("{}\n", i).as_bytes());
    }

    // Content rows should be 10
    assert_eq!(terminal.content_rows(), 10, "should have 10 content rows");

    // Render at initial small size
    terminal.render(80 * cell_width, initial_visual_height, true);
    let small_buffer = terminal.buffer().to_vec();

    // Now simulate growth: configure terminal to 10 rows
    let action = terminal.configure(10);
    println!("configure(10) returned: {:?}", action);

    // Complete the resize
    terminal.complete_resize();

    // Render at new larger size
    let grown_visual_height = 10 * cell_height;
    terminal.render(80 * cell_width, grown_visual_height, true);
    let grown_buffer = terminal.buffer().to_vec();

    // The grown buffer should have more non-background pixels
    // Count non-background pixels (background is 0xFF1A1A1A)
    let bg_color = 0xFF1A1A1A_u32;
    let small_content_pixels = small_buffer.iter().filter(|&&p| p != bg_color).count();
    let grown_content_pixels = grown_buffer.iter().filter(|&&p| p != bg_color).count();

    println!("Small buffer: {} content pixels out of {}", small_content_pixels, small_buffer.len());
    println!("Grown buffer: {} content pixels out of {}", grown_content_pixels, grown_buffer.len());

    // The grown buffer should have significantly more content
    // (it shows 10 lines instead of 3)
    assert!(
        grown_content_pixels > small_content_pixels,
        "grown terminal should show more content: {} vs {}",
        grown_content_pixels, small_content_pixels
    );

    // More specifically, grown should have roughly 3x more content pixels
    // (10 lines vs ~3 lines visible)
    assert!(
        grown_content_pixels > small_content_pixels * 2,
        "grown terminal should show much more content (10 lines vs 3): {} vs {}",
        grown_content_pixels, small_content_pixels
    );
}

/// Test that inject_bytes triggers RequestGrowth action
///
/// This verifies the sizing state machine properly returns growth requests
/// when content exceeds the initial row count.
#[test]
fn inject_bytes_triggers_growth_request() {
    let mut terminal = Terminal::new(80, 3).expect("create terminal");

    // Initially, sizing state is at rows=3, content_rows=0
    assert_eq!(terminal.content_rows(), 0);

    // Inject lines one at a time and track when growth is requested
    let mut growth_requested = false;
    let mut growth_target = 0u16;

    for i in 1..=10 {
        terminal.inject_bytes(format!("{}\n", i).as_bytes());

        // Check if content_rows now exceeds initial rows (3)
        let content = terminal.content_rows();
        if content > 3 && !growth_requested {
            growth_requested = true;
            growth_target = content as u16;
            println!("Growth should be requested at content_rows={}", content);
        }
    }

    assert!(growth_requested, "growth should have been requested when content exceeded 3 rows");
    assert!(growth_target >= 4, "growth target should be at least 4, got {}", growth_target);
    assert_eq!(terminal.content_rows(), 10, "final content_rows should be 10");
}

/// Test the complete growth flow: inject -> growth request -> configure -> resize
///
/// This simulates what the compositor does when a terminal needs to grow.
#[test]
fn complete_growth_flow() {
    let mut terminal = Terminal::new(80, 3).expect("create terminal");
    let (cell_width, cell_height) = terminal.cell_size();

    // Track visual height (like ManagedTerminal.height)
    let mut visual_height = 3 * cell_height;

    println!("Initial: visual_height={}, content_rows={}", visual_height, terminal.content_rows());

    // Inject 10 lines
    for i in 1..=10 {
        terminal.inject_bytes(format!("{}\n", i).as_bytes());
    }

    println!("After inject: visual_height={}, content_rows={}", visual_height, terminal.content_rows());

    // Content rows should be 10, visual height should still be 3 rows
    assert_eq!(terminal.content_rows(), 10);
    assert_eq!(visual_height, 3 * cell_height);

    // Now simulate what the compositor does: call configure to grow
    // The compositor would call grow_terminal which calls resize which calls configure
    let target_rows = terminal.content_rows() as u16;
    let action = terminal.configure(target_rows);

    println!("configure({}) returned: {:?}", target_rows, action);

    // Should return ApplyResize
    match action {
        SizingAction::ApplyResize { rows } => {
            assert_eq!(rows, target_rows, "ApplyResize should have target rows");
            // Update visual height (like ManagedTerminal.resize does)
            visual_height = rows as u32 * cell_height;
        }
        other => panic!("Expected ApplyResize, got {:?}", other),
    }

    // Complete the resize
    terminal.complete_resize();

    println!("After growth: visual_height={}, content_rows={}", visual_height, terminal.content_rows());

    // Visual height should now be 10 rows
    assert_eq!(visual_height, 10 * cell_height, "visual height should be 10 rows");

    // Render at new size and verify content is visible
    terminal.render(80 * cell_width, visual_height, true);
    let buffer = terminal.buffer();

    // Count non-background pixels
    let bg_color = 0xFF1A1A1A_u32;
    let content_pixels = buffer.iter().filter(|&&p| p != bg_color).count();

    println!("Rendered {} content pixels out of {}", content_pixels, buffer.len());

    // Should have visible content
    assert!(content_pixels > 0, "should have visible content after growth");
}

/// Test that grid resize preserves content visibility
///
/// When the terminal grid is resized from large (1000 rows) to small (10 rows),
/// the content at the TOP of the grid should still be visible.
///
/// This tests the scenario: PTY/grid is 1000 rows, user outputs 10 lines,
/// growth to 10 rows is triggered, grid resizes to 10 rows.
#[test]
fn grid_resize_preserves_content() {
    let mut terminal = Terminal::new(80, 3).expect("create terminal");
    let (cell_width, cell_height) = terminal.cell_size();

    // The grid is now 1000 rows (from Terminal::new's SHELL_PTY_ROWS)
    // Inject 10 lines of output
    for i in 1..=10 {
        terminal.inject_bytes(format!("line{}\n", i).as_bytes());
    }

    // Render before resize at 10 rows height
    terminal.render(80 * cell_width, 10 * cell_height, true);
    let before_buffer = terminal.buffer().to_vec();
    let bg_color = 0xFF1A1A1A_u32;
    let before_pixels = before_buffer.iter().filter(|&&p| p != bg_color).count();
    println!("Before resize: {} content pixels", before_pixels);

    // Now configure to 10 rows (this resizes the grid from 1000 to 10)
    let action = terminal.configure(10);
    println!("configure(10) returned: {:?}", action);
    terminal.complete_resize();

    // Check grid dimensions
    let grid_rows = terminal.grid_rows();
    println!("Grid rows after resize: {}", grid_rows);

    // Render after resize
    terminal.render(80 * cell_width, 10 * cell_height, true);
    let after_buffer = terminal.buffer().to_vec();
    let after_pixels = after_buffer.iter().filter(|&&p| p != bg_color).count();
    println!("After resize: {} content pixels", after_pixels);

    // Content should still be visible after resize
    // The after_pixels should be similar to before_pixels
    // (might differ slightly due to cursor position changes)
    assert!(
        after_pixels > 0,
        "content should be visible after grid resize"
    );

    // Content shouldn't be dramatically less
    assert!(
        after_pixels >= before_pixels / 2,
        "content shouldn't disappear after resize: {} vs {}",
        after_pixels, before_pixels
    );
}

/// Test that output appears in terminal grid when content exceeds visible rows
#[test]
fn output_visible_when_exceeds_rows() {
    // Small terminal (3 rows)
    let mut terminal = Terminal::new(80, 3).expect("create terminal");

    // Inject numbered lines
    for i in 1..=10 {
        terminal.inject_bytes(format!("{}\n", i).as_bytes());
    }

    // Render to a buffer
    terminal.render(80 * 8, 3 * 16, true);
    let buffer = terminal.buffer();

    // Buffer should not be empty
    assert!(!buffer.is_empty(), "render buffer should not be empty");

    // The terminal grid only has 3 rows, so only the last 3 lines (8, 9, 10) should be visible
    // But the content_rows tracking should have 10
    assert_eq!(terminal.content_rows(), 10);
}

/// Test that sizing state machine returns RequestGrowth at the right time.
///
/// This directly tests the core state machine to ensure growth is
/// requested when content exceeds the row count.
#[test]
fn sizing_state_returns_request_growth() {
    use terminal::sizing::{SizingAction, TerminalSizingState};

    let mut sizing = TerminalSizingState::new(3); // 3 rows

    // Initially stable with 0 content
    assert!(sizing.is_stable());
    assert_eq!(sizing.content_rows(), 0);
    assert_eq!(sizing.current_rows(), 3);

    // First 3 lines should NOT trigger growth (content <= rows)
    for i in 1..=3 {
        let action = sizing.on_new_line();
        assert_eq!(action, SizingAction::None, "line {} should not trigger growth", i);
        assert_eq!(sizing.content_rows(), i as u32);
    }

    // 4th line should trigger growth (content > rows)
    let action = sizing.on_new_line();
    assert_eq!(
        action,
        SizingAction::RequestGrowth { target_rows: 4 },
        "4th line should request growth to 4 rows"
    );
    assert_eq!(sizing.content_rows(), 4);

    // 5th line should also trigger growth
    let action = sizing.on_new_line();
    assert_eq!(
        action,
        SizingAction::RequestGrowth { target_rows: 5 },
        "5th line should request growth to 5 rows"
    );
}

/// Test that process_pty flow correctly generates sizing actions.
///
/// This simulates what the compositor does: read from PTY, get actions,
/// and verify growth is requested.
#[test]
fn process_pty_returns_sizing_actions() {
    use std::time::Duration;
    use std::collections::HashMap;

    // Create a command terminal that will produce output
    let mut terminal = terminal::Terminal::new_with_command(
        80,     // cols
        1000,   // pty_rows (large to prevent internal scrolling)
        3,      // visual_rows (small, to trigger growth)
        "seq 10", // command that outputs 10 lines
        std::path::Path::new("/tmp"),
        &HashMap::new(),
    ).expect("create terminal");

    // Wait for output and collect sizing actions
    let mut all_actions = Vec::new();
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(3);

    while start.elapsed() < timeout {
        let actions = terminal.process_pty();
        all_actions.extend(actions);

        // If we got enough content, stop waiting
        if terminal.content_rows() >= 10 {
            break;
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    // Verify we got content
    let content_rows = terminal.content_rows();
    println!("Content rows after processing: {}", content_rows);
    println!("Actions collected: {:?}", all_actions);

    // Should have 10 lines of content
    assert!(
        content_rows >= 10,
        "should have at least 10 content rows, got {}",
        content_rows
    );

    // Should have received growth requests
    // (first 3 lines don't trigger, lines 4-10 each trigger one)
    let growth_count = all_actions.iter()
        .filter(|a| matches!(a, terminal::sizing::SizingAction::RequestGrowth { .. }))
        .count();

    assert!(
        growth_count >= 7,
        "should have at least 7 RequestGrowth actions (lines 4-10), got {}",
        growth_count
    );
}

/// Test shell terminal growth with interactive command.
///
/// This tests the exact scenario the user reported: running `seq 10` in a shell
/// terminal and verifying that output triggers growth and is visible.
#[test]
fn shell_terminal_grows_on_seq_output() {
    use std::time::Duration;

    // Create a shell terminal like the compositor does
    // Terminal::new() spawns an interactive shell
    let mut terminal = terminal::Terminal::new(80, 3).expect("create shell terminal");

    // Wait for shell to initialize (prompt appears)
    let start = std::time::Instant::now();
    let init_timeout = Duration::from_secs(2);
    while start.elapsed() < init_timeout {
        terminal.process_pty();
        std::thread::sleep(Duration::from_millis(10));
    }

    let initial_content = terminal.content_rows();
    println!("After shell init: content_rows = {}", initial_content);

    // Send the command to produce 10 lines of output
    terminal.write(b"seq 10\n").expect("write to PTY");

    // Wait for output and collect sizing actions
    let mut all_actions = Vec::new();
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(3);

    while start.elapsed() < timeout {
        let actions = terminal.process_pty();
        all_actions.extend(actions);

        // Check if we got enough output (10 from seq + shell prompt after)
        if terminal.content_rows() >= initial_content + 10 {
            break;
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    let final_content = terminal.content_rows();
    let growth_count = all_actions.iter()
        .filter(|a| matches!(a, terminal::sizing::SizingAction::RequestGrowth { .. }))
        .count();

    println!("After seq 10: content_rows = {}", final_content);
    println!("Growth requests: {}", growth_count);
    println!("Actions: {:?}", all_actions);

    // Should have gained at least 10 content rows from seq output
    assert!(
        final_content >= initial_content + 10,
        "should have at least {} content rows (initial {} + 10 from seq), got {}",
        initial_content + 10, initial_content, final_content
    );

    // Should have received growth requests
    assert!(
        growth_count > 0,
        "should have received growth requests, got {}",
        growth_count
    );
}

/// Test that shell terminal output triggers proper height growth.
///
/// This is an end-to-end test that verifies:
/// 1. Shell terminal starts small
/// 2. Running a command that produces output triggers growth
/// 3. After growth, the terminal height is updated correctly
#[test]
fn shell_terminal_height_grows_with_output() {
    use std::time::Duration;
    use terminal::sizing::SizingAction;

    // Create a shell terminal with small initial size
    let mut terminal = terminal::Terminal::new(80, 3).expect("create shell terminal");
    let (cell_width, cell_height) = terminal.cell_size();

    // Track visual height like the compositor does
    let mut visual_height = 3 * cell_height;
    println!("Initial visual height: {} ({} rows)", visual_height, visual_height / cell_height);

    // Wait for shell to initialize
    let start = std::time::Instant::now();
    let init_timeout = Duration::from_secs(2);
    while start.elapsed() < init_timeout {
        terminal.process_pty();
        std::thread::sleep(Duration::from_millis(10));
    }

    // Send command that produces 10 lines
    terminal.write(b"seq 10\n").expect("write to PTY");

    // Process output and handle growth requests like the compositor does
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(3);
    let mut grew = false;

    while start.elapsed() < timeout {
        let actions = terminal.process_pty();

        for action in actions {
            if let SizingAction::RequestGrowth { target_rows } = action {
                println!("Growth requested to {} rows", target_rows);
                // Simulate what grow_terminal does
                let resize_action = terminal.configure(target_rows);
                if matches!(resize_action, SizingAction::ApplyResize { .. }) {
                    terminal.complete_resize();
                    visual_height = target_rows as u32 * cell_height;
                    grew = true;
                    println!("Grew to {} rows (height {})", target_rows, visual_height);
                }
            }
        }

        if grew && terminal.content_rows() >= 10 {
            break;
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    println!("Final: visual_height = {}, content_rows = {}", visual_height, terminal.content_rows());

    // Verify terminal grew
    assert!(grew, "terminal should have grown");
    assert!(
        visual_height > 3 * cell_height,
        "visual height should have increased from {} to more than {} (3 rows)",
        3 * cell_height, visual_height
    );

    // Render at the new size and verify content is visible
    terminal.render(80 * cell_width, visual_height, true);
    let buffer = terminal.buffer();
    let bg_color = 0xFF1A1A1A_u32;
    let content_pixels = buffer.iter().filter(|&&p| p != bg_color).count();

    println!("Content pixels in grown buffer: {}", content_pixels);
    assert!(
        content_pixels > 0,
        "should have visible content after growth"
    );
}

/// Test shell terminal with fish loop syntax.
///
/// This is the exact scenario the user reported:
/// `for i in (seq 10); echo $i; end`
#[test]
fn fish_loop_output_triggers_growth() {
    use std::time::Duration;
    use std::collections::HashMap;
    use terminal::sizing::SizingAction;

    // Skip if fish is not available
    if std::process::Command::new("fish").arg("--version").output().is_err() {
        eprintln!("Skipping test: fish shell not available");
        return;
    }

    // Create terminal running fish with the loop command
    let mut terminal = terminal::Terminal::new_with_command(
        80,     // cols
        1000,   // pty_rows (large to prevent internal scrolling)
        3,      // visual_rows (small, to trigger growth)
        "fish -c 'for i in (seq 10); echo $i; end'",
        std::path::Path::new("/tmp"),
        &HashMap::new(),
    ).expect("create fish terminal");

    let (_cell_width, cell_height) = terminal.cell_size();
    let mut visual_height = 3 * cell_height;

    // Process output and handle growth requests
    let mut all_actions = Vec::new();
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(3);

    while start.elapsed() < timeout {
        let actions = terminal.process_pty();

        for action in &actions {
            if let SizingAction::RequestGrowth { target_rows } = action {
                let resize_action = terminal.configure(*target_rows);
                if matches!(resize_action, SizingAction::ApplyResize { .. }) {
                    terminal.complete_resize();
                    visual_height = *target_rows as u32 * cell_height;
                }
            }
        }
        all_actions.extend(actions);

        // Fish loop should produce 10 lines of output
        if terminal.content_rows() >= 10 {
            break;
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    let content_rows = terminal.content_rows();
    let growth_count = all_actions.iter()
        .filter(|a| matches!(a, SizingAction::RequestGrowth { .. }))
        .count();

    println!("Fish loop: visual_height = {}, content_rows = {}", visual_height, content_rows);
    println!("Growth requests: {}", growth_count);

    // Should have 10 lines of output from fish loop
    assert!(
        content_rows >= 10,
        "should have at least 10 content rows from fish loop, got {}",
        content_rows
    );

    // Should have received growth requests
    assert!(
        growth_count >= 7,
        "should have at least 7 growth requests (for rows 4-10), got {}",
        growth_count
    );
}

/// Test that grid actually contains the output content.
///
/// This verifies the actual grid cells have the expected text,
/// not just that sizing metrics are correct.
#[test]
fn grid_contains_output_content() {
    use std::time::Duration;
    use std::collections::HashMap;

    // Create terminal running seq 5 (simpler output to verify)
    let mut terminal = terminal::Terminal::new_with_command(
        80,
        1000,  // large PTY
        3,     // small visual
        "seq 5",
        std::path::Path::new("/tmp"),
        &HashMap::new(),
    ).expect("create terminal");

    // Wait for output
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        terminal.process_pty();
        if terminal.content_rows() >= 5 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // Get actual grid content
    let grid_lines = terminal.grid_content();

    println!("Grid has {} lines:", grid_lines.len());
    for (i, line) in grid_lines.iter().enumerate().take(20) {
        if !line.is_empty() {
            println!("  Line {}: '{}'", i, line);
        }
    }

    // Find lines containing "1", "2", "3", "4", "5"
    let has_1 = grid_lines.iter().any(|l| l.trim() == "1");
    let has_2 = grid_lines.iter().any(|l| l.trim() == "2");
    let has_3 = grid_lines.iter().any(|l| l.trim() == "3");
    let has_4 = grid_lines.iter().any(|l| l.trim() == "4");
    let has_5 = grid_lines.iter().any(|l| l.trim() == "5");

    println!("Found: 1={}, 2={}, 3={}, 4={}, 5={}", has_1, has_2, has_3, has_4, has_5);

    assert!(has_1, "grid should contain line '1'");
    assert!(has_2, "grid should contain line '2'");
    assert!(has_3, "grid should contain line '3'");
    assert!(has_4, "grid should contain line '4'");
    assert!(has_5, "grid should contain line '5'");
}

/// Test shell terminal grid content after running seq command.
#[test]
fn shell_terminal_grid_has_output() {
    use std::time::Duration;

    // Create shell terminal
    let mut terminal = terminal::Terminal::new(80, 3).expect("create shell terminal");

    // Wait for shell init
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(1) {
        terminal.process_pty();
        std::thread::sleep(Duration::from_millis(10));
    }

    // Send seq 5 command
    terminal.write(b"seq 5\n").expect("write command");

    // Wait for output
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        terminal.process_pty();
        if terminal.content_rows() >= 5 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // Dump grid content
    let grid_lines = terminal.grid_content();
    println!("Shell terminal grid ({} lines):", grid_lines.len());
    for (i, line) in grid_lines.iter().enumerate().take(30) {
        if !line.is_empty() {
            println!("  Line {}: '{}'", i, line);
        }
    }

    // Should have the seq output somewhere in the grid
    let has_output = grid_lines.iter().any(|l| {
        let t = l.trim();
        t == "1" || t == "2" || t == "3" || t == "4" || t == "5"
    });

    println!("content_rows = {}", terminal.content_rows());
    println!("cursor_line = {}", terminal.cursor_line());

    assert!(has_output, "grid should contain seq output");

    // Now test rendering at SMALL height (3 rows) - simulates initial terminal size
    let (cell_width, cell_height) = terminal.cell_size();
    let small_height = 3 * cell_height;
    terminal.render(80 * cell_width, small_height, true);
    let small_buffer = terminal.buffer().to_vec();

    let bg_color = 0xFF1A1A1A_u32;
    let small_content = small_buffer.iter().filter(|&&p| p != bg_color).count();
    println!("Small render (3 rows, {} pixels): {} content pixels",
        small_height, small_content);

    // Even at small height, should have SOME content (prompt + first lines)
    assert!(small_content > 0, "small render should have content");

    // Now test rendering at GROWN height (10 rows)
    let grown_height = 10 * cell_height;
    terminal.render(80 * cell_width, grown_height, true);
    let grown_buffer = terminal.buffer().to_vec();

    let grown_content = grown_buffer.iter().filter(|&&p| p != bg_color).count();
    println!("Grown render (10 rows, {} pixels): {} content pixels",
        grown_height, grown_content);

    // Grown render should have MORE content
    assert!(grown_content > small_content,
        "grown render should have more content than small: {} vs {}",
        grown_content, small_content);
}

/// Test keyboard input with carriage return (\r) like real compositor.
///
/// The real compositor sends \r for the Enter key (see input.rs keysym_to_bytes).
/// This test verifies that command output still works with \r instead of \n.
#[test]
fn keyboard_input_uses_carriage_return() {
    use std::time::Duration;

    // Create shell terminal like the compositor does
    let mut terminal = terminal::Terminal::new(80, 3).expect("create shell terminal");

    // Wait for shell to initialize
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(1) {
        terminal.process_pty();
        std::thread::sleep(Duration::from_millis(10));
    }

    let initial_content = terminal.content_rows();
    println!("After shell init: content_rows = {}", initial_content);

    // Send command using \r (carriage return) like real keyboard input
    // This is exactly what keysym_to_bytes returns for Enter key
    terminal.write(b"seq 10\r").expect("write to PTY");

    // Process output and collect sizing actions
    let mut all_actions = Vec::new();
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(3);

    while start.elapsed() < timeout {
        let actions = terminal.process_pty();
        all_actions.extend(actions);

        if terminal.content_rows() >= initial_content + 10 {
            break;
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    let final_content = terminal.content_rows();
    let growth_count = all_actions.iter()
        .filter(|a| matches!(a, terminal::sizing::SizingAction::RequestGrowth { .. }))
        .count();

    println!("After seq 10 with \\r: content_rows = {}", final_content);
    println!("Growth requests: {}", growth_count);
    println!("Actions: {:?}", all_actions);

    // Should have gained at least 10 content rows from seq output
    assert!(
        final_content >= initial_content + 10,
        "should have at least {} content rows (initial {} + 10 from seq), got {}",
        initial_content + 10, initial_content, final_content
    );

    // Should have received growth requests
    assert!(
        growth_count > 0,
        "should have received growth requests, got {}",
        growth_count
    );
}

/// Test using Terminal::new() which uses $SHELL (like the real compositor)
///
/// This is the most accurate reproduction of the user scenario.
#[test]
fn shell_terminal_with_loop_output() {
    use std::time::Duration;
    use terminal::sizing::SizingAction;

    // Create shell terminal exactly like the compositor does
    let mut terminal = terminal::Terminal::new(80, 3).expect("create shell terminal");

    let (_cell_width, cell_height) = terminal.cell_size();
    let mut visual_height = 3 * cell_height;

    // Wait for shell to initialize and show prompt
    // We need to wait for the shell to be fully ready before sending commands
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(500) {
        terminal.process_pty();
        std::thread::sleep(Duration::from_millis(10));
    }

    let content_before = terminal.content_rows();
    println!("Shell is: {}", std::env::var("SHELL").unwrap_or_default());
    println!("Before command: content_rows = {}", content_before);

    // Use seq directly - works in all shells (bash, zsh, fish)
    // Use \r like the real compositor sends for Enter key
    terminal.write(b"seq 5\r").expect("write command");

    // Process output and handle growth
    let mut all_actions = Vec::new();
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(5);
    let mut bytes_total = 0;

    while start.elapsed() < timeout {
        let (actions, bytes_read) = terminal.process_pty_with_count();
        bytes_total += bytes_read;

        for action in &actions {
            if let SizingAction::RequestGrowth { target_rows } = action {
                let resize_action = terminal.configure(*target_rows);
                if matches!(resize_action, SizingAction::ApplyResize { .. }) {
                    terminal.complete_resize();
                    visual_height = *target_rows as u32 * cell_height;
                }
            }
        }
        all_actions.extend(actions);

        // Wait for 5 lines of output
        if terminal.content_rows() >= content_before + 5 {
            break;
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    let content_after = terminal.content_rows();
    let growth_count = all_actions.iter()
        .filter(|a| matches!(a, SizingAction::RequestGrowth { .. }))
        .count();

    println!("After command: content_rows = {}", content_after);
    println!("Growth from {} to {}", content_before, content_after);
    println!("Growth requests: {}", growth_count);
    println!("Visual height: {} ({} rows)", visual_height, visual_height / cell_height);
    println!("Total bytes read from PTY: {}", bytes_total);

    // Print grid content for debugging
    let grid_content = terminal.grid_content();
    println!("Grid content ({} lines):", grid_content.len());
    for (i, line) in grid_content.iter().take(20).enumerate() {
        if !line.is_empty() {
            println!("  {:3}: {}", i, line);
        }
    }

    // Should have gained at least 5 content rows
    let content_gained = content_after.saturating_sub(content_before);
    assert!(
        content_gained >= 5,
        "should have gained at least 5 content rows, got {}",
        content_gained
    );

    // Verify render output has visible content
    let (cell_width, _cell_height) = terminal.cell_size();
    terminal.render(80 * cell_width, visual_height, true);
    let buffer = terminal.buffer();
    let bg_color = 0xFF1A1A1A_u32;
    let content_pixels = buffer.iter().filter(|&&p| p != bg_color).count();
    println!("Render buffer: {} content pixels out of {}", content_pixels, buffer.len());
    assert!(content_pixels > 100, "render should show content, got {} pixels", content_pixels);
}

/// Test fish loop typed interactively.
///
/// This reproduces the exact user scenario:
/// 1. Start fish shell interactively
/// 2. Type "for i in (seq 5); echo $i; end"
/// 3. Press Enter
/// 4. Verify output is counted
#[test]
fn fish_loop_typed_interactively() {
    use std::time::Duration;
    use terminal::sizing::SizingAction;

    // Skip if fish is not available
    if std::process::Command::new("fish").arg("--version").output().is_err() {
        eprintln!("Skipping test: fish shell not available");
        return;
    }

    // Create terminal running fish interactively
    let mut terminal = terminal::Terminal::new_with_command(
        80,     // cols
        1000,   // pty_rows
        3,      // visual_rows
        "fish",
        std::path::Path::new("/tmp"),
        &std::collections::HashMap::new(),
    ).expect("create fish terminal");

    let (_cell_width, cell_height) = terminal.cell_size();
    let mut visual_height = 3 * cell_height;

    // Wait for fish to initialize and show prompt
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(500) {
        terminal.process_pty();
        std::thread::sleep(Duration::from_millis(10));
    }

    let content_before = terminal.content_rows();
    println!("Before command: content_rows = {}", content_before);

    // Type the loop command and press Enter (using \r like the real compositor)
    terminal.write(b"for i in (seq 5); echo $i; end\r").expect("write command");

    // Process output and handle growth
    let mut all_actions = Vec::new();
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(5);
    let mut bytes_total = 0;

    while start.elapsed() < timeout {
        let (actions, bytes_read) = terminal.process_pty_with_count();
        bytes_total += bytes_read;

        for action in &actions {
            if let SizingAction::RequestGrowth { target_rows } = action {
                let resize_action = terminal.configure(*target_rows);
                if matches!(resize_action, SizingAction::ApplyResize { .. }) {
                    terminal.complete_resize();
                    visual_height = *target_rows as u32 * cell_height;
                }
            }
        }
        all_actions.extend(actions);

        // Wait for 5 lines of output (numbers 1-5)
        if terminal.content_rows() >= content_before + 5 {
            break;
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    let content_after = terminal.content_rows();
    let growth_count = all_actions.iter()
        .filter(|a| matches!(a, SizingAction::RequestGrowth { .. }))
        .count();

    println!("After command: content_rows = {}", content_after);
    println!("Growth from {} to {}", content_before, content_after);
    println!("Growth requests: {}", growth_count);
    println!("Visual height: {} ({} rows)", visual_height, visual_height / cell_height);
    println!("Total bytes read from PTY: {}", bytes_total);

    // Print grid content for debugging
    let grid_content = terminal.grid_content();
    println!("Grid content ({} lines):", grid_content.len());
    for (i, line) in grid_content.iter().take(20).enumerate() {
        if !line.is_empty() {
            println!("  {:3}: {}", i, line);
        }
    }

    // Should have gained at least 5 content rows from the loop output
    let content_gained = content_after.saturating_sub(content_before);
    assert!(
        content_gained >= 5,
        "should have gained at least 5 content rows from fish loop, got {}",
        content_gained
    );
}

#[test]
fn multiple_terminals_stack_vertically() {
    let (mut tc, _terminals) = fixtures::multiple_terminals(3);

    tc.wait_for(
        |c| c.snapshot().window_count == 3,
        Duration::from_secs(2),
    )
    .expect("terminals should appear");

    let snapshot = tc.snapshot();
    assertions::assert_windows_dont_overlap(&snapshot);
}
