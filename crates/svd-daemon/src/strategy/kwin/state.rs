use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};
use svd_proto::{CardId, ConnectorId, LifecyclePhase, Mode};

use super::kscreen::OutputInfo;

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectState {
    pub schema_version: u32,
    pub phase: LifecyclePhase,
    pub card: CardId,
    pub virtual_port: ConnectorId,
    pub mode: Mode,
    pub session_uid: u32,
    pub previous_layout: Vec<OutputInfo>,
}

impl ConnectState {
    /// Serialize state to JSON and write atomically via a .json.tmp temp file,
    /// then rename. Creates parent directories if they do not exist.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp_path = path.with_extension(format!("tmp.{}", std::process::id()));
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&tmp_path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp_path, path)?;
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    }

    /// Load state from JSON file. Returns `Ok(None)` when the file does not
    /// exist; propagates any other I/O or parse error.
    pub fn load(path: &Path) -> std::io::Result<Option<Self>> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !metadata.file_type().is_file()
            || metadata.file_type().is_symlink()
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "state file must be a non-writable regular file",
            ));
        }
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                let state: Self = serde_json::from_str(&contents)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                if state.schema_version != CURRENT_SCHEMA_VERSION {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unsupported state schema {}", state.schema_version),
                    ));
                }
                if state
                    .previous_layout
                    .iter()
                    .any(|output| ConnectorId::try_from(output.name.as_str()).is_err())
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "state contains an invalid connector",
                    ));
                }
                Ok(Some(state))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Remove the state file. Returns `Ok(())` if the file does not exist.
    pub fn delete(path: &Path) -> std::io::Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use svd_proto::{CardId, ConnectorId, Mode};

    fn state() -> ConnectState {
        ConnectState {
            schema_version: CURRENT_SCHEMA_VERSION,
            phase: LifecyclePhase::Connected,
            card: CardId::try_from("card0").unwrap(),
            virtual_port: ConnectorId::try_from("DP-3").unwrap(),
            mode: Mode {
                width: 1920,
                height: 1080,
                refresh: 60,
            },
            session_uid: 1000,
            previous_layout: vec![
                OutputInfo {
                    name: "DP-1".into(),
                    enabled: true,
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
                OutputInfo {
                    name: "HDMI-A-1".into(),
                    enabled: false,
                    x: -2560,
                    y: 0,
                    width: 2560,
                    height: 1440,
                },
            ],
        }
    }

    #[test]
    fn round_trip() {
        let state = state();
        let dir = std::env::temp_dir();
        let path = dir.join(format!("svd_state_test_{}.json", std::process::id()));
        state.save(&path).unwrap();
        let loaded = ConnectState::load(&path).unwrap().unwrap();
        assert_eq!(loaded.card.as_str(), "card0");
        assert_eq!(loaded.virtual_port.as_str(), "DP-3");
        assert_eq!(loaded.phase, LifecyclePhase::Connected);
        assert_eq!(loaded.mode.refresh, 60);
        assert_eq!(loaded.previous_layout.len(), 2);
        assert_eq!(loaded.previous_layout[0].name, "DP-1");
        assert!(loaded.previous_layout[0].enabled);
        assert_eq!(loaded.previous_layout[1].name, "HDMI-A-1");
        assert!(!loaded.previous_layout[1].enabled);
        assert_eq!(loaded.previous_layout[1].x, -2560);
        ConnectState::delete(&path).unwrap();
        assert!(ConnectState::load(&path).unwrap().is_none());
    }

    #[test]
    fn unsupported_schema_is_rejected() {
        let json = r#"{
            "schema_version": 99,
            "phase": "connected",
            "card": "card0",
            "virtual_port": "DP-1",
            "mode": {"width": 1920, "height": 1080, "refresh": 60},
            "session_uid": 1000,
            "previous_layout": []
        }"#;
        let dir = std::env::temp_dir();
        let path = dir.join(format!("svd_schema_state_test_{}.json", std::process::id()));
        std::fs::write(&path, json).unwrap();
        assert!(ConnectState::load(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn state_file_is_owner_only() {
        let path = std::env::temp_dir().join(format!("svd_state_mode_{}.json", std::process::id()));
        state().save(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn group_writable_state_is_rejected() {
        let path =
            std::env::temp_dir().join(format!("svd_state_unsafe_{}.json", std::process::id()));
        state().save(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o620)).unwrap();
        assert!(ConnectState::load(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
