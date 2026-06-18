//! Sysfs and debugfs I/O helpers for DRM connector management.
//!
//! All functions use `std::fs` — no subprocess, no shell.

use std::path::{Path, PathBuf};

use crate::strategy::StrategyError;

// ---------------------------------------------------------------------------
// Internal path-construction helpers (pub(crate) so tests can reach them)
// ---------------------------------------------------------------------------

/// Build the sysfs status path for a connector.
/// Format: `/sys/class/drm/{card}-{port}/status`
pub(crate) fn connector_status_path(card: &str, port: &str) -> PathBuf {
    PathBuf::from(format!("/sys/class/drm/{card}-{port}/status"))
}

/// Build the debugfs EDID override path for a connector.
/// Format: `/sys/kernel/debug/dri/{index}/{port}/edid_override`
/// where `{index}` is the numeric suffix of `card` (e.g. `card0` → `0`).
pub(crate) fn edid_override_path(card: &str, port: &str) -> Result<PathBuf, StrategyError> {
    let index = card
        .strip_prefix("card")
        .and_then(|s| {
            if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
                Some(s)
            } else {
                None
            }
        })
        .ok_or_else(|| StrategyError::Other(format!("invalid card name: {card}")))?;
    Ok(PathBuf::from(format!(
        "/sys/kernel/debug/dri/{index}/{port}/edid_override"
    )))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// List available DRM card devices, e.g. `["card0", "card1"]`.
/// Reads `/dev/dri/` and returns entries matching `card[0-9]+`.
pub fn list_drm_cards() -> Result<Vec<String>, StrategyError> {
    let mut cards = Vec::new();
    for entry in std::fs::read_dir("/dev/dri")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_drm_card_name(&name) {
            cards.push(name);
        }
    }
    cards.sort();
    Ok(cards)
}

fn is_drm_card_name(name: &str) -> bool {
    name.starts_with("card")
        && name[4..].chars().all(|c| c.is_ascii_digit())
        && !name[4..].is_empty()
}

/// Return connector names with status `"connected"` for a card.
/// Reads `/sys/class/drm/cardN-*/status` files.
/// Returns a vec of connector names like `["DP-1", "HDMI-A-1"]`.
pub fn connected_connectors(card: &str) -> Result<Vec<String>, StrategyError> {
    let prefix = format!("{card}-");
    let mut result = Vec::new();
    for entry in std::fs::read_dir("/sys/class/drm")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(connector) = name.strip_prefix(&prefix) else { continue };
        let status_path = entry.path().join("status");
        match std::fs::read_to_string(&status_path) {
            Ok(s) if s.trim() == "connected" => result.push(connector.to_owned()),
            _ => {}
        }
    }
    Ok(result)
}

/// For a given card, count connected connectors (for auto-card selection).
/// Swallows errors — returns 0 on any failure.
pub fn connected_count(card: &str) -> usize {
    connected_connectors(card).map(|v| v.len()).unwrap_or(0)
}

/// Find the first DP or HDMI connector with status `"disconnected"`.
/// Preference order: DisplayPort first (`DP-`), then HDMI (`HDMI-`).
/// Connectors within each group are sorted lexically.
/// Returns a connector name like `"DP-3"`.
pub fn find_empty_slot(card: &str) -> Result<String, StrategyError> {
    let prefix = format!("{card}-");
    let mut dp_slots: Vec<String> = Vec::new();
    let mut hdmi_slots: Vec<String> = Vec::new();

    for entry in std::fs::read_dir("/sys/class/drm")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(connector) = name.strip_prefix(&prefix) else { continue };
        let status_path = entry.path().join("status");
        let status = match std::fs::read_to_string(&status_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Accept "disconnected" (never had a display) or "off" (we wrote "off"
        // in a previous disconnect to remove our virtual display).
        if !matches!(status.trim(), "disconnected" | "off") {
            continue;
        }
        if connector.starts_with("DP-") {
            dp_slots.push(connector.to_owned());
        } else if connector.starts_with("HDMI-") {
            hdmi_slots.push(connector.to_owned());
        }
    }

    dp_slots.sort();
    hdmi_slots.sort();

    dp_slots
        .into_iter()
        .chain(hdmi_slots)
        .next()
        .ok_or(StrategyError::NoSlot)
}

/// Set a connector's sysfs status to `"on"` or `"off"`.
/// Writes to `/sys/class/drm/cardN-PORT/status`.
pub fn set_connector_status(card: &str, port: &str, on: bool) -> Result<(), StrategyError> {
    let path = connector_status_path(card, port);
    let value = if on { "on" } else { "off" };
    std::fs::write(&path, value)?;
    Ok(())
}

/// Write EDID bytes to the debugfs override for a connector.
/// Path: `/sys/kernel/debug/dri/<card_index>/PORT/edid_override`
/// where `card_index` is the number from the card name (`card0` → `0`).
pub fn write_edid_override(card: &str, port: &str, edid: &[u8]) -> Result<(), StrategyError> {
    let path = edid_override_path(card, port)?;
    std::fs::write(&path, edid)?;
    Ok(())
}

/// Clear the EDID override (write empty bytes).
pub fn clear_edid_override(card: &str, port: &str) -> Result<(), StrategyError> {
    let path = edid_override_path(card, port)?;
    std::fs::write(&path, b"")?;
    Ok(())
}

/// Remove stale KWin output config entry for a port.
/// KWin stores per-connector config in `~/.config/kwinoutputconfig.json`.
///
/// `uid`: the real user's uid — used to locate their home directory by
/// parsing `/etc/passwd`. For uid 0, falls back to `/root`.
/// If the file does not exist or the port is not present, returns `Ok(())`.
pub fn clear_kwin_output_config(port: &str, uid: u32) -> Result<(), StrategyError> {
    let home = match find_home_for_uid(uid) {
        Some(h) => h,
        None => return Ok(()),
    };
    let config_path = Path::new(&home).join(".config").join("kwinoutputconfig.json");
    let contents = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(StrategyError::Io(e)),
    };
    if let Some(rewritten) = filter_kwin_output_config(&contents, port) {
        std::fs::write(&config_path, rewritten)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parse `/etc/passwd` to find the home directory for `uid`.
/// Line format: `name:password:uid:gid:gecos:home:shell`
/// Returns `Some(home)` on first match, `None` if not found.
/// For uid 0 with no `/etc/passwd` entry, falls back to `/root`.
fn find_home_for_uid(uid: u32) -> Option<String> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in passwd.lines() {
        let fields: Vec<&str> = line.splitn(7, ':').collect();
        if fields.len() < 7 {
            continue;
        }
        let line_uid: u32 = match fields[2].parse() {
            Ok(n) => n,
            Err(_) => continue,
        };
        if line_uid == uid {
            return Some(fields[5].to_owned());
        }
    }
    // Fallback for root when /etc/passwd has no matching entry
    if uid == 0 {
        return Some("/root".to_owned());
    }
    None
}

/// Transform the contents of `kwinoutputconfig.json`, removing any entry
/// whose `"name"` field equals `port`.
///
/// Handles both formats:
/// - bare array: `[{...}, ...]`
/// - object with outputs key: `{"outputs": [{...}, ...]}`
///
/// Returns `Some(new_json)` when an entry was removed, `None` when unchanged.
/// Returns `None` on any parse failure (best-effort cleanup).
pub(crate) fn filter_kwin_output_config(contents: &str, port: &str) -> Option<String> {
    let data: serde_json::Value = serde_json::from_str(contents).ok()?;

    match data {
        serde_json::Value::Array(mut arr) => {
            let before = arr.len();
            arr.retain(|item| item.get("name").and_then(|n| n.as_str()) != Some(port));
            if arr.len() < before {
                Some(serde_json::to_string_pretty(&arr).ok()?)
            } else {
                None
            }
        }
        serde_json::Value::Object(mut map) => {
            let outputs = map.get_mut("outputs")?.as_array_mut()?;
            let before = outputs.len();
            outputs.retain(|item| item.get("name").and_then(|n| n.as_str()) != Some(port));
            if outputs.len() < before {
                Some(serde_json::to_string_pretty(&serde_json::Value::Object(map)).ok()?)
            } else {
                None
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Path construction ---

    #[test]
    fn debugfs_edid_path_for_card0() {
        let path = edid_override_path("card0", "DP-1").unwrap();
        assert_eq!(
            path,
            PathBuf::from("/sys/kernel/debug/dri/0/DP-1/edid_override")
        );
    }

    #[test]
    fn debugfs_edid_path_for_card1() {
        let path = edid_override_path("card1", "HDMI-A-1").unwrap();
        assert_eq!(
            path,
            PathBuf::from("/sys/kernel/debug/dri/1/HDMI-A-1/edid_override")
        );
    }

    #[test]
    fn debugfs_edid_path_invalid_card_name() {
        assert!(edid_override_path("gpu0", "DP-1").is_err());
        assert!(edid_override_path("card", "DP-1").is_err());
        assert!(edid_override_path("card0abc", "DP-1").is_err());
    }

    #[test]
    fn sysfs_connector_status_path() {
        let path = connector_status_path("card1", "DP-2");
        assert_eq!(
            path,
            PathBuf::from("/sys/class/drm/card1-DP-2/status")
        );
    }

    #[test]
    fn sysfs_connector_status_path_hdmi() {
        let path = connector_status_path("card0", "HDMI-A-1");
        assert_eq!(
            path,
            PathBuf::from("/sys/class/drm/card0-HDMI-A-1/status")
        );
    }

    // --- DRM card name detection ---

    #[test]
    fn is_drm_card_name_accepts_valid() {
        assert!(is_drm_card_name("card0"));
        assert!(is_drm_card_name("card1"));
        assert!(is_drm_card_name("card10"));
    }

    #[test]
    fn is_drm_card_name_rejects_invalid() {
        assert!(!is_drm_card_name("card"));       // no digits
        assert!(!is_drm_card_name("renderD128")); // wrong prefix
        assert!(!is_drm_card_name("card0abc"));   // non-digit suffix
        assert!(!is_drm_card_name("controlD64")); // wrong prefix
    }

    // --- kwinoutputconfig.json filtering ---

    #[test]
    fn filter_kwin_output_config_bare_array_removes_matching_entry() {
        let json = r#"[{"name":"DP-2","scale":1.5},{"name":"HDMI-A-1","scale":1.0}]"#;
        let result = filter_kwin_output_config(json, "DP-2").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "HDMI-A-1");
    }

    #[test]
    fn filter_kwin_output_config_bare_array_no_match_returns_none() {
        let json = r#"[{"name":"DP-1","scale":1.0}]"#;
        let result = filter_kwin_output_config(json, "DP-99");
        assert!(result.is_none());
    }

    #[test]
    fn filter_kwin_output_config_object_removes_matching_entry() {
        let json = r#"{"outputs":[{"name":"DP-3","scale":1.0},{"name":"DP-1","scale":2.0}],"version":2}"#;
        let result = filter_kwin_output_config(json, "DP-3").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let outputs = parsed["outputs"].as_array().unwrap();
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0]["name"], "DP-1");
        // Other fields preserved
        assert_eq!(parsed["version"], 2);
    }

    #[test]
    fn filter_kwin_output_config_object_no_match_returns_none() {
        let json = r#"{"outputs":[{"name":"DP-1","scale":1.0}]}"#;
        let result = filter_kwin_output_config(json, "DP-99");
        assert!(result.is_none());
    }

    #[test]
    fn filter_kwin_output_config_invalid_json_returns_none() {
        let result = filter_kwin_output_config("not valid json {", "DP-1");
        assert!(result.is_none());
    }

    #[test]
    fn filter_kwin_output_config_empty_array_no_match() {
        let json = r#"[]"#;
        let result = filter_kwin_output_config(json, "DP-1");
        assert!(result.is_none());
    }

    // --- passwd parsing helper ---

    #[test]
    fn find_home_for_uid_uses_etc_passwd_fallback_logic() {
        // We can't rely on /etc/passwd content in test environments, but we
        // can verify the uid-0 fallback works when no entry matched.
        // The function reads the real /etc/passwd — if root is present the home
        // is whatever /etc/passwd says; if not, it falls back to "/root".
        // Either way the function must not panic.
        let home = find_home_for_uid(0);
        // uid 0 always resolves (either from /etc/passwd or fallback)
        assert!(home.is_some());
        let h = home.unwrap();
        assert!(!h.is_empty());
    }

    #[test]
    fn find_home_for_uid_nonexistent_uid_returns_none() {
        // uid u32::MAX should never exist in /etc/passwd
        let home = find_home_for_uid(u32::MAX);
        assert!(home.is_none());
    }
}
