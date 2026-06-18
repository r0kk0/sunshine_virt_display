use std::collections::HashMap;
use std::fs;

use crate::strategy::StrategyError;

/// Wayland session environment extracted from a running `kwin_wayland` process.
#[derive(Debug, Clone)]
pub struct KWinEnv {
    pub pid: u32,
    pub wayland_display: String,
    pub xdg_runtime_dir: String,
}

impl KWinEnv {
    /// Scan `/proc` for a process named `kwin_wayland` and extract its
    /// `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` from `/proc/$pid/environ`.
    ///
    /// Returns [`StrategyError::CompositorNotFound`] if no `kwin_wayland`
    /// process exists or if either required variable is absent or empty.
    /// Returns [`StrategyError::Io`] on top-level `/proc` read failure or on
    /// failure to read the matched process's `environ` file.
    pub fn detect() -> Result<Self, StrategyError> {
        // A top-level failure to enumerate /proc is a real I/O error.
        let entries = fs::read_dir("/proc").map_err(StrategyError::Io)?;

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Only consider numeric directory names (PIDs).
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();
            let pid: u32 = match name.parse() {
                Ok(n) => n,
                Err(_) => continue,
            };

            // Read /proc/$pid/comm to get the process name.
            // A read failure here means the process may have vanished — skip it.
            let comm_path = format!("/proc/{}/comm", pid);
            let comm = match fs::read_to_string(&comm_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Match "kwin_wayland" and "kwin_wayland_wrapper" (the latter is
            // truncated to "kwin_wayland_wr" in comm by the 15-char kernel limit).
            // On some distros the wrapper process owns the session environment
            // while the inner kwin_wayland receives the socket via fd, so we
            // try ALL matching processes and pick the first with both vars set.
            if !comm.trim().starts_with("kwin_wayland") {
                continue;
            }

            let environ_path = format!("/proc/{}/environ", pid);
            let raw = match fs::read(&environ_path) {
                Ok(b) => b,
                Err(_) => continue, // process may have vanished
            };

            let vars = parse_environ(&raw);

            let wayland_display = vars
                .get("WAYLAND_DISPLAY")
                .cloned()
                .unwrap_or_default();
            let xdg_runtime_dir = vars
                .get("XDG_RUNTIME_DIR")
                .cloned()
                .unwrap_or_default();

            // This candidate is missing the session vars — try the next one.
            if wayland_display.is_empty() || xdg_runtime_dir.is_empty() {
                continue;
            }

            return Ok(KWinEnv {
                pid,
                wayland_display,
                xdg_runtime_dir,
            });
        }

        Err(StrategyError::CompositorNotFound)
    }
}

/// Parse a NUL-separated `KEY=VALUE` byte blob (as found in `/proc/$pid/environ`)
/// into a `HashMap`.
///
/// - Splits on the **first** `=` so values containing `=` are preserved.
/// - Skips empty segments (the trailing NUL produces one).
/// - Silently skips entries that are not valid UTF-8.
pub(crate) fn parse_environ(bytes: &[u8]) -> HashMap<String, String> {
    bytes
        .split(|&b| b == 0)
        .filter(|seg| !seg.is_empty())
        .filter_map(|seg| {
            let s = std::str::from_utf8(seg).ok()?;
            let eq = s.find('=')?;
            let key = s[..eq].to_owned();
            let value = s[eq + 1..].to_owned();
            Some((key, value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_environ_bytes_extracts_vars() {
        let raw = b"HOME=/root\0WAYLAND_DISPLAY=wayland-1\0XDG_RUNTIME_DIR=/run/user/1000\0";
        let vars = parse_environ(raw);
        assert_eq!(vars.get("WAYLAND_DISPLAY").map(String::as_str), Some("wayland-1"));
        assert_eq!(vars.get("XDG_RUNTIME_DIR").map(String::as_str), Some("/run/user/1000"));
        assert_eq!(vars.get("HOME").map(String::as_str), Some("/root"));
    }

    #[test]
    fn parse_environ_value_with_equals() {
        // Values that themselves contain '=' must be preserved in full.
        let raw = b"FOO=bar=baz\0";
        let vars = parse_environ(raw);
        assert_eq!(vars.get("FOO").map(String::as_str), Some("bar=baz"));
    }

    #[test]
    fn parse_environ_trailing_nul_skipped() {
        // A trailing NUL must not produce a spurious empty entry.
        let raw = b"A=1\0";
        let vars = parse_environ(raw);
        assert_eq!(vars.len(), 1);
    }

    #[test]
    fn parse_environ_empty_input() {
        let vars = parse_environ(b"");
        assert!(vars.is_empty());
    }
}
