//! Tests for config loading and command detection

#[cfg(test)]
mod tests {
    use crate::Config;

    #[test]
    fn shell_commands_from_config() {
        let toml = r#"
            shell_commands = ["cd", "export", "source"]
        "#;

        let config: Config = toml::from_str(toml).unwrap();

        assert!(config.is_shell_command("cd"), "cd should be detected as shell command");
        assert!(config.is_shell_command("export"), "export should be detected as shell command");
        assert!(config.is_shell_command("source"), "source should be detected as shell command");
        assert!(!config.is_shell_command("ls"), "ls should NOT be detected as shell command");
    }

    #[test]
    fn shell_command_with_args() {
        let toml = r#"
            shell_commands = ["cd", "export"]
        "#;

        let config: Config = toml::from_str(toml).unwrap();

        assert!(config.is_shell_command("cd /tmp"), "cd with args should be detected");
        assert!(config.is_shell_command("export FOO=bar"), "export with args should be detected");
    }

    #[test]
    fn default_shell_commands() {
        let config = Config::default();

        // Default shell commands should be populated
        assert!(config.is_shell_command("cd"), "cd should be in default shell commands");
        assert!(config.is_shell_command("export"), "export should be in default shell commands");
        assert!(config.is_shell_command("source"), "source should be in default shell commands");
        assert!(config.is_shell_command("alias"), "alias should be in default shell commands");
    }

    #[test]
    fn column_compositor_tui_env_causes_exit_code_2() {
        // When COLUMN_COMPOSITOR_TUI is set, column-term should exit with code 2
        // (EXIT_SHELL_COMMAND) to tell the shell integration to run the command
        // via eval. This prevents mc's internal subshell commands from being
        // intercepted and spawned as separate terminals.
        //
        // This test runs column-term as a subprocess with the env var set.

        use std::process::Command;

        // Get the column-term binary path
        // Try multiple locations since tests run from different directories
        let cargo_bin = std::env::var("CARGO_BIN_EXE_column-term")
            .or_else(|_| {
                // Look relative to current exe
                std::env::current_exe()
                    .and_then(|p| {
                        let deps_dir = p.parent().unwrap();
                        // Could be target/debug/deps or target/release/deps
                        let bin_dir = deps_dir.parent().unwrap();
                        let path = bin_dir.join("column-term");
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
                                let release = dir.join("target/release/column-term");
                                if release.exists() {
                                    return Ok(release.to_string_lossy().to_string());
                                }
                                let debug = dir.join("target/debug/column-term");
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
                eprintln!("Skipping test: can't find column-term binary");
                return;
            }
        };

        // Check if binary exists
        if !std::path::Path::new(&bin_path).exists() {
            eprintln!("Skipping test: column-term binary not found at {}", bin_path);
            return;
        }

        // Run column-term with COLUMN_COMPOSITOR_TUI set
        let output = Command::new(&bin_path)
            .arg("-c")
            .arg("echo test")
            .env("COLUMN_COMPOSITOR_TUI", "1")
            .env("COLUMN_COMPOSITOR_SOCKET", "/dev/null")  // Fake socket
            .output();

        match output {
            Ok(o) => {
                let exit_code = o.status.code().unwrap_or(-1);
                assert_eq!(
                    exit_code, 2,
                    "column-term should exit with code 2 when COLUMN_COMPOSITOR_TUI is set, \
                     but got {}. stdout: {}, stderr: {}",
                    exit_code,
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                );
            }
            Err(e) => {
                eprintln!("Skipping test: failed to run column-term: {}", e);
            }
        }
    }
}
