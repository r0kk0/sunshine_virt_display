//! Configuration loading for svd-daemon.
//!
//! T2.3 — TOML config with safe defaults.
//!
//! A missing file yields validated defaults. Unknown keys and values outside
//! the supported safety bounds are rejected before the daemon starts.

use std::path::Path;

// ──────────────────────────────────────────────────────────────────────────────
// Error type
// ──────────────────────────────────────────────────────────────────────────────

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

    #[error("invalid configuration in {path}: {message}")]
    Validation {
        path: std::path::PathBuf,
        message: String,
    },
}

// ──────────────────────────────────────────────────────────────────────────────
// Config struct
// ──────────────────────────────────────────────────────────────────────────────

/// Daemon configuration loaded from a TOML file.
///
/// All fields have safe defaults; a missing config file produces the same
/// result as an empty one.  Unknown keys are rejected at parse time.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Override card auto-detection. Example: "card1".
    pub device: Option<String>,

    /// Additional modes beyond the built-in VIC table.
    pub extra_allowed_modes: Vec<svd_proto::Mode>,

    /// Timeout (seconds) waiting for output ready after virtual display on.
    /// Default: 30.
    pub output_ready_timeout_secs: u64,

    /// Timeout (seconds) for IPC operations.
    /// Default: 2.
    pub ipc_timeout_secs: u64,

    /// Log level. Default: "info".
    pub log_level: String,

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
            device: None,
            extra_allowed_modes: vec![],
            output_ready_timeout_secs: 30,
            ipc_timeout_secs: 2,
            log_level: "info".to_string(),
            disable_outputs: vec![],
        }
    }
}

impl Config {
    fn validate(self, path: &Path) -> Result<Self, ConfigError> {
        let invalid = |message: String| ConfigError::Validation {
            path: path.to_path_buf(),
            message,
        };

        if !(1..=120).contains(&self.output_ready_timeout_secs) {
            return Err(invalid(
                "output_ready_timeout_secs must be between 1 and 120".into(),
            ));
        }
        if !(1..=30).contains(&self.ipc_timeout_secs) {
            return Err(invalid("ipc_timeout_secs must be between 1 and 30".into()));
        }
        if !matches!(
            self.log_level.as_str(),
            "error" | "warn" | "info" | "debug" | "trace"
        ) {
            return Err(invalid(
                "log_level must be error, warn, info, debug, or trace".into(),
            ));
        }
        if let Some(device) = &self.device {
            if !valid_card(device) {
                return Err(invalid(format!("invalid DRM device {device:?}")));
            }
        }
        if let Some(output) = self
            .disable_outputs
            .iter()
            .find(|name| !valid_connector(name))
        {
            return Err(invalid(format!("invalid connector name {output:?}")));
        }
        if let Some(mode) = self.extra_allowed_modes.iter().find(|mode| {
            !(1..=16384).contains(&mode.width)
                || !(1..=16384).contains(&mode.height)
                || !(24..=480).contains(&mode.refresh)
        }) {
            return Err(invalid(format!(
                "extra mode {}x{}@{} is outside supported bounds",
                mode.width, mode.height, mode.refresh
            )));
        }

        Ok(self)
    }
}

fn valid_card(value: &str) -> bool {
    value
        .strip_prefix("card")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
}

fn valid_connector(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
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
pub fn load_config(path: &Path) -> Result<Config, ConfigError> {
    let content = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Missing file → safe defaults.
            return Config::default().validate(path);
        }
        Err(e) => {
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                source: e,
            });
        }
    };

    let config = toml::from_str(&content).map_err(|e| ConfigError::Parse {
        path: path.to_path_buf(),
        source: e,
    })?;
    Config::validate(config, path)
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
        assert_eq!(cfg.output_ready_timeout_secs, 30);
        assert_eq!(cfg.ipc_timeout_secs, 2);
    }

    // ── 2. Empty file → safe defaults ───────────────────────────────────────

    #[test]
    fn empty_file_returns_safe_defaults() {
        let tmp = TempFile::new("empty", "");
        let cfg = load_config(tmp.path()).expect("empty TOML should yield defaults");
        assert_eq!(cfg.output_ready_timeout_secs, 30);
        assert_eq!(cfg.ipc_timeout_secs, 2);
        assert_eq!(cfg.log_level, "info");
        assert!(cfg.device.is_none());
        assert!(cfg.extra_allowed_modes.is_empty());
    }

    #[test]
    fn removed_hdr_key_is_rejected() {
        let tmp = TempFile::new("hdr_true", "hdr = true\n");
        assert!(matches!(
            load_config(tmp.path()),
            Err(ConfigError::Parse { .. })
        ));
    }

    #[test]
    fn removed_path_keys_are_rejected() {
        let tmp = TempFile::new("socket_path", "socket_path = \"/tmp/svd.sock\"\n");
        assert!(matches!(
            load_config(tmp.path()),
            Err(ConfigError::Parse { .. })
        ));
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

    #[test]
    fn zero_ipc_timeout_is_rejected() {
        let tmp = TempFile::new("zero_ipc_timeout", "ipc_timeout_secs = 0\n");
        assert!(matches!(
            load_config(tmp.path()),
            Err(ConfigError::Validation { .. })
        ));
    }

    #[test]
    fn oversized_output_timeout_is_rejected() {
        let tmp = TempFile::new("large_output_timeout", "output_ready_timeout_secs = 121\n");
        assert!(matches!(
            load_config(tmp.path()),
            Err(ConfigError::Validation { .. })
        ));
    }

    #[test]
    fn invalid_device_is_rejected() {
        let tmp = TempFile::new("bad_device", "device = \"../card0\"\n");
        assert!(matches!(
            load_config(tmp.path()),
            Err(ConfigError::Validation { .. })
        ));
    }

    #[test]
    fn invalid_connector_is_rejected() {
        let tmp = TempFile::new("bad_connector", "disable_outputs = [\"DP-1.disable\"]\n");
        assert!(matches!(
            load_config(tmp.path()),
            Err(ConfigError::Validation { .. })
        ));
    }

    #[test]
    fn invalid_log_level_is_rejected() {
        let tmp = TempFile::new("bad_log_level", "log_level = \"verbose\"\n");
        assert!(matches!(
            load_config(tmp.path()),
            Err(ConfigError::Validation { .. })
        ));
    }

    #[test]
    fn out_of_range_extra_mode_is_rejected() {
        let tmp = TempFile::new(
            "bad_mode",
            "[[extra_allowed_modes]]\nwidth = 0\nheight = 1080\nrefresh = 60\n",
        );
        assert!(matches!(
            load_config(tmp.path()),
            Err(ConfigError::Validation { .. })
        ));
    }
}
