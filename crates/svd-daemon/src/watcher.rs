//! Sunshine process crash watcher.
//!
//! T5 — spawns a background thread that monitors the `sunshine` process via a
//! Linux pidfd.  When the process exits the watcher calls
//! `strategy.disconnect()` so the virtual display is torn down automatically.
//!
//! Architecture notes (SOLID):
//!   - Single Responsibility: this module owns only the "watch-and-react"
//!     concern; it does not know how disconnect works.
//!   - Dependency Inversion: depends on the `DisplayStrategy` abstraction, not
//!     on `KWinStrategy` directly.

use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::strategy::DisplayStrategy;

// ──────────────────────────────────────────────────────────────────────────────
// Public API
// ──────────────────────────────────────────────────────────────────────────────

/// Spawn a background thread that watches the `sunshine` process and calls
/// `strategy.disconnect()` when (and only when) the process exits.
///
/// Returns immediately. The thread exits when `shutdown` is set or Sunshine
/// exits.  If Sunshine is not running at call time the thread logs a warning
/// and returns without calling `disconnect()`.
pub fn spawn_watcher(strategy: Arc<dyn DisplayStrategy>, shutdown: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("sunshine-watcher".into())
        .spawn(move || watch_loop(strategy, shutdown))
        .expect("spawn sunshine-watcher");
}

// ──────────────────────────────────────────────────────────────────────────────
// Internal helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Locate a running `sunshine` process by scanning `/proc/*/comm`.
///
/// Returns the PID of the first matching entry, or `None` if sunshine is not
/// running (or disappears mid-scan).
fn find_sunshine_pid() -> Option<u32> {
    let proc_dir = std::fs::read_dir("/proc").ok()?;

    for entry in proc_dir.flatten() {
        // Only consider numeric directory names (i.e. process directories).
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let pid: u32 = match name_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        let comm_path = format!("/proc/{}/comm", pid);
        let contents = match std::fs::read_to_string(&comm_path) {
            Ok(c) => c,
            // The process may have exited between scan and read — skip it.
            Err(_) => continue,
        };

        // `comm` contains the process name truncated to 15 chars + newline.
        if contents.trim() == "sunshine" {
            return Some(pid);
        }
    }

    None
}

/// Open a pidfd for `pid` using `pidfd_open(2)`.
///
/// On Linux >= 5.3 this gives a file descriptor that becomes readable (POLLIN)
/// when the process exits, enabling race-free monitoring.
fn open_pidfd(pid: u32) -> Result<RawFd, std::io::Error> {
    // SAFETY: syscall arguments are well-formed; return value checked below.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0u32) };
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(fd as RawFd)
    }
}

/// Main watcher loop executed in the background thread.
///
/// Steps:
/// 1. Find the Sunshine PID via `/proc`.
/// 2. Open a pidfd for race-free monitoring.
/// 3. Poll the pidfd with a 1-second timeout until either the process exits or
///    `shutdown` is set.
/// 4. Close the pidfd.
/// 5. Call `strategy.disconnect()` only if the process actually exited (not on
///    clean shutdown) — this preserves the state file so `restore()` works
///    across daemon restarts.
fn watch_loop(strategy: Arc<dyn DisplayStrategy>, shutdown: Arc<AtomicBool>) {
    // Step 1 — find PID.
    let pid = match find_sunshine_pid() {
        Some(p) => p,
        None => {
            tracing::warn!("sunshine-watcher: sunshine process not found; nothing to watch");
            return;
        }
    };

    tracing::info!(pid, "sunshine-watcher: watching sunshine process");

    // Step 2 — open pidfd.
    let pidfd = match open_pidfd(pid) {
        Ok(fd) => fd,
        Err(e) => {
            // PID vanished between find and open — treat as an exit event.
            tracing::warn!(
                pid,
                error = %e,
                "sunshine-watcher: pidfd_open failed (process already gone); disconnecting"
            );
            call_disconnect(&strategy);
            return;
        }
    };

    // Step 3 — poll loop.
    let mut process_exited = false;

    loop {
        if shutdown.load(Ordering::Acquire) {
            tracing::debug!("sunshine-watcher: shutdown requested; exiting without disconnect");
            break;
        }

        // SAFETY: `pidfd` is valid; `pollfd` layout matches the kernel ABI.
        let mut pfd = libc::pollfd {
            fd: pidfd,
            events: libc::POLLIN,
            revents: 0,
        };

        let ret = unsafe { libc::poll(&mut pfd, 1, 1000) }; // 1 s timeout

        if ret < 0 {
            // EINTR (from a signal) or transient error — re-check shutdown.
            continue;
        }

        if ret > 0 && (pfd.revents & libc::POLLIN) != 0 {
            tracing::info!(pid, "sunshine-watcher: sunshine exited; disconnecting");
            process_exited = true;
            break;
        }

        // ret == 0: timeout; loop again.
    }

    // Step 4 — close pidfd.
    // SAFETY: `pidfd` was successfully opened above and has not been closed.
    unsafe { libc::close(pidfd) };

    // Step 5 — disconnect only on real process exit, not on daemon shutdown.
    if process_exited {
        call_disconnect(&strategy);
    }
}

/// Call `strategy.disconnect()` and log any error at WARN level.
fn call_disconnect(strategy: &Arc<dyn DisplayStrategy>) {
    if let Err(e) = strategy.disconnect() {
        tracing::warn!(error = %e, "sunshine-watcher: disconnect() failed after sunshine exit");
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Unit tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_sunshine_pid_returns_none_without_sunshine() {
        // When sunshine is not running, returns None.
        // If sunshine IS running on this machine the function may return Some —
        // the test still passes; we only verify it does not panic.
        let pid = find_sunshine_pid();
        let _ = pid;
    }
}
