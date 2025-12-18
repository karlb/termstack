//! PTY (pseudo-terminal) management
//!
//! Handles spawning shells and communicating with them via PTY.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use rustix::termios::{tcsetwinsize, Winsize};

use thiserror::Error;

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
        let mut slave_name_buf = [0u8; 256];
        let slave_name = rustix::pty::ptsname(&master_fd, &mut slave_name_buf)
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

        // Spawn shell - each Stdio now owns a unique fd
        let child = unsafe {
            Command::new(shell)
                .stdin(Stdio::from_raw_fd(slave_fd))
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

    /// Write data to PTY
    pub fn write(&mut self, data: &[u8]) -> Result<usize, PtyError> {
        self.master.write(data).map_err(PtyError::Io)
    }

    /// Get the raw FD for polling
    pub fn as_raw_fd(&self) -> RawFd {
        self.master.as_raw_fd()
    }

    /// Check if child process is still running
    pub fn is_running(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                tracing::warn!("shell exited with status: {:?}", status);
                false
            }
            Err(e) => {
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
        // Kill child process if still running
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
}
