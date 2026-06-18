pub mod edid;
pub mod env;
pub mod kscreen;
pub mod state;
pub mod sysfs;

use std::path::PathBuf;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use crate::strategy::kwin::state::ConnectState;
use crate::strategy::{
    ConnectParams, ConnectResult, DisplayStrategy, StrategyError, StrategyStatus,
};

// ---------------------------------------------------------------------------
// Local helper: read the real uid of a process from /proc/$pid/status
// ---------------------------------------------------------------------------

fn read_uid(pid: u32) -> Result<u32, StrategyError> {
    let status_path = format!("/proc/{}/status", pid);
    let contents = std::fs::read_to_string(&status_path).map_err(StrategyError::Io)?;

    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            // Format: "Uid:\tREAL EFFECTIVE SAVED FILESYSTEM"
            let first = rest
                .split_whitespace()
                .next()
                .ok_or(StrategyError::CompositorNotFound)?;
            return first
                .parse::<u32>()
                .map_err(|_| StrategyError::CompositorNotFound);
        }
    }
    Err(StrategyError::CompositorNotFound)
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
            state: RwLock::new(None),
        }
    }
}

impl DisplayStrategy for KWinStrategy {
    fn connect(&self, params: &ConnectParams) -> Result<ConnectResult, StrategyError> {
        // Guard: refuse if already connected to give a clear error instead of NoSlot.
        {
            let guard = self.state.read().unwrap();
            if guard.is_some() {
                return Err(StrategyError::AlreadyConnected);
            }
        }

        // Step 1: Detect KWin environment.
        let kwin_env = env::KWinEnv::detect()?;

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

        // Step 5: Write EDID override.
        sysfs::write_edid_override(&card, &slot, &edid_bytes)?;

        // Step 6: Clear stale KWin output config.
        let uid = read_uid(kwin_env.pid)?;
        sysfs::clear_kwin_output_config(&slot, uid)?;

        // Step 7: Snapshot current layout before any changes.
        // This is saved in state and used on disconnect to restore exact positions.
        let layout_snapshot: Vec<kscreen::OutputInfo> = kscreen::list_outputs(&kwin_env)
            .map(|raw| kscreen::parse_outputs(&raw))
            .unwrap_or_default();

        tracing::debug!(
            outputs = layout_snapshot.len(),
            "layout snapshot taken before connect"
        );

        // Step 8: Determine which connectors to disable.
        //
        // --exclusive: disable all currently-enabled physical outputs (auto-detect).
        // disable_outputs config (non-empty): disable only those specific connectors.
        // Neither: don't disable anything — just add the virtual display alongside.
        let to_disable: Vec<String> = if params.exclusive {
            layout_snapshot
                .iter()
                .filter(|o| o.enabled && o.name != slot)
                .map(|o| o.name.clone())
                .collect()
        } else {
            self.disable_outputs.clone()
        };

        // Step 9: Disable selected physical connectors via kscreen-doctor.
        // Double-run: KWin needs two passes to fully settle the layout.
        for port in &to_disable {
            let arg = format!("output.{}.disable", port);
            if let Err(e) = kscreen::run_twice(&kwin_env, &[arg.as_str()], 400) {
                tracing::warn!(port, error = %e, "failed to disable output — continuing");
            }
        }
        if !to_disable.is_empty() {
            // Extra settle time after batch disable.
            std::thread::sleep(Duration::from_millis(300));
        }

        // Step 10: Enable virtual slot via sysfs.
        sysfs::set_connector_status(&card, &slot, true)?;

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
            tracing::warn!(
                slot = %slot,
                "virtual display not detected by KWin within timeout; attempting kscreen-doctor anyway"
            );
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
        if let Err(e) = kscreen::run_twice(
            &kwin_env,
            &[mode_arg.as_str(), pos_arg.as_str(), enable_arg.as_str()],
            500,
        ) {
            tracing::warn!(error = %e, "kscreen-doctor failed to enable virtual display");
        }

        // Step 14: Build ConnectState with full layout snapshot.
        let card_index = card.trim_start_matches("card");
        let edid_override_path = format!(
            "/sys/kernel/debug/dri/{}/{}/edid_override",
            card_index, slot
        );
        let cs = ConnectState {
            card: card.clone(),
            virtual_port: slot.clone(),
            previous_layout: layout_snapshot,
            previous_ports: vec![],
            edid_override_path,
        };

        // Step 15: Save state.
        cs.save(&self.state_path)?;

        // Step 16: Cache in self.state.
        {
            let mut guard = self.state.write().unwrap();
            *guard = Some(cs);
        }

        // Step 17: Return result.
        Ok(ConnectResult {
            card,
            connector: slot.clone(),
            mode: format!("{}x{}@{}", params.width, params.height, params.refresh),
        })
    }

    fn disconnect(&self) -> Result<(), StrategyError> {
        // Step 1: Read state — if None, not connected.
        let cs = {
            let guard = self.state.read().unwrap();
            match guard.as_ref() {
                Some(s) => s.clone(),
                None => return Err(StrategyError::NotConnected),
            }
        };

        // Step 2: Detect KWin env fresh (best-effort — KWin may have restarted).
        // Failure here must NOT abort cleanup: sysfs and state-file steps do not
        // need the kwin env and must always run.
        let kwin_env = match env::KWinEnv::detect() {
            Ok(e) => Some(e),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "KWin not found during disconnect; skipping kscreen calls, continuing cleanup"
                );
                None
            }
        };

        // Step 3: Restore previous layout atomically in one kscreen-doctor call.
        // previous_layout holds the full pre-connect snapshot (positions + enabled
        // state). Build one atomic command that re-enables what was enabled and
        // repositions everything, then disables the virtual port.
        if let Some(ref env) = kwin_env {
            let mut restore_args: Vec<String> = Vec::new();

            if !cs.previous_layout.is_empty() {
                for output in &cs.previous_layout {
                    if output.name == cs.virtual_port {
                        continue; // virtual port is cleaned up via sysfs below
                    }
                    if output.enabled {
                        restore_args.push(format!(
                            "output.{}.position.{},{}",
                            output.name, output.x, output.y
                        ));
                        restore_args.push(format!("output.{}.enable", output.name));
                    }
                }
            } else {
                // Legacy fallback: no layout snapshot, just re-enable previous_ports.
                for port in &cs.previous_ports {
                    restore_args.push(format!("output.{}.enable", port));
                }
            }

            // Always disable the virtual port as part of the atomic restore.
            restore_args.push(format!("output.{}.disable", cs.virtual_port));

            if !restore_args.is_empty() {
                let args_refs: Vec<&str> = restore_args.iter().map(|s| s.as_str()).collect();
                if let Err(e) = kscreen::run_twice(env, &args_refs, 500) {
                    tracing::warn!(
                        error = %e,
                        "kscreen layout restore failed; continuing cleanup"
                    );
                }
            }
        }

        // Step 4: Set connector status to off via sysfs (best-effort).
        if let Err(e) = sysfs::set_connector_status(&cs.card, &cs.virtual_port, false) {
            tracing::warn!(
                error = %e,
                "failed to set sysfs connector status to off; continuing"
            );
        }

        // Step 5: Clear EDID override (best-effort).
        if let Err(e) = sysfs::clear_edid_override(&cs.card, &cs.virtual_port) {
            tracing::warn!(
                error = %e,
                "failed to clear EDID override; continuing"
            );
        }

        // Step 6: Delete state file (best-effort — cache clear must still run).
        if let Err(e) = ConnectState::delete(&self.state_path) {
            tracing::warn!(
                error = %e,
                "failed to delete state file during disconnect; continuing"
            );
        }

        // Step 7: Clear self.state cache.
        {
            let mut guard = self.state.write().unwrap();
            *guard = None;
        }

        Ok(())
    }

    fn restore(&self) -> Result<(), StrategyError> {
        // Load state file; nothing to do on a clean start.
        let cs = match ConnectState::load(&self.state_path)? {
            Some(cs) => cs,
            None => return Ok(()),
        };

        // Verify the virtual display is still active in KWin.
        // If the daemon crashed while connected, KWin may have already removed
        // the output — the state file is stale and must be cleaned up so
        // find_empty_slot() can offer the slot again on the next connect().
        let is_active = match env::KWinEnv::detect() {
            Ok(kwin_env) => kscreen::list_outputs(&kwin_env)
                .ok()
                .map(|raw| {
                    kscreen::parse_outputs(&raw)
                        .into_iter()
                        .any(|o| o.name == cs.virtual_port && o.enabled)
                })
                .unwrap_or(false),
            Err(_) => false,
        };

        if is_active {
            tracing::info!(
                card = %cs.card,
                connector = %cs.virtual_port,
                "virtual display still active — restoring daemon state"
            );
            let mut guard = self.state.write().unwrap();
            *guard = Some(cs);
        } else {
            // Stale state: virtual display no longer enabled in KWin.
            // Clean up sysfs/EDID so the slot is free for the next connect().
            tracing::info!(
                card = %cs.card,
                connector = %cs.virtual_port,
                "stale virtual display state detected — cleaning up on startup"
            );

            // Tell KWin to remove the output (best-effort, KWin may be gone).
            if let Ok(kwin_env) = env::KWinEnv::detect() {
                let disable_arg = format!("output.{}.disable", cs.virtual_port);
                let _ = kscreen::run(&kwin_env, &[disable_arg.as_str()]);
            }

            // Reset sysfs status so find_empty_slot() sees the slot as free.
            if let Err(e) = sysfs::set_connector_status(&cs.card, &cs.virtual_port, false) {
                tracing::warn!(error = %e, "stale cleanup: failed to set sysfs status to off");
            }

            // Remove the EDID override so the kernel stops reporting the connector.
            if let Err(e) = sysfs::clear_edid_override(&cs.card, &cs.virtual_port) {
                tracing::warn!(error = %e, "stale cleanup: failed to clear EDID override");
            }

            // Delete the state file — next connect() starts fresh.
            if let Err(e) = ConnectState::delete(&self.state_path) {
                tracing::warn!(error = %e, "stale cleanup: failed to delete state file");
            }

            // State cache remains None.
        }

        Ok(())
    }

    fn status(&self) -> StrategyStatus {
        let guard = self.state.read().unwrap();
        match guard.as_ref() {
            Some(cs) => StrategyStatus {
                connected: true,
                card: Some(cs.card.clone()),
                connector: Some(cs.virtual_port.clone()),
                mode: None,
                strategy: Some("kwin".into()),
            },
            None => StrategyStatus {
                connected: false,
                card: None,
                connector: None,
                mode: None,
                strategy: Some("kwin".into()),
            },
        }
    }
}
