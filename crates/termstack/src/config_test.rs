//! Tests for config loading and command detection

#[cfg(test)]
mod tests {
    use crate::{shell::{Shell, FishShell, detect_shell}, Config};

    #[test]
    fn shell_commands_from_config() {
        let toml = r#"
            shell_commands = ["cd", "export", "source"]
        "#;

        let config: Config = toml::from_str(toml).unwrap();
        let shell = detect_shell();

        assert!(config.is_shell_command("cd", shell.as_ref()), "cd should be detected as shell command");
        assert!(config.is_shell_command("export", shell.as_ref()), "export should be detected as shell command");
        assert!(config.is_shell_command("source", shell.as_ref()), "source should be detected as shell command");
        assert!(!config.is_shell_command("ls", shell.as_ref()), "ls should NOT be detected as shell command");
    }

    #[test]
    fn shell_command_with_args() {
        let toml = r#"
            shell_commands = ["cd", "export"]
        "#;

        let config: Config = toml::from_str(toml).unwrap();
        let shell = detect_shell();

        assert!(config.is_shell_command("cd /tmp", shell.as_ref()), "cd with args should be detected");
        assert!(config.is_shell_command("export FOO=bar", shell.as_ref()), "export with args should be detected");
    }

    #[test]
    fn default_shell_commands() {
        let config = Config::default();
        let shell = detect_shell();

        // Default shell commands should be populated
        assert!(config.is_shell_command("cd", shell.as_ref()), "cd should be in default shell commands");
        assert!(config.is_shell_command("export", shell.as_ref()), "export should be in default shell commands");
        assert!(config.is_shell_command("source", shell.as_ref()), "source should be in default shell commands");
        assert!(config.is_shell_command("alias", shell.as_ref()), "alias should be in default shell commands");
    }

    #[test]
    fn column_compositor_tui_env_causes_exit_code_2() {
        // When TERMSTACK_TUI is set, termstack should exit with code 2
        // (EXIT_SHELL_COMMAND) to tell the shell integration to run the command
        // via eval. This prevents mc's internal subshell commands from being
        // intercepted and spawned as separate terminals.
        //
        // This test runs termstack as a subprocess with the env var set.

        use std::process::Command;

        // Get the termstack binary path
        // Try multiple locations since tests run from different directories
        let cargo_bin = std::env::var("CARGO_BIN_EXE_termstack")
            .or_else(|_| {
                // Look relative to current exe
                std::env::current_exe()
                    .and_then(|p| {
                        let deps_dir = p.parent().unwrap();
                        // Could be target/debug/deps or target/release/deps
                        let bin_dir = deps_dir.parent().unwrap();
                        let path = bin_dir.join("termstack");
                        if path.exists() {
                            Ok(path.to_string_lossy().to_string())
                        } else {
                            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found"))
                        }
                    })
            })
            .or_else(|_| {
                // Try workspace root/target/release
                std::env::current_dir()
                    .and_then(|p| {
                        // Walk up to find Cargo.toml
                        let mut dir = p;
                        loop {
                            let cargo_toml = dir.join("Cargo.toml");
                            if cargo_toml.exists() {
                                let release = dir.join("target/release/termstack");
                                if release.exists() {
                                    return Ok(release.to_string_lossy().to_string());
                                }
                                let debug = dir.join("target/debug/termstack");
                                if debug.exists() {
                                    return Ok(debug.to_string_lossy().to_string());
                                }
                            }
                            if let Some(parent) = dir.parent() {
                                dir = parent.to_path_buf();
                            } else {
                                break;
                            }
                        }
                        Err(std::io::Error::new(std::io::ErrorKind::NotFound, "not found"))
                    })
            });

        // Skip test if we can't find the binary
        let bin_path = match cargo_bin {
            Ok(p) => p,
            Err(_) => {
                eprintln!("Skipping test: can't find termstack binary");
                return;
            }
        };

        // Check if binary exists
        if !std::path::Path::new(&bin_path).exists() {
            eprintln!("Skipping test: termstack binary not found at {}", bin_path);
            return;
        }

        // Run termstack with TERMSTACK_TUI set
        let output = Command::new(&bin_path)
            .arg("-c")
            .arg("echo test")
            .env("TERMSTACK_TUI", "1")
            .env("TERMSTACK_SOCKET", "/dev/null")  // Fake socket
            .output();

        match output {
            Ok(o) => {
                let exit_code = o.status.code().unwrap_or(-1);
                assert_eq!(
                    exit_code, 2,
                    "termstack should exit with code 2 when TERMSTACK_TUI is set, \
                     but got {}. stdout: {}, stderr: {}",
                    exit_code,
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                );
            }
            Err(e) => {
                eprintln!("Skipping test: failed to run termstack: {}", e);
            }
        }
    }

    #[test]
    fn syntax_check_complete_commands() {
        // Simple complete commands should be valid in any shell
        let shell = detect_shell();
        assert!(shell.is_syntax_complete("echo hello"), "simple echo should be complete");
        assert!(shell.is_syntax_complete("ls -la"), "ls with args should be complete");
        assert!(shell.is_syntax_complete("cat /etc/passwd"), "cat should be complete");
    }

    #[test]
    fn syntax_check_incomplete_commands() {
        // These are incomplete in fish
        // Skip test if not using fish
        let shell_name = std::env::var("SHELL").unwrap_or_default();
        if !shell_name.ends_with("fish") {
            eprintln!("Skipping test: not using fish shell");
            return;
        }

        let shell = detect_shell();
        // (unterminated string)
        assert!(!shell.is_syntax_complete("echo 'hello"), "unterminated single quote");
        assert!(!shell.is_syntax_complete("echo \"hello"), "unterminated double quote");
    }

    #[test]
    fn syntax_check_shell_specific_incomplete() {
        // Test fish-specific incomplete syntax
        let shell_name = std::env::var("SHELL").unwrap_or_default();

        if shell_name.ends_with("fish") {
            let shell = detect_shell();
            assert!(!shell.is_syntax_complete("begin"), "fish begin without end");
            assert!(shell.is_syntax_complete("begin; echo hi; end"), "fish complete begin/end");
        }
    }

    #[test]
    fn syntax_check_multiline_equivalent_to_singleline() {
        // Bug report: Multi-line and single-line versions of the same command
        // should be treated identically. Syntactic differences (newlines vs semicolons)
        // should not cause behavioral differences.

        // Check if fish is available
        use std::process::Command;
        let fish_check = Command::new("fish").arg("--version").output();
        if fish_check.is_err() {
            eprintln!("Skipping test: fish not found in PATH");
            return;
        }

        std::env::set_var("SHELL", "fish");
        let shell = FishShell::new();

        // Single-line version (with semicolons)
        let singleline = "begin; echo a; echo b; end";

        // Multi-line version (with newlines) - normalize first
        let multiline = shell.normalize_command("begin\n  echo a\n  echo b\nend");

        // Space-separated version (what Fish's commandline returns) - normalize first
        let space_separated = shell.normalize_command("begin echo a echo b end");

        // All three should be syntactically complete
        let singleline_complete = shell.is_syntax_complete(singleline);
        let multiline_complete = shell.is_syntax_complete(&multiline);
        let space_separated_complete = shell.is_syntax_complete(&space_separated);

        assert_eq!(
            singleline_complete, multiline_complete,
            "Single-line and multi-line versions should have same syntax completeness. \
             Single-line: {}, Multi-line: {}",
            singleline_complete, multiline_complete
        );

        assert_eq!(
            singleline_complete, space_separated_complete,
            "Single-line and space-separated versions should have same syntax completeness. \
             Single-line: {}, Space-separated: {}",
            singleline_complete, space_separated_complete
        );

        // All should be complete (not incomplete)
        assert!(
            singleline_complete,
            "Single-line 'begin; echo a; echo b; end' should be complete"
        );
        assert!(
            multiline_complete,
            "Multi-line 'begin\\n  echo a\\n  echo b\\nend' should be complete (normalized to '{}')",
            multiline
        );
        assert!(
            space_separated_complete,
            "Space-separated 'begin echo a echo b end' should be complete (normalized to '{}')",
            space_separated
        );
    }

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
