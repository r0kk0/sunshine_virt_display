use std::io;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::Duration;

use crate::strategy::kwin::env::KWinEnv;
use crate::strategy::StrategyError;
use svd_proto::ConnectorId;

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
    let mut current_output = None;

    for line in text.lines() {
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix("Output:") {
            // "Output: N NAME uuid" — start a new output block.
            let parts: Vec<&str> = rest.split_whitespace().collect();
            current_output = None;
            if parts.len() >= 2 && ConnectorId::try_from(parts[1]).is_ok() {
                result.push(OutputInfo {
                    name: parts[1].to_string(),
                    enabled: false,
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                });
                current_output = Some(result.len() - 1);
            }
        } else if trimmed == "enabled" {
            if let Some(output) = current_output.and_then(|index| result.get_mut(index)) {
                output.enabled = true;
            }
        } else if trimmed == "disabled" {
            if let Some(output) = current_output.and_then(|index| result.get_mut(index)) {
                output.enabled = false;
            }
        } else if let Some(rest) = trimmed.strip_prefix("Geometry:") {
            if let Some(output) = current_output.and_then(|index| result.get_mut(index)) {
                if let Some((x, y, w, h)) = parse_geometry(rest.trim()) {
                    output.x = x;
                    output.y = y;
                    output.width = w;
                    output.height = h;
                }
            }
        }
    }

    result
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ActiveMode {
    width: u32,
    height: u32,
    refresh: f64,
}

fn parse_active_mode_token(token: &str) -> Option<ActiveMode> {
    let marker_index = token.find(['*', '!'])?;
    if !token[marker_index..].contains('*') {
        return None;
    }
    let (_, mode) = token[..marker_index].split_once(':')?;
    let (size, refresh) = mode.split_once('@')?;
    let (width, height) = size.split_once('x')?;
    Some(ActiveMode {
        width: width.parse().ok()?,
        height: height.parse().ok()?,
        refresh: refresh.parse().ok()?,
    })
}

pub(crate) fn active_mode_matches(
    text: &str,
    connector: &str,
    width: u32,
    height: u32,
    refresh: u32,
) -> bool {
    if ConnectorId::try_from(connector).is_err() {
        return false;
    }

    let mut current_is_target = false;
    let mut enabled = false;
    let mut active_mode = None;

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Output:") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            current_is_target = parts.get(1).is_some_and(|name| *name == connector);
            if current_is_target {
                enabled = false;
                active_mode = None;
            }
        } else if current_is_target && trimmed == "enabled" {
            enabled = true;
        } else if current_is_target && trimmed == "disabled" {
            enabled = false;
        } else if current_is_target {
            if let Some(modes) = trimmed.strip_prefix("Modes:") {
                active_mode = modes.split_whitespace().find_map(parse_active_mode_token);
            }
        }
    }

    enabled
        && active_mode.is_some_and(|mode| {
            mode.width == width
                && mode.height == height
                && (mode.refresh - f64::from(refresh)).abs() <= 0.5
        })
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
// ANSI sanitizer
// ──────────────────────────────────────────────────────────────────────────────

fn strip_ansi_csi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for inner in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&inner) {
                        break;
                    }
                }
            }
            Some(other) => {
                out.push(other);
            }
            None => {}
        }
    }
    out
}

// ──────────────────────────────────────────────────────────────────────────────
// kscreen-doctor subprocess
// ──────────────────────────────────────────────────────────────────────────────

/// Core implementation: spawn `binary` with `args` under the KWin session
/// environment. Extracted so tests can inject an arbitrary binary name
/// without requiring `kscreen-doctor` to be installed.
///
/// `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` are set from `env`; all other daemon
/// environment variables are cleared before credentials are dropped.
fn run_with_binary(binary: &str, env: &KWinEnv, args: &[&str]) -> Result<String, StrategyError> {
    let output = Command::new(binary)
        .uid(env.uid)
        .gid(env.gid)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
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
    Ok(strip_ansi_csi(stdout.as_ref()).trim().to_owned())
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
    run_with_binary("/usr/bin/kscreen-doctor", env, args)
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
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata("/proc/self").expect("process metadata");
        KWinEnv {
            uid: metadata.uid(),
            gid: metadata.gid(),
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

    #[test]
    fn parse_outputs_discards_unsafe_connector_names() {
        let text = "Output: 1 DP-1.disable uuid1\n    enabled\n    Geometry: 0,0 1920x1080\nOutput: 2 HDMI-A-1 uuid2\n    enabled\n    Geometry: 1920,0 1920x1080\n";

        let outputs = parse_outputs(text);

        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].name, "HDMI-A-1");
    }

    #[test]
    fn active_mode_matches_physical_pixels_instead_of_scaled_geometry() {
        let text = "Output: 1 DP-3 uuid\n    enabled\n    Modes:  1:3840x2160@60.00!  2:3840x2160@144.00*\n    Geometry: 2560,0 2560x1440\n    Scale: 1.5\n";

        assert!(active_mode_matches(text, "DP-3", 3840, 2160, 144));
        assert!(!active_mode_matches(text, "DP-3", 2560, 1440, 144));
    }

    #[test]
    fn active_mode_accepts_fractional_refresh_close_to_requested_integer() {
        let text = "Output: 1 DP-1 uuid\n    enabled\n    Modes:  1:1920x1080@59.94*\n    Geometry: 0,0 1920x1080\n";

        assert!(active_mode_matches(text, "DP-1", 1920, 1080, 60));
    }

    #[test]
    fn active_mode_rejects_wrong_refresh() {
        let text = "Output: 1 DP-1 uuid\n    enabled\n    Modes:  1:1920x1080@120.00*\n";

        assert!(!active_mode_matches(text, "DP-1", 1920, 1080, 60));
    }

    #[test]
    fn active_mode_does_not_treat_preferred_mode_as_current() {
        let text =
            "Output: 1 DP-1 uuid\n    enabled\n    Modes:  1:1920x1080@60.00!  2:1280x720@60.00*\n";

        assert!(!active_mode_matches(text, "DP-1", 1920, 1080, 60));
        assert!(active_mode_matches(text, "DP-1", 1280, 720, 60));
    }

    #[test]
    fn active_mode_rejects_disabled_or_different_connector() {
        let text = "Output: 1 DP-1 uuid\n    disabled\n    Modes:  1:1920x1080@60.00*\nOutput: 2 DP-2 uuid\n    enabled\n    Modes:  2:1920x1080@60.00*\n";

        assert!(!active_mode_matches(text, "DP-1", 1920, 1080, 60));
        assert!(!active_mode_matches(text, "DP-3", 1920, 1080, 60));
    }

    #[test]
    fn active_mode_rejects_malformed_mode_token() {
        let text = "Output: 1 DP-1 uuid\n    enabled\n    Modes:  malformed*\n";

        assert!(!active_mode_matches(text, "DP-1", 1920, 1080, 60));
    }

    // ── ANSI sanitizer tests ───────────────────────────────────────────────────

    #[test]
    fn strip_ansi_removes_sgr_sequences() {
        let input = "\x1b[01;32mOutput: \x1b[0;0m1 DP-1 uuid";
        assert_eq!(strip_ansi_csi(input), "Output: 1 DP-1 uuid");
    }

    #[test]
    fn strip_ansi_preserves_plain_text() {
        let input = "Output: 1 DP-1 uuid\n    enabled\n    Geometry: 0,0 1920x1080";
        assert_eq!(strip_ansi_csi(input), input);
    }

    #[test]
    fn strip_ansi_no_panic_on_truncated_escape() {
        let _ = strip_ansi_csi("\x1b");
        let _ = strip_ansi_csi("\x1b[01;32");
    }

    #[test]
    fn parse_outputs_colored_output_block() {
        let colored = "\x1b[01;32mOutput: \x1b[0;0m1 DP-1 233cb8ab-5f87-40fc-9c7f-a17389b58f68\n\
                       \x1b[01;34m    enabled\x1b[0;0m\n\
                           connected\n\
                       \x1b[01;34m    Geometry: \x1b[0;0m0,0 1920x1080\n\
                       \x1b[01;34m    Scale: \x1b[0;0m1\n";
        let clean = strip_ansi_csi(colored);
        let outputs = parse_outputs(&clean);
        assert_eq!(outputs.len(), 1, "expected 1 output, got: {:?}", outputs);
        assert_eq!(outputs[0].name, "DP-1");
        assert!(outputs[0].enabled);
        assert_eq!(outputs[0].width, 1920);
        assert_eq!(outputs[0].height, 1080);
    }

    #[test]
    fn active_mode_matches_colored_fixture() {
        let colored = "\x1b[01;32mOutput: \x1b[0;0m1 DP-1 233cb8ab-5f87-40fc-9c7f-a17389b58f68\n\
                       \x1b[01;34m    enabled\x1b[0;0m\n\
                           connected\n\
                       \x1b[01;34m    Modes: \x1b[0;0m 1:\x1b[01;32m1920x1080@60.00*!\x1b[0;0m\n\
                       \x1b[01;34m    Geometry: \x1b[0;0m0,0 1920x1080\n\
                       \x1b[01;34m    Scale: \x1b[0;0m1\n";
        let clean = strip_ansi_csi(colored);
        assert!(
            active_mode_matches(&clean, "DP-1", 1920, 1080, 60),
            "active_mode_matches should return true for colored DP-1 fixture"
        );
    }
}
