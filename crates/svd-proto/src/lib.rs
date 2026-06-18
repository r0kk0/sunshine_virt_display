// Shared types for sunshine-virt-display.
pub mod framing;
// T2.1 — IPC Request/Response types + mode allowlist + input validation.
//
// Security requirements:
//   - All Request variants use #[serde(deny_unknown_fields)] so that unknown
//     keys are rejected at deserialization time, never silently dropped.
//   - (width, height, refresh) must be on a known-good allowlist; numeric
//     bounds alone are not sufficient.
//   - Device names must match ^card[0-9]+$ and must not contain path traversal
//     characters, NUL bytes, or control characters.

// ──────────────────────────────────────────────────────────────────────────────
// Request
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Connect {
        width: u32,
        height: u32,
        refresh: u32,
        #[serde(default)]
        device: Option<String>,
        #[serde(default)]
        dry_run: bool,
        /// Disable all active physical monitors before connecting the virtual
        /// display. Use for remote headless streaming. Without this flag,
        /// physical monitors are left on.
        #[serde(default)]
        exclusive: bool,
    },
    /// Empty struct variants (not unit variants) so that `deny_unknown_fields`
    /// applies uniformly.  Wire format is unchanged: serializes as
    /// `{"cmd":"disconnect"}` etc.  Unit variants skip the struct deserializer
    /// in serde's internally-tagged enum implementation and would silently
    /// accept extra keys — exactly what the security requirement forbids.
    Disconnect {},
    Status {},
    Restore {},
}

// ──────────────────────────────────────────────────────────────────────────────
// Response
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Response {
    Connect {
        ok: bool,
        connector: Option<String>,
        card: Option<String>,
        mode: Option<String>,
        error: Option<String>,
        message: Option<String>,
    },
    Disconnect {
        ok: bool,
        error: Option<String>,
    },
    Status {
        ok: bool,
        connected: bool,
        card: Option<String>,
        connector: Option<String>,
        mode: Option<String>,
        strategy: Option<String>,
    },
    Restore {
        ok: bool,
        error: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecyclePhase {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
    RecoveryRequired,
}

// ──────────────────────────────────────────────────────────────────────────────
// Mode
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CardId(String);

impl CardId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for CardId {
    type Error = &'static str;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let suffix = value.strip_prefix("card").ok_or("invalid_device")?;
        if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err("invalid_device");
        }
        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for CardId {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl From<CardId> for String {
    fn from(value: CardId) -> Self {
        value.0
    }
}

impl std::fmt::Display for CardId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ConnectorId(String);

impl ConnectorId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<&str> for ConnectorId {
    type Error = &'static str;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("invalid_connector");
        }
        Ok(Self(value.to_owned()))
    }
}

impl TryFrom<String> for ConnectorId {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl From<ConnectorId> for String {
    fn from(value: ConnectorId) -> Self {
        value.0
    }
}

impl std::fmt::Display for ConnectorId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod identifier_tests {
    use super::{CardId, ConnectorId};

    #[test]
    fn card_id_accepts_kernel_card_names() {
        assert_eq!(
            CardId::try_from("card12").expect("valid").as_str(),
            "card12"
        );
    }

    #[test]
    fn card_id_rejects_path_and_empty_suffix() {
        assert!(CardId::try_from("card").is_err());
        assert!(CardId::try_from("../card0").is_err());
    }

    #[test]
    fn connector_id_accepts_common_drm_names() {
        for name in ["DP-1", "HDMI-A-2", "eDP-1", "Virtual_1"] {
            assert_eq!(ConnectorId::try_from(name).expect("valid").as_str(), name);
        }
    }

    #[test]
    fn connector_id_rejects_command_and_path_syntax() {
        for name in ["", "DP-1.disable", "../DP-1", "DP/1", "DP-1\n"] {
            assert!(ConnectorId::try_from(name).is_err(), "accepted {name:?}");
        }
    }

    #[test]
    fn identifiers_validate_during_deserialization() {
        assert!(serde_json::from_str::<CardId>("\"card0\"").is_ok());
        assert!(serde_json::from_str::<CardId>("\"../card0\"").is_err());
        assert!(serde_json::from_str::<ConnectorId>("\"DP-1\"").is_ok());
        assert!(serde_json::from_str::<ConnectorId>("\"DP-1.disable\"").is_err());
    }
}

impl Mode {
    const fn new(width: u32, height: u32, refresh: u32) -> Self {
        Self {
            width,
            height,
            refresh,
        }
    }
}

/// Built-in VIC allowlist (minimum required by T2.1).
///
/// A display mode is only considered safe if it appears here (or is explicitly
/// provided by the caller via `extra_allowed`).  Numeric range alone is not a
/// sufficient guard because off-the-wall modes can confuse drivers.
static VIC_TABLE: &[Mode] = &[
    // 720p
    Mode::new(1280, 720, 30),
    Mode::new(1280, 720, 60),
    // 1080p
    Mode::new(1920, 1080, 30),
    Mode::new(1920, 1080, 60),
    Mode::new(1920, 1080, 120),
    // 1440p
    Mode::new(2560, 1440, 60),
    Mode::new(2560, 1440, 120),
    // 4K
    Mode::new(3840, 2160, 30),
    Mode::new(3840, 2160, 60),
];

/// Returns `true` when `mode` is in the built-in VIC table **or** in
/// `extra_allowed`.
pub fn is_mode_allowed(mode: &Mode, extra_allowed: &[Mode]) -> bool {
    VIC_TABLE.contains(mode) || extra_allowed.contains(mode)
}

// ──────────────────────────────────────────────────────────────────────────────
// Validation
// ──────────────────────────────────────────────────────────────────────────────

/// Returns `true` when `s` contains a path-traversal sequence, NUL byte, or
/// any control character.
fn has_disallowed_chars(s: &str) -> bool {
    s.contains('/') || s.contains("..") || s.chars().any(|c| c.is_control())
}

/// Checks that a device name matches `^card[0-9]+$`.
fn is_valid_device(s: &str) -> bool {
    CardId::try_from(s).is_ok()
}

/// Validate a [`Request`] checking only numeric bounds and device format.
///
/// This is the validation the CLI performs locally before sending to the
/// daemon. It intentionally skips the mode allowlist so that modes added via
/// `extra_allowed_modes` in the daemon config are not rejected client-side.
/// The daemon performs full validation via [`validate_request`].
pub fn validate_request_format(req: &Request) -> Result<(), &'static str> {
    match req {
        Request::Connect {
            width,
            height,
            refresh,
            device,
            ..
        } => {
            if *width < 1 || *width > 16384 {
                return Err("out_of_range");
            }
            if *height < 1 || *height > 16384 {
                return Err("out_of_range");
            }
            if *refresh < 24 || *refresh > 480 {
                return Err("out_of_range");
            }
            if let Some(dev) = device {
                if has_disallowed_chars(dev) {
                    return Err("invalid_input");
                }
                if !is_valid_device(dev) {
                    return Err("invalid_device");
                }
            }
            Ok(())
        }
        Request::Disconnect {} | Request::Status {} | Request::Restore {} => Ok(()),
    }
}

/// Validate a [`Request`].  Returns `Err` with an IPC error-code string on
/// failure.
///
/// Validation order (important — earlier checks take precedence):
/// 1. **out_of_range**  — numeric sanity pre-filter (1 ≤ w,h ≤ 16384,
///    24 ≤ refresh ≤ 480).  Must run *before* allowlist so that an
///    obviously-bogus value produces `out_of_range`, not `mode_not_allowed`.
/// 2. **mode_not_allowed** — (width, height, refresh) must be on the VIC
///    table or in `extra_allowed`.
/// 3. **invalid_input** — device/string fields must not contain `/`, `..`,
///    NUL, or other control characters.  Must run before the regex check so
///    that a NUL in `device` returns `invalid_input`, not `invalid_device`.
/// 4. **invalid_device** — device must match `^card[0-9]+$`.
pub fn validate_request(req: &Request, extra_allowed: &[Mode]) -> Result<(), &'static str> {
    match req {
        Request::Connect {
            width,
            height,
            refresh,
            device,
            ..
        } => {
            // 1. Numeric sanity bounds (pre-filter).
            if *width < 1 || *width > 16384 {
                return Err("out_of_range");
            }
            if *height < 1 || *height > 16384 {
                return Err("out_of_range");
            }
            if *refresh < 24 || *refresh > 480 {
                return Err("out_of_range");
            }

            // 2. Mode allowlist.
            let mode = Mode {
                width: *width,
                height: *height,
                refresh: *refresh,
            };
            if !is_mode_allowed(&mode, extra_allowed) {
                return Err("mode_not_allowed");
            }

            // 3 & 4. Device field validation.
            if let Some(dev) = device {
                // 3. Disallowed characters check (invalid_input before regex).
                if has_disallowed_chars(dev) {
                    return Err("invalid_input");
                }
                // 4. Must match ^card[0-9]+$.
                if !is_valid_device(dev) {
                    return Err("invalid_device");
                }
            }

            Ok(())
        }

        // Disconnect / Status / Restore carry no user-supplied data.
        Request::Disconnect {} | Request::Status {} | Request::Restore {} => Ok(()),
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn connect(width: u32, height: u32, refresh: u32) -> Request {
        Request::Connect {
            width,
            height,
            refresh,
            device: None,
            dry_run: false,
            exclusive: false,
        }
    }

    fn connect_dev(width: u32, height: u32, refresh: u32, dev: &str) -> Request {
        Request::Connect {
            width,
            height,
            refresh,
            device: Some(dev.to_owned()),
            dry_run: false,
            exclusive: false,
        }
    }

    // ── 1. serde round-trip for each Request variant ──────────────────────────

    #[test]
    fn roundtrip_connect() {
        let req = connect(1920, 1080, 60);
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        match back {
            Request::Connect {
                width,
                height,
                refresh,
                device,
                dry_run,
                ..
            } => {
                assert_eq!((width, height, refresh), (1920, 1080, 60));
                assert_eq!(device, None);
                assert!(!dry_run);
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }

    #[test]
    fn roundtrip_connect_with_device_and_dry_run() {
        let req = Request::Connect {
            width: 1280,
            height: 720,
            refresh: 60,
            device: Some("card0".to_owned()),
            dry_run: true,
            exclusive: true,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        match back {
            Request::Connect {
                width,
                height,
                refresh,
                device,
                dry_run,
                exclusive,
            } => {
                assert_eq!((width, height, refresh), (1280, 720, 60));
                assert_eq!(device.as_deref(), Some("card0"));
                assert!(dry_run);
                assert!(exclusive);
            }
            _ => panic!("wrong variant after round-trip"),
        }
    }

    // ── 11. Disconnect / Status / Restore round-trip ─────────────────────────

    #[test]
    fn roundtrip_disconnect() {
        let req = Request::Disconnect {};
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::Disconnect { .. }));
    }

    #[test]
    fn roundtrip_status() {
        let req = Request::Status {};
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::Status { .. }));
    }

    #[test]
    fn roundtrip_restore() {
        let req = Request::Restore {};
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::Restore { .. }));
    }

    // ── 2. Unknown field → deserialization error (deny_unknown_fields) ────────

    #[test]
    fn unknown_field_in_connect_is_rejected() {
        // The extra "evil" key must cause a deserialization error, not be
        // silently ignored.  This is the core security test for the
        // deny_unknown_fields constraint.
        let json = r#"{"cmd":"connect","width":1280,"height":720,"refresh":60,"evil":1}"#;
        let result: Result<Request, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "expected error for unknown field 'evil', got Ok"
        );
    }

    #[test]
    fn unknown_field_in_disconnect_is_rejected() {
        // Works because Disconnect is an empty *struct* variant, not a unit
        // variant.  Unit variants bypass the struct deserializer in serde's
        // internally-tagged enum path and would silently accept extra keys.
        let json = r#"{"cmd":"disconnect","evil":true}"#;
        let result: Result<Request, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "expected error for unknown field in Disconnect"
        );
    }

    #[test]
    fn unknown_field_in_status_is_rejected() {
        let json = r#"{"cmd":"status","evil":true}"#;
        let result: Result<Request, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "expected error for unknown field in Status"
        );
    }

    #[test]
    fn unknown_field_in_restore_is_rejected() {
        let json = r#"{"cmd":"restore","evil":true}"#;
        let result: Result<Request, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "expected error for unknown field in Restore"
        );
    }

    // ── 3. Mode on VIC allowlist → Ok(()) ────────────────────────────────────

    #[test]
    fn vic_mode_accepted() {
        // 1920×1080@60 is in the VIC table.
        assert_eq!(validate_request(&connect(1920, 1080, 60), &[]), Ok(()));
    }

    #[test]
    fn all_vic_modes_accepted() {
        for mode in VIC_TABLE {
            let req = connect(mode.width, mode.height, mode.refresh);
            assert_eq!(
                validate_request(&req, &[]),
                Ok(()),
                "VIC mode {mode:?} should be accepted"
            );
        }
    }

    // ── 4. Mode NOT on allowlist → Err("mode_not_allowed") ───────────────────

    #[test]
    fn off_allowlist_mode_rejected() {
        // 1024×768@75 is not in the VIC table.
        assert_eq!(
            validate_request(&connect(1024, 768, 75), &[]),
            Err("mode_not_allowed")
        );
    }

    // ── 5. Mode in extra_allowed → Ok(()) ────────────────────────────────────

    #[test]
    fn extra_allowed_mode_accepted() {
        let extra = vec![Mode {
            width: 1024,
            height: 768,
            refresh: 75,
        }];
        assert_eq!(validate_request(&connect(1024, 768, 75), &extra), Ok(()));
    }

    // ── 6. device = "../etc/passwd" → Err("invalid_input") or Err("invalid_device")

    #[test]
    fn path_traversal_in_device_rejected() {
        let req = connect_dev(1920, 1080, 60, "../etc/passwd");
        let result = validate_request(&req, &[]);
        assert!(
            result == Err("invalid_input") || result == Err("invalid_device"),
            "expected invalid_input or invalid_device, got {result:?}"
        );
    }

    // ── 7. device = "card1" → Ok(()) ─────────────────────────────────────────

    #[test]
    fn valid_device_card1_accepted() {
        let req = connect_dev(1920, 1080, 60, "card1");
        assert_eq!(validate_request(&req, &[]), Ok(()));
    }

    #[test]
    fn valid_device_card0_accepted() {
        let req = connect_dev(1920, 1080, 60, "card0");
        assert_eq!(validate_request(&req, &[]), Ok(()));
    }

    // ── 8. NUL in device → Err("invalid_input") ──────────────────────────────

    #[test]
    fn nul_in_device_is_invalid_input() {
        let req = connect_dev(1920, 1080, 60, "card\x001");
        assert_eq!(validate_request(&req, &[]), Err("invalid_input"));
    }

    // ── 9. width=0 → Err("out_of_range") ─────────────────────────────────────

    #[test]
    fn width_zero_is_out_of_range() {
        // width=0 is below the sanity bound; must return out_of_range, not
        // mode_not_allowed (numeric pre-filter runs before allowlist check).
        assert_eq!(
            validate_request(&connect(0, 1080, 60), &[]),
            Err("out_of_range")
        );
    }

    // ── 10. refresh=999 → Err("out_of_range") ────────────────────────────────

    #[test]
    fn refresh_999_is_out_of_range() {
        // refresh=999 is above the sanity bound; same ordering rule applies.
        assert_eq!(
            validate_request(&connect(1920, 1080, 999), &[]),
            Err("out_of_range")
        );
    }

    // ── edge cases ────────────────────────────────────────────────────────────

    #[test]
    fn invalid_device_no_digits_rejected() {
        let req = connect_dev(1920, 1080, 60, "card");
        assert_eq!(validate_request(&req, &[]), Err("invalid_device"));
    }

    #[test]
    fn invalid_device_wrong_prefix_rejected() {
        let req = connect_dev(1920, 1080, 60, "gpu0");
        assert_eq!(validate_request(&req, &[]), Err("invalid_device"));
    }

    #[test]
    fn disconnect_always_valid() {
        assert_eq!(validate_request(&Request::Disconnect {}, &[]), Ok(()));
    }

    #[test]
    fn status_always_valid() {
        assert_eq!(validate_request(&Request::Status {}, &[]), Ok(()));
    }

    #[test]
    fn restore_always_valid() {
        assert_eq!(validate_request(&Request::Restore {}, &[]), Ok(()));
    }

    #[test]
    fn height_zero_is_out_of_range() {
        assert_eq!(
            validate_request(&connect(1920, 0, 60), &[]),
            Err("out_of_range")
        );
    }

    #[test]
    fn refresh_below_minimum_is_out_of_range() {
        // 23 is below the minimum of 24.
        assert_eq!(
            validate_request(&connect(1920, 1080, 23), &[]),
            Err("out_of_range")
        );
    }

    #[test]
    fn width_above_max_is_out_of_range() {
        assert_eq!(
            validate_request(&connect(16385, 1080, 60), &[]),
            Err("out_of_range")
        );
    }

    #[test]
    fn height_above_max_is_out_of_range() {
        assert_eq!(
            validate_request(&connect(1920, 16385, 60), &[]),
            Err("out_of_range")
        );
    }

    #[test]
    fn u32_overflow_rejected_by_serde() {
        // Values exceeding u32::MAX are rejected at deserialization before validation.
        let json = r#"{"cmd":"connect","width":99999999999,"height":1080,"refresh":60}"#;
        assert!(serde_json::from_str::<Request>(json).is_err());
    }
}
