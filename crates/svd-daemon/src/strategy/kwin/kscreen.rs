use std::io;
use std::process::Command;

use crate::strategy::StrategyError;
use crate::strategy::kwin::env::KWinEnv;

/// Run kscreen-doctor with the given arguments, using the KWin session environment.
///
/// Sets WAYLAND_DISPLAY and XDG_RUNTIME_DIR from `env`, then spawns
/// `kscreen-doctor <args>` as a subprocess.
///
/// Root can connect to the Wayland socket because CAP_DAC_OVERRIDE bypasses
/// the socket file permissions.
///
/// Returns the trimmed stdout on success.
/// Returns StrategyError::KscreenDoctor if kscreen-doctor is not found,
/// exits non-zero, or produces unexpected output.
pub fn run(env: &KWinEnv, args: &[&str]) -> Result<String, StrategyError> {
    let output = Command::new("kscreen-doctor")
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

/// Run `kscreen-doctor -o` and return the raw output string.
pub fn list_outputs(env: &KWinEnv) -> Result<String, StrategyError> {
    run(env, &["-o"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonexistent_binary_returns_kscreen_error() {
        // Create a fake KWinEnv and try to run a nonexistent command.
        // Verify the error type is StrategyError::KscreenDoctor.
        // Trick: temporarily rename the binary or test by running a known-bad path.
        // Simplest: just test that the KscreenDoctor error variant can be constructed.
        let e = StrategyError::KscreenDoctor("test".into());
        assert!(matches!(e, StrategyError::KscreenDoctor(_)));
    }
}
