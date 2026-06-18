use serde::{Deserialize, Serialize};
use std::path::Path;

use super::kscreen::OutputInfo;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectState {
    pub card: String,
    pub virtual_port: String,
    /// Full layout snapshot taken before connect. Used to restore exact
    /// output positions and enabled states on disconnect.
    /// Empty on state files written before this field was added.
    #[serde(default)]
    pub previous_layout: Vec<OutputInfo>,
    /// Legacy field from initial release. Superseded by previous_layout.
    /// Kept for reading old state files; not written to new files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub previous_ports: Vec<String>,
    pub edid_override_path: String,
}

impl ConnectState {
    /// Serialize state to JSON and write atomically via a .json.tmp temp file,
    /// then rename. Creates parent directories if they do not exist.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp_path = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&tmp_path, json)?;
        std::fs::rename(&tmp_path, path)?;
        Ok(())
    }

    /// Load state from JSON file. Returns `Ok(None)` when the file does not
    /// exist; propagates any other I/O or parse error.
    pub fn load(path: &Path) -> std::io::Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(contents) => {
                let state: Self = serde_json::from_str(&contents)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
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

    #[test]
    fn round_trip() {
        let state = ConnectState {
            card: "card0".into(),
            virtual_port: "DP-3".into(),
            previous_layout: vec![
                OutputInfo { name: "DP-1".into(), enabled: true, x: 0, y: 0, width: 1920, height: 1080 },
                OutputInfo { name: "HDMI-A-1".into(), enabled: false, x: -2560, y: 0, width: 2560, height: 1440 },
            ],
            previous_ports: vec![],
            edid_override_path: "/sys/kernel/debug/dri/0/DP-3/edid_override".into(),
        };
        let dir = std::env::temp_dir();
        let path = dir.join(format!("svd_state_test_{}.json", std::process::id()));
        state.save(&path).unwrap();
        let loaded = ConnectState::load(&path).unwrap().unwrap();
        assert_eq!(loaded.card, "card0");
        assert_eq!(loaded.virtual_port, "DP-3");
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
    fn legacy_state_loads_with_empty_previous_layout() {
        // Old state files have previous_ports but no previous_layout.
        let json = r#"{
            "card": "card1",
            "virtual_port": "DP-1",
            "previous_ports": ["DP-2"],
            "edid_override_path": "/sys/kernel/debug/dri/1/DP-1/edid_override"
        }"#;
        let dir = std::env::temp_dir();
        let path = dir.join(format!("svd_legacy_state_test_{}.json", std::process::id()));
        std::fs::write(&path, json).unwrap();
        let loaded = ConnectState::load(&path).unwrap().unwrap();
        assert_eq!(loaded.previous_layout.len(), 0);
        assert_eq!(loaded.previous_ports, vec!["DP-2"]);
        let _ = std::fs::remove_file(&path);
    }
}
