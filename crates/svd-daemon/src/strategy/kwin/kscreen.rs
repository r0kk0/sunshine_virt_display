use std::io;
use std::process::Command;

use crate::strategy::StrategyError;
use crate::strategy::kwin::env::KWinEnv;

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
}
