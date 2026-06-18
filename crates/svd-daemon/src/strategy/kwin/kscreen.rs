use std::io;
use std::process::Command;
use std::time::Duration;

use crate::strategy::kwin::env::KWinEnv;
use crate::strategy::StrategyError;

// ──────────────────────────────────────────────────────────────────────────────
// OutputInfo — layout snapshot
// ──────────────────────────────────────────────────────────────────────────────

/// Snapshot of an output's state as reported by `kscreen-doctor -o`.
///
/// Used to save the full display layout before connect so that disconnect can
/// restore exact positions and enabled states atomically.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OutputInfo {
    /// Connector name, e.g. "DP-2".
    pub name: String,
    /// Whether the output is currently enabled.
    pub enabled: bool,
    /// Left edge x coordinate (can be negative for left-of-primary setups).
    pub x: i32,
    /// Top edge y coordinate.
    pub y: i32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

// ──────────────────────────────────────────────────────────────────────────────
// Layout parser
// ──────────────────────────────────────────────────────────────────────────────

/// Parse `kscreen-doctor -o` output into a list of [`OutputInfo`].
///
/// KDE Plasma 6 format:
/// ```text
/// Output: N NAME uuid
///     enabled          ← or "disabled", on its own line
///     connected
///     Geometry: x,y WxH    ← space between pos and size, no '@', x can be negative
/// ```
/// Unknown lines are silently skipped — the parser is lenient so future
/// kscreen-doctor format changes don't break the daemon.
pub fn parse_outputs(text: &str) -> Vec<OutputInfo> {
    let mut result: Vec<OutputInfo> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix("Output:") {
            // "Output: N NAME uuid" — start a new output block.
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 2 {
                result.push(OutputInfo {
                    name: parts[1].to_string(),
                    enabled: false,
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                });
            }
        } else if trimmed == "enabled" {
            if let Some(last) = result.last_mut() {
                last.enabled = true;
            }
        } else if trimmed == "disabled" {
            if let Some(last) = result.last_mut() {
                last.enabled = false;
            }
        } else if let Some(rest) = trimmed.strip_prefix("Geometry:") {
            if let Some(last) = result.last_mut() {
                if let Some((x, y, w, h)) = parse_geometry(rest.trim()) {
                    last.x = x;
                    last.y = y;
                    last.width = w;
                    last.height = h;
                }
            }
        }
    }

    result
}

fn parse_geometry(s: &str) -> Option<(i32, i32, u32, u32)> {
    // Format: "x,y WxH"  e.g. "-2560,0 2560x1440" or "0,0 1920x1080"
    let (pos, size) = s.split_once(' ')?;
    let (x_str, y_str) = pos.split_once(',')?;
    let (w_str, h_str) = size.split_once('x')?;
    Some((
        x_str.parse().ok()?,
        y_str.parse().ok()?,
        w_str.parse().ok()?,
        h_str.parse().ok()?,
    ))
}

// ──────────────────────────────────────────────────────────────────────────────
// kscreen-doctor subprocess
// ──────────────────────────────────────────────────────────────────────────────

/// Core implementation: spawn `binary` with `args` under the KWin session
/// environment. Extracted so tests can inject an arbitrary binary name
/// without requiring `kscreen-doctor` to be installed.
///
/// `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` are set from `env`; the rest of the
/// daemon's environment is inherited (no `.env_clear()`) — no secrets are
/// expected in the daemon's environment at this call site.
fn run_with_binary(binary: &str, env: &KWinEnv, args: &[&str]) -> Result<String, StrategyError> {
    let output = Command::new(binary)
        .env("WAYLAND_DISPLAY", &env.wayland_display)
        .env("XDG_RUNTIME_DIR", &env.xdg_runtime_dir)
        .args(args)
        .output()
        .map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                StrategyError::KscreenDoctor("kscreen-doctor not found in PATH".into())
            } else {
                StrategyError::Io(e)
            }
        })?;

    if !output.status.success() {
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".into());
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(StrategyError::KscreenDoctor(format!(
            "exit {}: {}",
            code,
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.trim().to_owned())
}

/// Run kscreen-doctor with the given arguments, using the KWin session environment.
///
/// Sets `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` from `env`, then spawns
/// `kscreen-doctor <args>` as a subprocess.
///
/// Returns the trimmed stdout on success.
/// Returns `StrategyError::KscreenDoctor` if kscreen-doctor is not found in
/// PATH, exits non-zero, or is killed by a signal.
pub fn run(env: &KWinEnv, args: &[&str]) -> Result<String, StrategyError> {
    run_with_binary("kscreen-doctor", env, args)
}

/// Run `kscreen-doctor -o` and return the trimmed stdout string.
pub fn list_outputs(env: &KWinEnv) -> Result<String, StrategyError> {
    run(env, &["-o"])
}

/// Run kscreen-doctor with `args` twice, sleeping `delay_ms` between passes.
///
/// KWin sometimes needs two kscreen-doctor invocations to fully apply a layout
/// change: the first triggers KWin's internal reflow and the second commits the
/// settled state. Errors on the first pass are logged and swallowed; only the
/// second pass result is returned.
pub fn run_twice(env: &KWinEnv, args: &[&str], delay_ms: u64) -> Result<String, StrategyError> {
    if let Err(e) = run(env, args) {
        tracing::debug!(error = %e, "run_twice: first pass failed");
    }
    std::thread::sleep(Duration::from_millis(delay_ms));
    run(env, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_env() -> KWinEnv {
        KWinEnv {
            pid: 0,
            wayland_display: "wayland-1".into(),
            xdg_runtime_dir: "/run/user/1000".into(),
        }
    }

    #[test]
    fn nonexistent_binary_returns_kscreen_error() {
        // Use a binary name that is guaranteed to be absent on any system.
        // run_with_binary maps io::ErrorKind::NotFound → StrategyError::KscreenDoctor.
        let result = run_with_binary("__svd_nonexistent_binary_xyz__", &fake_env(), &[]);
        assert!(
            matches!(result, Err(StrategyError::KscreenDoctor(_))),
            "expected KscreenDoctor error, got: {:?}",
            result
        );
    }

    // ── parse_outputs tests ────────────────────────────────────────────────────

    #[test]
    fn parse_outputs_extracts_enabled_geometry() {
        let text = "Output: 2 DP-2 some-uuid\n    enabled\n    connected\n    Geometry: 0,0 2560x1440\n    Scale: 1\n";
        let outputs = parse_outputs(text);
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].name, "DP-2");
        assert!(outputs[0].enabled);
        assert_eq!(outputs[0].x, 0);
        assert_eq!(outputs[0].y, 0);
        assert_eq!(outputs[0].width, 2560);
        assert_eq!(outputs[0].height, 1440);
    }

    #[test]
    fn parse_outputs_negative_x_geometry() {
        let text = "Output: 1 HDMI-A-1 uuid\n    disabled\n    Geometry: -2560,0 2560x1440\n";
        let outputs = parse_outputs(text);
        assert_eq!(outputs.len(), 1);
        assert!(!outputs[0].enabled);
        assert_eq!(outputs[0].x, -2560);
        assert_eq!(outputs[0].y, 0);
        assert_eq!(outputs[0].width, 2560);
        assert_eq!(outputs[0].height, 1440);
    }

    #[test]
    fn parse_outputs_multiple_outputs() {
        let text = "Output: 1 DP-2 uuid1\n    enabled\n    Geometry: 0,0 2560x1440\nOutput: 2 DP-3 uuid2\n    enabled\n    Geometry: 2560,0 2560x1440\n";
        let outputs = parse_outputs(text);
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].name, "DP-2");
        assert_eq!(outputs[1].name, "DP-3");
        assert_eq!(outputs[1].x, 2560);
    }

    #[test]
    fn parse_outputs_disabled_output_has_enabled_false() {
        let text =
            "Output: 4 DP-1 uuid\n    disabled\n    connected\n    Geometry: 0,0 1920x1080\n";
        let outputs = parse_outputs(text);
        assert_eq!(outputs.len(), 1);
        assert!(!outputs[0].enabled);
    }

    #[test]
    fn parse_outputs_empty_input() {
        let outputs = parse_outputs("");
        assert!(outputs.is_empty());
    }
}
