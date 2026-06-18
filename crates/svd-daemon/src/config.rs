//! Configuration loading for svd-daemon.
//!
//! T2.3 — TOML config with safe defaults.
//!
//! Security invariants:
//!   - `hdr` defaults to `false` — HDR mode causes freezes on many setups.
//!   - `allow_master_stealing` defaults to `false` — this is the most
//!     black-screen-prone operation and must be explicitly opted in.
//!   - A missing config file is safe: `load_config` returns the defaults.
//!   - Unknown TOML keys are rejected via `deny_unknown_fields`.

use std::path::Path;

// ──────────────────────────────────────────────────────────────────────────────
// Error type
// ──────────────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },

    #[error("invalid config file {path}: {source}")]
    Parse {
        path: std::path::PathBuf,
        source: toml::de::Error,
    },
}

// ──────────────────────────────────────────────────────────────────────────────
// Config struct
// ──────────────────────────────────────────────────────────────────────────────

/// Daemon configuration loaded from a TOML file.
///
/// All fields have safe defaults; a missing config file produces the same
/// result as an empty one.  Unknown keys are rejected at parse time.
#[allow(dead_code)]
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// HDR mode. Default: false (causes freezes on many setups).
    pub hdr: bool,

    /// Allow DRM-master stealing via pidfd_getfd. Default: false.
    /// Must be explicitly opted in — this is the most black-screen-prone
    /// operation.
    pub allow_master_stealing: bool,

    /// Override card auto-detection. Example: "card1".
    pub device: Option<String>,

    /// Additional modes beyond the built-in VIC table.
    pub extra_allowed_modes: Vec<svd_proto::Mode>,

    /// Timeout (seconds) waiting for output ready after virtual display on.
    /// Default: 30.
    pub output_ready_timeout_secs: u64,

    /// Timeout (seconds) for IPC operations.
    /// Default: 10.
    pub ipc_timeout_secs: u64,

    /// Log level. Default: "info".
    pub log_level: String,

    /// Path to the Unix socket. Default: "/run/sunshine-vd/svd.sock".
    pub socket_path: String,

    /// Path to the state file. Default: "/var/lib/sunshine-vd/state.json".
    pub state_path: String,

    /// Connectors to disable when a virtual display is connected.
    /// Example: ["DP-1", "HDMI-A-2"]
    ///
    /// Scenarios:
    ///   []                 — (default) add virtual display, leave all physical monitors on
    ///   ["DP-1"]           — disable primary monitor only, keep secondary active
    ///   ["DP-1","HDMI-A-1"]— disable all physical monitors (headless/remote streaming)
    ///
    /// Use `kscreen-doctor -o` to discover connector names.
    pub disable_outputs: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hdr: false,
            allow_master_stealing: false,
            device: None,
            extra_allowed_modes: vec![],
            output_ready_timeout_secs: 30,
            ipc_timeout_secs: 10,
            log_level: "info".to_string(),
            socket_path: "/run/sunshine-vd/svd.sock".to_string(),
            state_path: "/var/lib/sunshine-vd/state.json".to_string(),
            disable_outputs: vec![],
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Loader
// ──────────────────────────────────────────────────────────────────────────────

/// Load config from `path`.
///
/// - If the file does not exist, return safe defaults (`Config::default()`).
/// - If the file exists but cannot be read (permission denied, etc.), return
///   `Err(ConfigError::Io)`.
/// - If the file exists but contains invalid TOML or unknown fields, return
///   `Err(ConfigError::Parse)`.
#[allow(dead_code)]
pub fn load_config(path: &Path) -> Result<Config, ConfigError> {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Missing file → safe defaults.
            return Ok(Config::default());
        }
        Err(e) => {
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                source: e,
            });
        }
    };

    toml::from_str(&content).map_err(|e| ConfigError::Parse {
        path: path.to_path_buf(),
        source: e,
    })
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use svd_proto::is_mode_allowed;

    /// Write `content` to a uniquely-named temp file and return (TempHolder, path).
    /// The file is deleted when `TempHolder` is dropped.
    struct TempFile {
        path: std::path::PathBuf,
    }

    impl TempFile {
        fn new(name: &str, content: &str) -> Self {
            let mut path = std::env::temp_dir();
            // Include the test name AND a nonce to avoid parallel-test collisions.
            path.push(format!(
                "svd_config_test_{}_{}.toml",
                name,
                std::process::id()
            ));
            let mut f = std::fs::File::create(&path).expect("create temp file");
            f.write_all(content.as_bytes()).expect("write temp file");
            TempFile { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    // ── 1. Non-existent path → safe defaults ────────────────────────────────

    #[test]
    fn missing_file_returns_safe_defaults() {
        let path = std::path::Path::new("/tmp/svd_config_this_file_must_not_exist_abc123xyz.toml");
        // Guarantee it really does not exist.
        let _ = std::fs::remove_file(path);

        let cfg = load_config(path).expect("missing file should yield defaults");
        assert!(!cfg.hdr, "hdr must default to false");
        assert!(
            !cfg.allow_master_stealing,
            "allow_master_stealing must default to false"
        );
    }

    // ── 2. Empty file → safe defaults ───────────────────────────────────────

    #[test]
    fn empty_file_returns_safe_defaults() {
        let tmp = TempFile::new("empty", "");
        let cfg = load_config(tmp.path()).expect("empty TOML should yield defaults");
        assert!(!cfg.hdr);
        assert!(!cfg.allow_master_stealing);
        assert_eq!(cfg.output_ready_timeout_secs, 30);
        assert_eq!(cfg.ipc_timeout_secs, 10);
        assert_eq!(cfg.log_level, "info");
        assert_eq!(cfg.socket_path, "/run/sunshine-vd/svd.sock");
        assert_eq!(cfg.state_path, "/var/lib/sunshine-vd/state.json");
        assert!(cfg.device.is_none());
        assert!(cfg.extra_allowed_modes.is_empty());
    }

    // ── 3. hdr = true → parsed correctly ────────────────────────────────────

    #[test]
    fn hdr_true_is_parsed() {
        let tmp = TempFile::new("hdr_true", "hdr = true\n");
        let cfg = load_config(tmp.path()).expect("valid TOML");
        assert!(cfg.hdr, "hdr should be true when set in config");
        // Everything else stays at default.
        assert!(!cfg.allow_master_stealing);
    }

    // ── 4. allow_master_stealing = true → parsed correctly ──────────────────

    #[test]
    fn allow_master_stealing_true_is_parsed() {
        let tmp = TempFile::new("ams_true", "allow_master_stealing = true\n");
        let cfg = load_config(tmp.path()).expect("valid TOML");
        assert!(cfg.allow_master_stealing);
        assert!(!cfg.hdr);
    }

    // ── 5. extra_allowed_modes → populated ──────────────────────────────────

    #[test]
    fn extra_allowed_modes_are_parsed() {
        let content = r#"
[[extra_allowed_modes]]
width = 1920
height = 1080
refresh = 120
"#;
        let tmp = TempFile::new("extra_modes", content);
        let cfg = load_config(tmp.path()).expect("valid TOML");
        assert_eq!(cfg.extra_allowed_modes.len(), 1);
        let m = &cfg.extra_allowed_modes[0];
        assert_eq!(m.width, 1920);
        assert_eq!(m.height, 1080);
        assert_eq!(m.refresh, 120);
    }

    // ── 6. Unknown field → ConfigError::Parse ───────────────────────────────

    #[test]
    fn unknown_field_returns_parse_error() {
        let tmp = TempFile::new("unknown_field", "foo = \"bar\"\n");
        let result = load_config(tmp.path());
        assert!(
            matches!(result, Err(ConfigError::Parse { .. })),
            "expected ConfigError::Parse for unknown field, got: {result:?}"
        );
        // The error message must include the file path.
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains(tmp.path().to_str().unwrap()),
            "error message should contain the file path, got: {msg}"
        );
    }

    // ── 7. Malformed TOML → ConfigError::Parse ──────────────────────────────

    #[test]
    fn malformed_toml_returns_parse_error() {
        let tmp = TempFile::new("malformed", "hdr = [[[not valid toml\n");
        let result = load_config(tmp.path());
        assert!(
            matches!(result, Err(ConfigError::Parse { .. })),
            "expected ConfigError::Parse for malformed TOML, got: {result:?}"
        );
    }

    // ── 8. extra_allowed_modes widens the VIC allowlist ─────────────────────

    #[test]
    fn extra_allowed_modes_widen_vic_allowlist() {
        // 1024×768@75 is NOT in the built-in VIC table.
        let non_vic = svd_proto::Mode {
            width: 1024,
            height: 768,
            refresh: 75,
        };
        assert!(
            !is_mode_allowed(&non_vic, &[]),
            "1024x768@75 should not be in the built-in VIC table"
        );

        // Load a config that adds it via extra_allowed_modes.
        let content = r#"
[[extra_allowed_modes]]
width = 1024
height = 768
refresh = 75
"#;
        let tmp = TempFile::new("widen_allowlist", content);
        let cfg = load_config(tmp.path()).expect("valid TOML");

        // Now the mode should be allowed.
        assert!(
            is_mode_allowed(&non_vic, &cfg.extra_allowed_modes),
            "1024x768@75 should be allowed after adding to extra_allowed_modes"
        );
    }
}
