//! Tests for CLI subcommands and fish integration

#[cfg(test)]
mod tests {
    #[test]
    fn gui_subcommand_requires_socket() {
        // When gui subcommand is used without TERMSTACK_SOCKET,
        // it should error with a helpful message
        use std::process::Command;

        let bin_path = find_column_term_binary();
        let bin_path = match bin_path {
            Some(p) => p,
            None => {
                eprintln!("Skipping test: can't find termstack binary");
                return;
            }
        };

        // Run termstack gui without TERMSTACK_SOCKET
        let output = Command::new(&bin_path)
            .args(["gui", "pqiv", "image.png"])
            .env_remove("TERMSTACK_SOCKET")
            .output();

        match output {
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                assert!(
                    !o.status.success(),
                    "gui should fail without TERMSTACK_SOCKET"
                );
                assert!(
                    stderr.contains("TERMSTACK_SOCKET") || stderr.contains("termstack"),
                    "error should mention TERMSTACK_SOCKET, got: {}",
                    stderr
                );
            }
            Err(e) => {
                eprintln!("Skipping test: failed to run termstack: {}", e);
            }
        }
    }

    #[test]
    fn gui_subcommand_requires_command_arg() {
        // gui subcommand needs at least one argument (the command to run)
        use std::process::Command;

        let bin_path = find_column_term_binary();
        let bin_path = match bin_path {
            Some(p) => p,
            None => {
                eprintln!("Skipping test: can't find termstack binary");
                return;
            }
        };

        // Run termstack gui with no command
        let output = Command::new(&bin_path)
            .args(["gui"])
            .env("TERMSTACK_SOCKET", "/tmp/fake")
            .output();

        match output {
            Ok(o) => {
                assert!(
                    !o.status.success(),
                    "gui without command should fail"
                );
                let stderr = String::from_utf8_lossy(&o.stderr);
                assert!(
                    stderr.contains("usage") || stderr.contains("command"),
                    "error should mention usage, got: {}",
                    stderr
                );
            }
            Err(e) => {
                eprintln!("Skipping test: failed to run termstack: {}", e);
            }
        }
    }

    #[test]
    fn gui_subcommand_strips_gui_prefix() {
        // When using 'termstack gui swayimg image.png', the command sent to the
        // compositor should be 'swayimg image.png' (without 'gui' prefix).
        // This test verifies the subcommand detection works by checking debug output.
        use std::process::Command;

        let bin_path = find_column_term_binary();
        let bin_path = match bin_path {
            Some(p) => p,
            None => {
                eprintln!("Skipping test: can't find termstack binary");
                return;
            }
        };

        // Run termstack gui with debug enabled - it will fail to connect but
        // we can check the debug output to see what command it would send
        let output = Command::new(&bin_path)
            .args(["gui", "swayimg", "image.png"])
            .env("DEBUG_TSTACK", "1")
            .env("TERMSTACK_SOCKET", "/tmp/nonexistent-socket-12345")
            .output();

        match output {
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                // Should see "gui spawn: command=" with the command WITHOUT "gui" prefix
                assert!(
                    stderr.contains("gui spawn: command=\"swayimg image.png\"") ||
                    stderr.contains("gui spawn: command=\\\"swayimg image.png\\\""),
                    "gui subcommand should strip 'gui' prefix from command. stderr: {}",
                    stderr
                );
                // Should NOT see "spawning in terminal" (that would mean regular spawn path)
                assert!(
                    !stderr.contains("spawning in terminal"),
                    "should use gui_spawn path, not regular spawn. stderr: {}",
                    stderr
                );
            }
            Err(e) => {
                eprintln!("Skipping test: failed to run termstack: {}", e);
            }
        }
    }

    #[test]
    fn fish_integration_function_defined_with_socket() {
        // Verify the fish integration script defines termstack_exec when TERMSTACK_SOCKET is set
        use std::process::Command;

        // Find the integration script
        let script_path = find_integration_script("integration.fish");
        let script_path = match script_path {
            Some(p) => p,
            None => {
                eprintln!("Skipping test: can't find integration.fish");
                return;
            }
        };

        // Run fish to check if function is defined
        let output = Command::new("fish")
            .args(["-c", &format!(
                "source {}; type termstack_exec 2>/dev/null && echo 'FUNCTION_DEFINED'",
                script_path
            )])
            .env("TERMSTACK_SOCKET", "/tmp/fake")
            .output();

        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                assert!(
                    stdout.contains("FUNCTION_DEFINED"),
                    "termstack_exec should be defined when TERMSTACK_SOCKET is set. stdout: {}, stderr: {}",
                    stdout,
                    String::from_utf8_lossy(&o.stderr)
                );
            }
            Err(e) => {
                eprintln!("Skipping test: fish not available: {}", e);
            }
        }
    }

    #[test]
    fn fish_integration_binding_set_with_socket() {
        // Verify the fish integration script binds Enter to termstack_exec
        use std::process::Command;

        let script_path = find_integration_script("integration.fish");
        let script_path = match script_path {
            Some(p) => p,
            None => {
                eprintln!("Skipping test: can't find integration.fish");
                return;
            }
        };

        // Run fish to check if binding is set
        let output = Command::new("fish")
            .args(["-c", &format!(
                "source {}; bind | grep -E 'enter|\\\\r' | grep termstack_exec && echo 'BINDING_SET'",
                script_path
            )])
            .env("TERMSTACK_SOCKET", "/tmp/fake")
            .output();

        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                assert!(
                    stdout.contains("BINDING_SET"),
                    "Enter should be bound to termstack_exec when TERMSTACK_SOCKET is set. \
                     stdout: {}, stderr: {}",
                    stdout,
                    String::from_utf8_lossy(&o.stderr)
                );
            }
            Err(e) => {
                eprintln!("Skipping test: fish not available: {}", e);
            }
        }
    }

    #[test]
    fn fish_integration_gui_function_defined() {
        // Verify the fish integration script defines 'gui' function when socket is set
        use std::process::Command;

        let script_path = find_integration_script("integration.fish");
        let script_path = match script_path {
            Some(p) => p,
            None => {
                eprintln!("Skipping test: can't find integration.fish");
                return;
            }
        };

        // Run fish to check if gui function is defined
        let output = Command::new("fish")
            .args(["-c", &format!(
                "source {}; type gui 2>/dev/null && echo 'GUI_FUNCTION_DEFINED'",
                script_path
            )])
            .env("TERMSTACK_SOCKET", "/tmp/fake")
            .output();

        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                assert!(
                    stdout.contains("GUI_FUNCTION_DEFINED"),
                    "gui function should be defined when TERMSTACK_SOCKET is set. stdout: {}, stderr: {}",
                    stdout,
                    String::from_utf8_lossy(&o.stderr)
                );
            }
            Err(e) => {
                eprintln!("Skipping test: fish not available: {}", e);
            }
        }
    }

    #[test]
    fn fish_integration_not_active_without_socket() {
        // Verify the fish integration script does NOT define termstack_exec when socket is unset
        use std::process::Command;

        let script_path = find_integration_script("integration.fish");
        let script_path = match script_path {
            Some(p) => p,
            None => {
                eprintln!("Skipping test: can't find integration.fish");
                return;
            }
        };

        // Run fish without TERMSTACK_SOCKET
        let output = Command::new("fish")
            .args(["-c", &format!(
                "source {}; type termstack_exec 2>/dev/null && echo 'FUNCTION_DEFINED' || echo 'NOT_DEFINED'",
                script_path
            )])
            .env_remove("TERMSTACK_SOCKET")
            .output();

        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                assert!(
                    stdout.contains("NOT_DEFINED"),
                    "termstack_exec should NOT be defined without TERMSTACK_SOCKET. stdout: {}",
                    stdout
                );
            }
            Err(e) => {
                eprintln!("Skipping test: fish not available: {}", e);
            }
        }
    }

    // Helper functions for finding binaries and scripts

    fn find_column_term_binary() -> Option<String> {
        // Try CARGO_BIN_EXE first
        if let Ok(p) = std::env::var("CARGO_BIN_EXE_termstack") {
            if std::path::Path::new(&p).exists() {
                return Some(p);
            }
        }

        // Try relative to current exe
        if let Ok(exe) = std::env::current_exe() {
            if let Some(deps_dir) = exe.parent() {
                if let Some(bin_dir) = deps_dir.parent() {
                    let path = bin_dir.join("termstack");
                    if path.exists() {
                        return Some(path.to_string_lossy().to_string());
                    }
                }
            }
        }

        // Try workspace root
        if let Ok(cwd) = std::env::current_dir() {
            let mut dir = cwd;
            loop {
                let cargo_toml = dir.join("Cargo.toml");
                if cargo_toml.exists() {
                    for subdir in ["target/release", "target/debug"] {
                        let path = dir.join(subdir).join("termstack");
                        if path.exists() {
                            return Some(path.to_string_lossy().to_string());
                        }
                    }
                }
                if let Some(parent) = dir.parent() {
                    dir = parent.to_path_buf();
                } else {
                    break;
                }
            }
        }

        None
    }

    fn find_integration_script(name: &str) -> Option<String> {
        // Try relative to current dir
        if let Ok(cwd) = std::env::current_dir() {
            let mut dir = cwd;
            loop {
                let script = dir.join("scripts").join(name);
                if script.exists() {
                    return Some(script.to_string_lossy().to_string());
                }
                // Also check if we're in a crate dir
                let cargo_toml = dir.join("Cargo.toml");
                if cargo_toml.exists() {
                    // Check both current dir and parent (workspace)
                    let script = dir.join("scripts").join(name);
                    if script.exists() {
                        return Some(script.to_string_lossy().to_string());
                    }
                }
                if let Some(parent) = dir.parent() {
                    dir = parent.to_path_buf();
                } else {
                    break;
                }
            }
        }

        None
    }
}
