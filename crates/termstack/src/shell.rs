//! Shell abstraction for command normalization and syntax checking

use std::env;
use std::process::Command;

use crate::util::debug_enabled;

/// Shell abstraction for handling shell-specific command processing
pub trait Shell {
    /// Normalize a command for the shell (e.g., insert semicolons for Fish)
    fn normalize_command(&self, command: &str) -> String;

    /// Check if the command is syntactically complete
    fn is_syntax_complete(&self, command: &str) -> bool;

    /// Check if a command is a shell builtin
    fn is_builtin(&self, command: &str, builtins: &[String]) -> bool {
        let program = self.program_name(command);
        builtins.iter().any(|cmd| cmd == program)
    }

    /// Extract the program name from a command (first word, without path)
    fn program_name<'a>(&self, command: &'a str) -> &'a str {
        command
            .split_whitespace()
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("")
    }
}

/// Fish shell implementation
pub struct FishShell {
    shell_path: String,
}

impl FishShell {
    pub fn new() -> Self {
        let shell_path = env::var("SHELL").unwrap_or_else(|_| "/usr/bin/fish".to_string());
        Self { shell_path }
    }
}

impl Shell for FishShell {
    fn normalize_command(&self, command: &str) -> String {
        // Keywords that start blocks and need semicolons after them
        let block_keywords = [
            "begin", "if", "while", "for", "function", "switch",
        ];

        // Keywords that end blocks and need semicolons before them
        let end_keywords = [
            "end", "else", "case",
        ];

        let mut result = String::new();
        let mut words = command.split_whitespace().peekable();
        let mut needs_semicolon_before = false;

        while let Some(word) = words.next() {
            // Add semicolon before end keywords (unless at start)
            if !result.is_empty() && end_keywords.contains(&word) && needs_semicolon_before {
                result.push_str("; ");
                needs_semicolon_before = false;
            } else if !result.is_empty() {
                result.push(' ');
            }

            result.push_str(word);

            // Add semicolon after block keywords (unless at end)
            if block_keywords.contains(&word) && words.peek().is_some() {
                result.push(';');
                needs_semicolon_before = true;
            } else if !end_keywords.contains(&word) {
                needs_semicolon_before = true;
            }
        }

        result
    }

    fn is_syntax_complete(&self, command: &str) -> bool {
        let debug = debug_enabled();

        if debug {
            eprintln!("[termstack] fish syntax check: shell={}", self.shell_path);
        }

        let result = Command::new(&self.shell_path)
            .args(["-n", "-c", command])
            .output();

        match result {
            Ok(output) => {
                let complete = output.status.success();
                if debug {
                    eprintln!("[termstack] syntax check: exit={:?}, complete={}", output.status.code(), complete);
                }
                complete
            }
            Err(e) => {
                if debug {
                    eprintln!("[termstack] syntax check: error running shell: {}", e);
                }
                true // If we can't run the shell, assume syntax is complete
            }
        }
    }
}

/// Default shell implementation (assumes sh-compatible)
pub struct DefaultShell;

impl Shell for DefaultShell {
    fn normalize_command(&self, command: &str) -> String {
        // No normalization needed for sh-compatible shells
        command.to_string()
    }

    fn is_syntax_complete(&self, _command: &str) -> bool {
        // For non-fish shells, we don't have a reliable way to check syntax
        // so we assume all commands are complete
        true
    }
}

/// Create a shell instance based on the SHELL environment variable
pub fn detect_shell() -> Box<dyn Shell> {
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let shell_name = std::path::Path::new(&shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("sh");

    if shell_name == "fish" {
        Box::new(FishShell::new())
    } else {
        Box::new(DefaultShell)
    }
}
