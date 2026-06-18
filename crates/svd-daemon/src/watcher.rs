//! Watches the Sunshine process that launched the authenticated `svd` client.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use crate::strategy::DisplayStrategy;

pub fn spawn_watcher(
    strategy: Arc<dyn DisplayStrategy>,
    shutdown: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    token: u64,
    requester_pid: u32,
    requester_uid: u32,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("sunshine-watcher".into())
        .spawn(move || {
            watch_loop(
                strategy,
                shutdown,
                generation,
                token,
                requester_pid,
                requester_uid,
            );
        })?;
    Ok(())
}

fn parse_process_status(status: &str) -> Option<(u32, u32)> {
    let mut parent = None;
    let mut uid = None;
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("PPid:") {
            parent = value.split_whitespace().next()?.parse().ok();
        } else if let Some(value) = line.strip_prefix("Uid:") {
            uid = value.split_whitespace().next()?.parse().ok();
        }
    }
    Some((parent?, uid?))
}

fn find_sunshine_ancestor(mut pid: u32, expected_uid: u32) -> Option<u32> {
    for _ in 0..32 {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        let (parent, uid) = parse_process_status(&status)?;
        if uid != expected_uid {
            return None;
        }
        let command = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
        if command.trim() == "sunshine" {
            return Some(pid);
        }
        if parent == 0 || parent == pid {
            return None;
        }
        pid = parent;
    }
    None
}

fn open_pidfd(pid: u32) -> std::io::Result<OwnedFd> {
    // SAFETY: pidfd_open takes a numeric PID and flags; both arguments are
    // validated values and the return code is checked before ownership transfer.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0u32) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a successful pidfd_open returns a new descriptor owned by caller.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

fn watch_loop(
    strategy: Arc<dyn DisplayStrategy>,
    shutdown: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    token: u64,
    requester_pid: u32,
    requester_uid: u32,
) {
    let Some(pid) = find_sunshine_ancestor(requester_pid, requester_uid) else {
        tracing::info!(
            requester_pid,
            requester_uid,
            "no Sunshine ancestor; watcher disabled"
        );
        return;
    };
    let pidfd = match open_pidfd(pid) {
        Ok(pidfd) => pidfd,
        Err(error) => {
            tracing::warn!(pid, %error, "could not open Sunshine pidfd");
            return;
        }
    };

    loop {
        if shutdown.load(Ordering::Acquire) || generation.load(Ordering::Acquire) != token {
            return;
        }
        let mut pollfd = libc::pollfd {
            fd: pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pollfd points to one initialized entry and pidfd remains owned.
        let result = unsafe { libc::poll(&mut pollfd, 1, 1000) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            tracing::warn!(pid, %error, "Sunshine pidfd poll failed");
            return;
        }
        if result > 0 && pollfd.revents & libc::POLLIN != 0 {
            if generation.load(Ordering::Acquire) == token {
                if let Err(error) = strategy.disconnect() {
                    tracing::warn!(%error, "disconnect after Sunshine exit failed");
                }
            }
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_status_parser_extracts_parent_and_uid() {
        let status = "Name:\tsvd\nUid:\t1000\t1000\t1000\t1000\nPPid:\t42\n";
        assert_eq!(parse_process_status(status), Some((42, 1000)));
    }

    #[test]
    fn process_status_parser_requires_both_fields() {
        assert_eq!(parse_process_status("PPid:\t42\n"), None);
    }

    #[test]
    fn ancestry_search_rejects_wrong_uid() {
        assert_eq!(find_sunshine_ancestor(std::process::id(), u32::MAX), None);
    }
}
