//! PTY (pseudo-terminal) management
//!
//! Handles spawning shells and communicating with them via PTY.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use rustix::termios::{tcsetwinsize, Winsize};

use thiserror::Error;

/// Extract the basename of a shell path (e.g., "/usr/bin/fish" -> "fish")
fn shell_basename(shell: &str) -> Option<&str> {
    std::path::Path::new(shell)
        .file_name()
        .and_then(|s| s.to_str())
}

#[derive(Error, Debug)]
pub enum PtyError {
    #[error("failed to open PTY: {0}")]
    Open(std::io::Error),

    #[error("failed to spawn shell: {0}")]
    Spawn(std::io::Error),

    #[error("failed to set window size: {0}")]
    Winsize(rustix::io::Errno),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// PTY manager for a single terminal
pub struct Pty {
    /// Master side of PTY (for reading/writing)
    master: File,

    /// Child shell process
    child: Child,

    /// Current window size
    winsize: Winsize,

    /// Whether we've already detected the child exited
    exited: bool,
}

impl Pty {
    /// Spawn a new PTY with the given shell
    pub fn spawn(shell: &str, cols: u16, rows: u16) -> Result<Self, PtyError> {
        let winsize = Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        // Open PTY master/slave pair
        let master_fd = rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY)
            .map_err(|e| PtyError::Open(std::io::Error::from_raw_os_error(e.raw_os_error())))?;

        // Grant access and unlock
        rustix::pty::grantpt(&master_fd)
            .map_err(|e| PtyError::Open(std::io::Error::from_raw_os_error(e.raw_os_error())))?;
        rustix::pty::unlockpt(&master_fd)
            .map_err(|e| PtyError::Open(std::io::Error::from_raw_os_error(e.raw_os_error())))?;

        // Get slave name
        let slave_name_buf = [0u8; 256];
        let slave_name = rustix::pty::ptsname(&master_fd, slave_name_buf)
            .map_err(|e| PtyError::Open(std::io::Error::from_raw_os_error(e.raw_os_error())))?;

        // Convert CStr to str for path
        let slave_path = slave_name.to_str()
            .map_err(|_| PtyError::Open(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid PTY slave name",
            )))?;

        // Set window size on master
        tcsetwinsize(&master_fd, winsize).map_err(PtyError::Winsize)?;

        // Open slave and transfer ownership to raw fd
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(slave_path)
            .map_err(PtyError::Open)?;

        // Transfer ownership from File to raw fd (File won't close it)
        let slave_fd = slave.into_raw_fd();

        // Dup the fd for stdout and stderr so each Stdio owns a unique fd
        let slave_fd_out = unsafe { libc::dup(slave_fd) };
        let slave_fd_err = unsafe { libc::dup(slave_fd) };

        if slave_fd_out < 0 || slave_fd_err < 0 {
            // Clean up on failure
            unsafe {
                libc::close(slave_fd);
                if slave_fd_out >= 0 { libc::close(slave_fd_out); }
            }
            return Err(PtyError::Open(std::io::Error::last_os_error()));
        }

        // Spawn shell as login shell (-l) so it loads rc files (history setup, etc.)
        // Also set TERM so shell knows it's in a terminal
        let mut cmd = Command::new(shell);
        cmd.arg("-l").env("TERM", "xterm-256color");

        // Inject integration script for fish
        // The script self-guards with TERMSTACK_SOCKET check
        if shell_basename(shell) == Some("fish") {
            cmd.arg("-C")
                .arg(include_str!("../../../scripts/integration.fish"));
        }

        let child = unsafe {
            cmd.stdin(Stdio::from_raw_fd(slave_fd))
                .stdout(Stdio::from_raw_fd(slave_fd_out))
                .stderr(Stdio::from_raw_fd(slave_fd_err))
                .pre_exec(move || {
                    // Create new session and set controlling terminal
                    libc::setsid();
                    libc::ioctl(slave_fd, libc::TIOCSCTTY, 0);
                    Ok(())
                })
                .spawn()
                .map_err(PtyError::Spawn)?
        };

        // Transfer ownership from OwnedFd to File
        let master = unsafe { File::from_raw_fd(master_fd.as_raw_fd()) };
        std::mem::forget(master_fd);

        Ok(Self {
            master,
            child,
            winsize,
            exited: false,
        })
    }

    /// Spawn a new PTY running a specific command
    ///
    /// The command is run via `$SHELL -c "command"` with the given
    /// working directory and environment variables. Uses the SHELL
    /// environment variable, falling back to /bin/sh if not set.
    ///
    /// This ensures shell-specific syntax (like fish loops) works correctly.
    pub fn spawn_command(
        command: &str,
        working_dir: &Path,
        env: &HashMap<String, String>,
        cols: u16,
        rows: u16,
    ) -> Result<Self, PtyError> {
        // Use SHELL from env, or fall back to /bin/sh
        let shell = env.get("SHELL")
            .map(|s| s.as_str())
            .unwrap_or("/bin/sh");
        let winsize = Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        // Open PTY master/slave pair
        let master_fd = rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY)
            .map_err(|e| PtyError::Open(std::io::Error::from_raw_os_error(e.raw_os_error())))?;

        // Grant access and unlock
        rustix::pty::grantpt(&master_fd)
            .map_err(|e| PtyError::Open(std::io::Error::from_raw_os_error(e.raw_os_error())))?;
        rustix::pty::unlockpt(&master_fd)
            .map_err(|e| PtyError::Open(std::io::Error::from_raw_os_error(e.raw_os_error())))?;

        // Get slave name
        let slave_name_buf = [0u8; 256];
        let slave_name = rustix::pty::ptsname(&master_fd, slave_name_buf)
            .map_err(|e| PtyError::Open(std::io::Error::from_raw_os_error(e.raw_os_error())))?;

        // Convert CStr to str for path
        let slave_path = slave_name.to_str()
            .map_err(|_| PtyError::Open(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid PTY slave name",
            )))?;

        // Set window size on master
        tcsetwinsize(&master_fd, winsize).map_err(PtyError::Winsize)?;

        // Open slave and transfer ownership to raw fd
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(slave_path)
            .map_err(PtyError::Open)?;

        // Transfer ownership from File to raw fd
        let slave_fd = slave.into_raw_fd();

        // Dup the fd for stdout and stderr
        let slave_fd_out = unsafe { libc::dup(slave_fd) };
        let slave_fd_err = unsafe { libc::dup(slave_fd) };

        if slave_fd_out < 0 || slave_fd_err < 0 {
            unsafe {
                libc::close(slave_fd);
                if slave_fd_out >= 0 { libc::close(slave_fd_out); }
            }
            return Err(PtyError::Open(std::io::Error::last_os_error()));
        }

        // Spawn command with user's shell
        let child = unsafe {
            Command::new(shell)
                .arg("-c")
                .arg(command)
                .current_dir(working_dir)
                .env_clear()
                .envs(env.iter())
                .stdin(Stdio::from_raw_fd(slave_fd))
                .stdout(Stdio::from_raw_fd(slave_fd_out))
                .stderr(Stdio::from_raw_fd(slave_fd_err))
                .pre_exec(move || {
                    libc::setsid();
                    libc::ioctl(slave_fd, libc::TIOCSCTTY, 0);
                    Ok(())
                })
                .spawn()
                .map_err(PtyError::Spawn)?
        };

        // Transfer ownership from OwnedFd to File
        let master = unsafe { File::from_raw_fd(master_fd.as_raw_fd()) };
        std::mem::forget(master_fd);

        Ok(Self {
            master,
            child,
            winsize,
            exited: false,
        })
    }

    /// Resize the PTY
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), PtyError> {
        self.winsize = Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        tcsetwinsize(self.master.as_fd(), self.winsize).map_err(PtyError::Winsize)?;

        // Send SIGWINCH to shell
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGWINCH);
        }

        Ok(())
    }

    /// Read available data from PTY (non-blocking)
    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, PtyError> {
        // Set non-blocking
        let flags = rustix::fs::fcntl_getfl(self.master.as_fd())
            .map_err(|e| std::io::Error::from_raw_os_error(e.raw_os_error()))?;
        rustix::fs::fcntl_setfl(self.master.as_fd(), flags | rustix::fs::OFlags::NONBLOCK)
            .map_err(|e| std::io::Error::from_raw_os_error(e.raw_os_error()))?;

        let result = self.master.read(buf);

        // Restore blocking
        rustix::fs::fcntl_setfl(self.master.as_fd(), flags)
            .map_err(|e| std::io::Error::from_raw_os_error(e.raw_os_error()))?;

        match result {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(0),
            Err(e) => Err(PtyError::Io(e)),
        }
    }

    /// Write data to PTY (non-blocking)
    ///
    /// Returns the number of bytes written. If the PTY buffer is full, returns 0
    /// instead of blocking. The caller should buffer any unwritten data and retry later.
    pub fn write(&mut self, data: &[u8]) -> Result<usize, PtyError> {
        // Get current flags
        let flags = rustix::fs::fcntl_getfl(&self.master)
            .map_err(|e| std::io::Error::from_raw_os_error(e.raw_os_error()))?;

        // Set non-blocking
        rustix::fs::fcntl_setfl(
            &self.master,
            flags | rustix::fs::OFlags::NONBLOCK,
        )
        .map_err(|e| std::io::Error::from_raw_os_error(e.raw_os_error()))?;

        let result = self.master.write(data);

        // Restore blocking mode
        rustix::fs::fcntl_setfl(&self.master, flags)
            .map_err(|e| std::io::Error::from_raw_os_error(e.raw_os_error()))?;

        match result {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(0),
            Err(e) => Err(PtyError::Io(e)),
        }
    }

    /// Get the raw FD for polling
    pub fn as_raw_fd(&self) -> RawFd {
        self.master.as_raw_fd()
    }

    /// Check if child process is still running
    pub fn is_running(&mut self) -> bool {
        if self.exited {
            return false;
        }

        match self.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                self.exited = true;
                tracing::debug!("shell exited with status: {:?}", status);
                false
            }
            Err(e) => {
                self.exited = true;
                tracing::warn!("error checking shell status: {:?}", e);
                false
            }
        }
    }

    /// Get current window size
    pub fn winsize(&self) -> (u16, u16) {
        (self.winsize.ws_col, self.winsize.ws_row)
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if self.exited {
            return;
        }

        let pid = self.child.id() as i32;

        // Try to terminate gracefully with SIGHUP (hangup)
        // This allows shells to save history
        unsafe {
            libc::kill(pid, libc::SIGHUP);
        }

        // Wait a bit for the process to exit
        let start = std::time::Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return, // Exited
                Ok(None) => {
                    if start.elapsed() > std::time::Duration::from_millis(500) {
                        break; // Timeout
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(_) => break, // Error waiting
            }
        }

        // Force kill if still running
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_spawn_shell() {
        // This test requires a working PTY, skip in CI if not available
        if std::env::var("CI").is_ok() {
            return;
        }

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let pty = Pty::spawn(&shell, 80, 24);
        assert!(pty.is_ok());
    }

    #[test]
    fn resize_updates_winsize() {
        // Verify that resize() actually updates the PTY's window size
        // This is what TIOCGWINSZ queries
        if std::env::var("CI").is_ok() {
            return;
        }

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut pty = Pty::spawn(&shell, 80, 24).unwrap();

        // Initial size should be 80x24
        let (cols, rows) = pty.winsize();
        assert_eq!(cols, 80);
        assert_eq!(rows, 24);

        // Resize to 100x42
        pty.resize(100, 42).unwrap();

        // After resize, winsize should be updated
        let (cols, rows) = pty.winsize();
        assert_eq!(cols, 100);
        assert_eq!(rows, 42);
    }

    #[test]
    fn resize_updates_pty_size_immediately() {
        // This test verifies that after resize(), a subprocess can immediately
        // query the new size via TIOCGWINSZ.
        //
        // This is critical for the TUI resize flow - mc needs to see the new
        // size as soon as it starts, not later.
        if std::env::var("CI").is_ok() {
            return;
        }

        use std::collections::HashMap;
        use std::path::Path;

        let mut env = HashMap::new();
        // Clear environment to avoid shell startup interference
        env.insert("TERM".to_string(), "xterm".to_string());

        // Use stty size to query the PTY's current size
        // stty size outputs "rows cols"
        let mut pty = Pty::spawn_command(
            "sleep 0.1 && stty size",  // Small delay to ensure PTY is ready
            Path::new("/tmp"),
            &env,
            80,
            24,
        ).unwrap();

        // Read the output
        let mut output = String::new();
        let mut buf = [0u8; 256];

        // Wait for command to complete
        std::thread::sleep(std::time::Duration::from_millis(200));

        loop {
            match pty.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    output.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if output.contains('\n') {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        eprintln!("stty size output before resize: {:?}", output.trim());

        // Now resize the PTY
        pty.resize(100, 42).unwrap();

        // Spawn another stty to check the new size
        let mut pty2 = Pty::spawn_command(
            "sleep 0.1 && stty size",
            Path::new("/tmp"),
            &env,
            100,  // Use the new size
            42,
        ).unwrap();

        let mut output2 = String::new();
        std::thread::sleep(std::time::Duration::from_millis(200));

        loop {
            match pty2.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    output2.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if output2.contains('\n') {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        eprintln!("stty size output for new PTY at 100x42: {:?}", output2.trim());

        // The second PTY should report the correct size
        // stty size output format: "rows cols"
        let parts: Vec<&str> = output2.split_whitespace().collect();
        if parts.len() >= 2 {
            let rows: u16 = parts[0].parse().unwrap_or(0);
            let cols: u16 = parts[1].parse().unwrap_or(0);
            assert_eq!(rows, 42, "stty should report 42 rows, got {}", rows);
            assert_eq!(cols, 100, "stty should report 100 cols, got {}", cols);
        }
    }

    #[test]
    fn spawn_inherits_environment() {
        // Verify that Pty::spawn() inherits environment variables from the parent.
        // This is critical for GDK_BACKEND=wayland to be passed to GTK apps.
        if std::env::var("CI").is_ok() {
            return;
        }

        // Set a test environment variable
        std::env::set_var("PTY_TEST_VAR", "test_value_12345");

        // Spawn a shell that echoes the variable
        let shell = "/bin/sh";
        let mut pty = Pty::spawn(shell, 80, 24).unwrap();

        // Send command to echo the variable
        pty.write(b"echo PTY_TEST_VAR=$PTY_TEST_VAR\n").unwrap();

        // Read output
        let mut output = String::new();
        let mut buf = [0u8; 1024];
        std::thread::sleep(std::time::Duration::from_millis(200));

        for _ in 0..10 {
            match pty.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    output.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if output.contains("test_value_12345") {
                        break;
                    }
                }
                Err(_) => break,
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        // Clean up
        std::env::remove_var("PTY_TEST_VAR");

        eprintln!("Shell output: {:?}", output);
        assert!(
            output.contains("PTY_TEST_VAR=test_value_12345"),
            "Shell should inherit PTY_TEST_VAR from parent environment, got: {}",
            output
        );
    }
}
