pub mod edid;
pub mod env;
pub mod kscreen;
pub mod state;
pub mod sysfs;

use std::path::PathBuf;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use crate::strategy::{
    ConnectParams, ConnectResult, DisplayStrategy, StrategyError, StrategyStatus,
};
use crate::strategy::kwin::state::ConnectState;

// ---------------------------------------------------------------------------
// Local helper: read the real uid of a process from /proc/$pid/status
// ---------------------------------------------------------------------------

fn read_uid(pid: u32) -> Result<u32, StrategyError> {
    let status_path = format!("/proc/{}/status", pid);
    let contents = std::fs::read_to_string(&status_path).map_err(StrategyError::Io)?;

    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            // Format: "Uid:\tREAL EFFECTIVE SAVED FILESYSTEM"
            let first = rest.split_whitespace().next().ok_or_else(|| {
                StrategyError::CompositorNotFound
            })?;
            return first.parse::<u32>().map_err(|_| StrategyError::CompositorNotFound);
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
    state: RwLock<Option<ConnectState>>,
}

impl KWinStrategy {
    pub fn new(state_path: PathBuf, output_ready_timeout_secs: u64) -> Self {
        KWinStrategy {
            state_path,
            output_ready_timeout_secs,
            state: RwLock::new(None),
        }
    }
}

impl DisplayStrategy for KWinStrategy {
    fn connect(&self, params: &ConnectParams) -> Result<ConnectResult, StrategyError> {
        // Step 1: Detect KWin environment.
        let kwin_env = env::KWinEnv::detect()?;

        // Step 2: Select DRM card.
        let card = if let Some(dev) = &params.device {
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

        // Step 7: Record currently connected connectors.
        let previous = sysfs::connected_connectors(&card)?;

        // Step 8: Disable physical connectors.
        for port in &previous {
            let arg = format!("output.{}.disable", port);
            kscreen::run(&kwin_env, &[arg.as_str()])?;
        }

        // Step 9: Enable the virtual slot via sysfs.
        sysfs::set_connector_status(&card, &slot, true)?;

        // Step 10: Wait for KWin to assign the connector.
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
            // Timeout fallback: force the mode via kscreen-doctor.
            let mode_arg = format!(
                "output.{}.mode.{}x{}@{}",
                slot, params.width, params.height, params.refresh
            );
            kscreen::run(&kwin_env, &[mode_arg.as_str()])?;
            let enable_arg = format!("output.{}.enable", slot);
            kscreen::run(&kwin_env, &[enable_arg.as_str()])?;
        }

        // Step 11: Build ConnectState.
        let card_index = card.trim_start_matches("card");
        let edid_override_path = format!(
            "/sys/kernel/debug/dri/{}/{}/edid_override",
            card_index, slot
        );
        let cs = ConnectState {
            card: card.clone(),
            virtual_port: slot.clone(),
            previous_ports: previous,
            edid_override_path,
        };

        // Step 12: Save state.
        cs.save(&self.state_path)?;

        // Step 13: Cache in self.state.
        {
            let mut guard = self.state.write().unwrap();
            *guard = Some(cs);
        }

        // Step 14: Return result.
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

        // Step 3: Detect KWin env fresh.
        let kwin_env = env::KWinEnv::detect()?;

        // Step 3 continued: Re-enable physical connectors (best-effort).
        for port in &cs.previous_ports {
            let arg = format!("output.{}.enable", port);
            if let Err(e) = kscreen::run(&kwin_env, &[arg.as_str()]) {
                tracing::warn!(
                    error = %e,
                    port = %port,
                    "failed to re-enable physical connector during disconnect; continuing"
                );
            }
        }

        // Step 4a: Disable virtual slot (best-effort).
        let disable_arg = format!("output.{}.disable", cs.virtual_port);
        if let Err(e) = kscreen::run(&kwin_env, &[disable_arg.as_str()]) {
            tracing::warn!(
                error = %e,
                port = %cs.virtual_port,
                "failed to disable virtual connector during disconnect; continuing"
            );
        }

        // Step 4b: Set connector status to off via sysfs.
        if let Err(e) = sysfs::set_connector_status(&cs.card, &cs.virtual_port, false) {
            tracing::warn!(
                error = %e,
                "failed to set sysfs connector status to off; continuing"
            );
        }

        // Step 5: Clear EDID override.
        if let Err(e) = sysfs::clear_edid_override(&cs.card, &cs.virtual_port) {
            tracing::warn!(
                error = %e,
                "failed to clear EDID override; continuing"
            );
        }

        // Step 6: Delete state file.
        ConnectState::delete(&self.state_path)?;

        // Step 7: Clear self.state cache.
        {
            let mut guard = self.state.write().unwrap();
            *guard = None;
        }

        Ok(())
    }

    fn restore(&self) -> Result<(), StrategyError> {
        // Step 1: Try loading ConnectState.
        match ConnectState::load(&self.state_path)? {
            Some(cs) => {
                tracing::info!(
                    card = %cs.card,
                    connector = %cs.virtual_port,
                    "restored existing virtual display state"
                );
                // Step 2: Cache in self.state.
                let mut guard = self.state.write().unwrap();
                *guard = Some(cs);
            }
            None => {
                // Step 3: Nothing to restore.
            }
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
