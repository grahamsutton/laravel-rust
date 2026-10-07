//! POSIX signal numbers, for [`InvokedProcess::signal`](crate::InvokedProcess::signal).
//!
//! These are the familiar `SIG*` constants PHP exposes through `pcntl`:
//!
//! ```
//! use illuminate_process::signal::{SIGTERM, SIGUSR2};
//!
//! assert_eq!(SIGTERM, 15);
//! # let _ = SIGUSR2;
//! ```

/// Hangup.
pub const SIGHUP: i32 = 1;
/// Interrupt (`Ctrl+C`).
pub const SIGINT: i32 = 2;
/// Quit.
pub const SIGQUIT: i32 = 3;
/// Kill (cannot be caught or ignored).
pub const SIGKILL: i32 = 9;
/// Alarm clock.
pub const SIGALRM: i32 = 14;
/// Termination request.
pub const SIGTERM: i32 = 15;

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)))]
mod platform {
    /// User-defined signal 1.
    pub const SIGUSR1: i32 = 10;
    /// User-defined signal 2.
    pub const SIGUSR2: i32 = 12;
    /// Child stopped or terminated.
    pub const SIGCHLD: i32 = 17;
    /// Continue if stopped.
    pub const SIGCONT: i32 = 18;
    /// Stop (cannot be caught or ignored).
    pub const SIGSTOP: i32 = 19;
    /// Stop typed at the terminal.
    pub const SIGTSTP: i32 = 20;
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
mod platform {
    /// User-defined signal 1.
    pub const SIGUSR1: i32 = 30;
    /// User-defined signal 2.
    pub const SIGUSR2: i32 = 31;
    /// Child stopped or terminated.
    pub const SIGCHLD: i32 = 20;
    /// Continue if stopped.
    pub const SIGCONT: i32 = 19;
    /// Stop (cannot be caught or ignored).
    pub const SIGSTOP: i32 = 17;
    /// Stop typed at the terminal.
    pub const SIGTSTP: i32 = 18;
}

pub use platform::*;

/// Send a signal to a process.
#[cfg(unix)]
pub(crate) fn send(pid: u32, signal: i32) -> std::io::Result<()> {
    unsafe extern "C" {
        // `kill` only takes plain integers and has no memory safety
        // requirements, so it is declared safe to call.
        safe fn kill(pid: i32, signal: i32) -> i32;
    }

    let pid = i32::try_from(pid)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid process id"))?;

    if kill(pid, signal) == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Send a signal to a process.
#[cfg(not(unix))]
pub(crate) fn send(_pid: u32, _signal: i32) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Signals are not supported on this platform.",
    ))
}
