pub mod edid;
pub mod env;
pub mod kscreen;
pub mod state;
pub mod sysfs;

use std::path::PathBuf;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::strategy::kwin::state::ConnectState;
use crate::strategy::{
    ConnectParams, ConnectResult, DisplayStrategy, StrategyError, StrategyStatus,
};
use svd_proto::{CardId, ConnectorId, LifecyclePhase, Mode};

fn resolve_candidates<F>(
    candidates: Vec<env::KWinEnv>,
    mut probe: F,
) -> Result<env::KWinEnv, StrategyError>
where
    F: FnMut(&env::KWinEnv) -> Result<String, StrategyError>,
{
    if candidates.is_empty() {
        return Err(StrategyError::CompositorNotFound);
    }

    let mut successes = Vec::new();
    let mut last_error = None;
    for candidate in candidates {
        match probe(&candidate) {
            Ok(_) => successes.push(candidate),
            Err(error) => {
                tracing::debug!(
                    uid = candidate.uid,
                    wayland_display = %candidate.wayland_display,
                    %error,
                    "discarding Wayland candidate after failed KScreen probe"
                );
                last_error = Some(error);
            }
        }
    }

    match successes.len() {
        0 => Err(last_error.unwrap_or(StrategyError::CompositorNotFound)),
        1 => Ok(successes.remove(0)),
        _ => Err(StrategyError::AmbiguousCompositor),
    }
}

fn resolve_compositor(requester_uid: Option<u32>) -> Result<env::KWinEnv, StrategyError> {
    let candidates = env::discover_for_uid(requester_uid)?;
    resolve_candidates(candidates, kscreen::list_outputs)
}

// ---------------------------------------------------------------------------
// KWinStrategy
// ---------------------------------------------------------------------------

pub struct KWinStrategy {
    state_path: PathBuf,
    output_ready_timeout_secs: u64,
    /// Explicit list of connectors to disable on connect (non-exclusive mode).
    /// Empty = don't disable anything unless --exclusive is set.
    disable_outputs: Vec<String>,
    default_device: Option<String>,
    operation: Mutex<()>,
    state: RwLock<Option<ConnectState>>,
}

impl KWinStrategy {
    pub fn new(
        state_path: PathBuf,
        output_ready_timeout_secs: u64,
        disable_outputs: Vec<String>,
        default_device: Option<String>,
    ) -> Self {
        KWinStrategy {
            state_path,
            output_ready_timeout_secs,
            disable_outputs,
            default_device,
            operation: Mutex::new(()),
            state: RwLock::new(None),
        }
    }

    fn cache(&self, state: Option<ConnectState>) {
        *self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = state;
    }

    fn cleanup_state(
        &self,
        state: &ConnectState,
        kwin_env: Option<&env::KWinEnv>,
    ) -> Result<(), StrategyError> {
        let mut first_error = None;
        if let Some(kwin_env) = kwin_env {
            let args = build_restore_args(state);
            let args_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            if let Err(error) = kscreen::run_twice(kwin_env, &args_refs, 500) {
                first_error = Some(error);
            }
        } else {
            first_error = Some(StrategyError::CompositorNotFound);
        }
        if let Err(error) =
            sysfs::set_connector_status(state.card.as_str(), state.virtual_port.as_str(), false)
        {
            first_error.get_or_insert(error);
        }
        if let Err(error) =
            sysfs::clear_edid_override(state.card.as_str(), state.virtual_port.as_str())
        {
            first_error.get_or_insert(error);
        }
        first_error.map_or(Ok(()), Err)
    }

    fn failed_connect(
        &self,
        mut state: ConnectState,
        kwin_env: &env::KWinEnv,
        source: StrategyError,
    ) -> StrategyError {
        match self.cleanup_state(&state, Some(kwin_env)) {
            Ok(()) => {
                let _ = ConnectState::delete(&self.state_path);
                self.cache(None);
                source
            }
            Err(cleanup) => {
                state.phase = LifecyclePhase::RecoveryRequired;
                let persist = state.save(&self.state_path).err();
                self.cache(Some(state));
                StrategyError::Other(format!(
                    "connect failed: {source}; rollback failed: {cleanup}{}",
                    persist
                        .map(|error| format!("; journal update failed: {error}"))
                        .unwrap_or_default()
                ))
            }
        }
    }
}

fn build_restore_args(state: &ConnectState) -> Vec<String> {
    let mut args = Vec::new();
    for output in &state.previous_layout {
        if output.name != state.virtual_port.as_str() && output.enabled {
            args.push(format!(
                "output.{}.position.{},{}",
                output.name, output.x, output.y
            ));
            args.push(format!("output.{}.enable", output.name));
        }
    }
    args.push(format!("output.{}.disable", state.virtual_port.as_str()));
    args
}

impl DisplayStrategy for KWinStrategy {
    fn connect(&self, params: &ConnectParams) -> Result<ConnectResult, StrategyError> {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Guard: refuse if already connected to give a clear error instead of NoSlot.
        {
            let guard = self
                .state
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if guard.is_some() {
                return Err(StrategyError::AlreadyConnected);
            }
        }

        // Step 1: Detect KWin environment.
        let kwin_env = resolve_compositor(params.requester_uid)?;

        // Step 2: Select DRM card.
        let card = if let Some(dev) = params.device.as_ref().or(self.default_device.as_ref()) {
            dev.clone()
        } else {
            let cards = sysfs::list_drm_cards()?;
            if cards.is_empty() {
                return Err(StrategyError::NoCard);
            }
            // Pick the card with the highest connected_count; first card wins on ties.
            let mut best = cards[0].clone();
            let mut best_count = sysfs::connected_count(&best);
            for c in cards.iter().skip(1) {
                let cnt = sysfs::connected_count(c);
                if cnt > best_count {
                    best_count = cnt;
                    best = c.clone();
                }
            }
            best
        };

        // Step 3: Find an empty connector slot.
        let slot = sysfs::find_empty_slot(&card)?;

        // Step 4: Generate EDID.
        let edid_bytes = edid::generate(params.width, params.height, params.refresh);

        // Snapshot and journal recovery intent before the first display mutation.
        let layout_snapshot = kscreen::parse_outputs(&kscreen::list_outputs(&kwin_env)?);
        let to_disable: Vec<String> = if params.exclusive {
            layout_snapshot
                .iter()
                .filter(|output| output.enabled && output.name != slot)
                .map(|output| output.name.clone())
                .collect()
        } else {
            self.disable_outputs.clone()
        };
        let mut cs = ConnectState {
            schema_version: state::CURRENT_SCHEMA_VERSION,
            phase: LifecyclePhase::Connecting,
            card: CardId::try_from(card.as_str())
                .map_err(|error| StrategyError::Other(error.into()))?,
            virtual_port: ConnectorId::try_from(slot.as_str())
                .map_err(|error| StrategyError::Other(error.into()))?,
            mode: Mode {
                width: params.width,
                height: params.height,
                refresh: params.refresh,
            },
            session_uid: kwin_env.uid,
            previous_layout: layout_snapshot,
        };
        cs.save(&self.state_path)?;
        self.cache(Some(cs.clone()));

        // Step 5: Write EDID override.
        if let Err(error) = sysfs::write_edid_override(&card, &slot, &edid_bytes) {
            return Err(self.failed_connect(cs, &kwin_env, error));
        }

        // Step 6: Clear stale KWin output config.
        if let Err(error) = sysfs::clear_kwin_output_config(&slot, kwin_env.uid) {
            return Err(self.failed_connect(cs, &kwin_env, error));
        }

        tracing::debug!(
            outputs = cs.previous_layout.len(),
            "layout snapshot taken before connect"
        );

        // Step 9: Disable selected physical connectors via kscreen-doctor.
        // Double-run: KWin needs two passes to fully settle the layout.
        for port in &to_disable {
            let arg = format!("output.{}.disable", port);
            if let Err(error) = kscreen::run_twice(&kwin_env, &[arg.as_str()], 400) {
                return Err(self.failed_connect(cs, &kwin_env, error));
            }
        }
        if !to_disable.is_empty() {
            // Extra settle time after batch disable.
            std::thread::sleep(Duration::from_millis(300));
        }

        // Step 10: Enable virtual slot via sysfs.
        if let Err(error) = sysfs::set_connector_status(&card, &slot, true) {
            return Err(self.failed_connect(cs, &kwin_env, error));
        }

        // Step 11: Wait for KWin to detect the virtual connector in its output list.
        let timeout = Duration::from_secs(self.output_ready_timeout_secs);
        let start = Instant::now();
        let mut appeared = false;
        while start.elapsed() < timeout {
            std::thread::sleep(Duration::from_millis(200));
            match kscreen::list_outputs(&kwin_env) {
                Ok(output) if output.to_lowercase().contains(&slot.to_lowercase()) => {
                    appeared = true;
                    break;
                }
                _ => {}
            }
        }
        if !appeared {
            return Err(self.failed_connect(cs, &kwin_env, StrategyError::Timeout));
        }

        // Step 12: Compute where to place the virtual display.
        // Place it to the right of the rightmost currently-enabled output to
        // avoid overlapping with physical monitors.
        let virtual_x: i32 = kscreen::list_outputs(&kwin_env)
            .ok()
            .map(|raw| {
                kscreen::parse_outputs(&raw)
                    .into_iter()
                    .filter(|o| o.enabled && o.name != slot)
                    .map(|o| o.x + o.width as i32)
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);

        // Step 13: Enable virtual display with explicit mode and position.
        // Double-run for KWin to apply the layout without overlapping outputs.
        let mode_arg = format!(
            "output.{}.mode.{}x{}@{}",
            slot, params.width, params.height, params.refresh
        );
        let pos_arg = format!("output.{}.position.{},{}", slot, virtual_x, 0);
        let enable_arg = format!("output.{}.enable", slot);
        if let Err(error) = kscreen::run_twice(
            &kwin_env,
            &[mode_arg.as_str(), pos_arg.as_str(), enable_arg.as_str()],
            500,
        ) {
            return Err(self.failed_connect(cs, &kwin_env, error));
        }

        let verified = kscreen::list_outputs(&kwin_env)
            .map(|raw| {
                kscreen::parse_outputs(&raw).into_iter().any(|output| {
                    output.name == slot
                        && output.enabled
                        && output.width == params.width
                        && output.height == params.height
                })
            })
            .unwrap_or(false);
        if !verified {
            return Err(self.failed_connect(cs, &kwin_env, StrategyError::Timeout));
        }

        // Step 15: Save state.
        cs.phase = LifecyclePhase::Connected;
        if let Err(error) = cs.save(&self.state_path) {
            return Err(self.failed_connect(cs, &kwin_env, error.into()));
        }

        // Step 16: Cache in self.state.
        self.cache(Some(cs));

        // Step 17: Return result.
        Ok(ConnectResult {
            card,
            connector: slot.clone(),
            mode: format!("{}x{}@{}", params.width, params.height, params.refresh),
        })
    }

    fn disconnect(&self) -> Result<(), StrategyError> {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = self
            .state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or(StrategyError::NotConnected)?;
        state.phase = LifecyclePhase::Disconnecting;
        state.save(&self.state_path)?;
        self.cache(Some(state.clone()));

        let kwin_env = resolve_compositor(Some(state.session_uid)).ok();
        if let Err(error) = self.cleanup_state(&state, kwin_env.as_ref()) {
            state.phase = LifecyclePhase::RecoveryRequired;
            state.save(&self.state_path)?;
            self.cache(Some(state));
            return Err(error);
        }

        ConnectState::delete(&self.state_path)?;
        self.cache(None);
        Ok(())
    }

    fn restore(&self) -> Result<(), StrategyError> {
        let _operation = self
            .operation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut state = match ConnectState::load(&self.state_path)? {
            Some(state) => state,
            None => return Ok(()),
        };
        let kwin_env = resolve_compositor(Some(state.session_uid)).ok();
        let active = kwin_env
            .as_ref()
            .and_then(|environment| kscreen::list_outputs(environment).ok())
            .map(|raw| {
                kscreen::parse_outputs(&raw)
                    .into_iter()
                    .any(|output| output.name == state.virtual_port.as_str() && output.enabled)
            })
            .unwrap_or(false);

        if state.phase == LifecyclePhase::Connected && active {
            self.cache(Some(state));
            return Ok(());
        }

        if let Err(error) = self.cleanup_state(&state, kwin_env.as_ref()) {
            state.phase = LifecyclePhase::RecoveryRequired;
            state.save(&self.state_path)?;
            self.cache(Some(state));
            return Err(error);
        }
        ConnectState::delete(&self.state_path)?;
        self.cache(None);
        Ok(())
    }

    fn status(&self) -> StrategyStatus {
        let guard = self
            .state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match guard.as_ref() {
            Some(cs) => StrategyStatus {
                phase: cs.phase,
                connected: cs.phase == LifecyclePhase::Connected,
                card: Some(cs.card.to_string()),
                connector: Some(cs.virtual_port.to_string()),
                mode: Some(format!(
                    "{}x{}@{}",
                    cs.mode.width, cs.mode.height, cs.mode.refresh
                )),
                strategy: Some("kwin".into()),
            },
            None => StrategyStatus {
                phase: LifecyclePhase::Disconnected,
                connected: false,
                card: None,
                connector: None,
                mode: None,
                strategy: Some("kwin".into()),
            },
        }
    }

    fn is_authorized(&self, uid: u32) -> bool {
        if uid == 0 {
            return true;
        }
        self.state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|state| state.session_uid == uid)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::strategy::kwin::env::KWinEnv;
    use crate::strategy::kwin::kscreen::OutputInfo;

    fn candidate(display: &str) -> KWinEnv {
        KWinEnv {
            uid: 1000,
            gid: 1000,
            wayland_display: display.into(),
            xdg_runtime_dir: "/run/user/1000".into(),
        }
    }

    #[test]
    fn resolver_returns_compositor_not_found_without_candidates() {
        let result = resolve_candidates(Vec::new(), |_| Ok(String::new()));
        assert!(matches!(result, Err(StrategyError::CompositorNotFound)));
    }

    #[test]
    fn resolver_selects_single_success() {
        let result = resolve_candidates(vec![candidate("wayland-2")], |_| Ok("outputs".into()))
            .expect("resolve candidate");
        assert_eq!(result.wayland_display, "wayland-2");
    }

    #[test]
    fn resolver_skips_stale_candidate_and_selects_valid_one() {
        let result = resolve_candidates(
            vec![candidate("wayland-0"), candidate("wayland-1")],
            |environment| {
                if environment.wayland_display == "wayland-1" {
                    Ok("outputs".into())
                } else {
                    Err(StrategyError::KscreenDoctor("stale socket".into()))
                }
            },
        )
        .expect("resolve valid candidate");
        assert_eq!(result.wayland_display, "wayland-1");
    }

    #[test]
    fn resolver_preserves_kscreen_error_when_all_probes_fail() {
        let result = resolve_candidates(
            vec![candidate("wayland-0"), candidate("wayland-1")],
            |environment| {
                Err(StrategyError::KscreenDoctor(format!(
                    "{} failed",
                    environment.wayland_display
                )))
            },
        );
        assert!(matches!(
            result,
            Err(StrategyError::KscreenDoctor(message)) if message == "wayland-1 failed"
        ));
    }

    #[test]
    fn resolver_rejects_multiple_successful_compositors() {
        let result =
            resolve_candidates(vec![candidate("wayland-0"), candidate("wayland-1")], |_| {
                Ok("outputs".into())
            });
        assert!(matches!(result, Err(StrategyError::AmbiguousCompositor)));
    }

    #[test]
    fn resolver_preserves_missing_binary_error() {
        let result = resolve_candidates(vec![candidate("wayland-0")], |_| {
            Err(StrategyError::KscreenDoctor(
                "kscreen-doctor not found in PATH".into(),
            ))
        });
        assert!(matches!(
            result,
            Err(StrategyError::KscreenDoctor(message))
                if message == "kscreen-doctor not found in PATH"
        ));
    }

    #[test]
    fn restore_arguments_enable_physical_outputs_before_disabling_virtual() {
        let state = ConnectState {
            schema_version: state::CURRENT_SCHEMA_VERSION,
            phase: LifecyclePhase::Disconnecting,
            card: CardId::try_from("card0").unwrap(),
            virtual_port: ConnectorId::try_from("DP-3").unwrap(),
            mode: Mode {
                width: 1920,
                height: 1080,
                refresh: 60,
            },
            session_uid: 1000,
            previous_layout: vec![OutputInfo {
                name: "DP-1".into(),
                enabled: true,
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080,
            }],
        };

        assert_eq!(
            build_restore_args(&state),
            vec![
                "output.DP-1.position.-1920,0",
                "output.DP-1.enable",
                "output.DP-3.disable",
            ]
        );
    }
}
