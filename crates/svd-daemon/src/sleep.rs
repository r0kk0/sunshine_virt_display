//! Sleep/wake D-Bus listener for logind integration.
//!
//! Acquires a logind sleep inhibitor delay lock so the system waits for the
//! virtual display to disconnect before sleeping.  Listens for
//! `PrepareForSleep` and `PrepareForShutdown` signals from
//! `org.freedesktop.login1.Manager`.
//!
//! Architecture notes (SOLID):
//!   - Single Responsibility: this module owns only the sleep/wake lifecycle.
//!     It has no knowledge of IPC framing, config, or the handler layer.
//!   - Dependency Inversion: depends on the `DisplayStrategy` trait, not on
//!     any concrete implementation.
//!   - The inhibitor fd is managed as `Option<OwnedFd>` — acquiring and
//!     releasing are separate operations with explicit ownership semantics.

use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedFd as ZOwnedFd;

use crate::strategy::DisplayStrategy;

// ──────────────────────────────────────────────────────────────────────────────
// Public API
// ──────────────────────────────────────────────────────────────────────────────

/// Spawn the sleep/wake D-Bus listener thread.
///
/// The thread:
/// 1. Connects to the system bus.
/// 2. Acquires a logind sleep inhibitor delay lock.
/// 3. Listens for `PrepareForSleep` and `PrepareForShutdown` signals.
/// 4. On sleep (`PrepareForSleep(true)`): disconnects the virtual display then
///    releases the inhibitor so the kernel can proceed with sleep.
/// 5. On wake (`PrepareForSleep(false)`): re-acquires the inhibitor.
/// 6. On shutdown (`PrepareForShutdown(true)`): disconnects and exits.
///
/// The thread is intentionally non-joinable — the daemon's main thread owns
/// the shutdown flag and will exit the process when it fires, which drops all
/// threads naturally.
pub fn spawn_sleep_handler(
    strategy: Arc<dyn DisplayStrategy>,
    shutdown: Arc<AtomicBool>,
) {
    std::thread::Builder::new()
        .name("sleep-handler".into())
        .spawn(move || run_sleep_loop(strategy, shutdown))
        .expect("failed to spawn sleep-handler thread");
}

// ──────────────────────────────────────────────────────────────────────────────
// Internal implementation
// ──────────────────────────────────────────────────────────────────────────────

const LOGIND_DEST: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";
const LOGIND_IFACE: &str = "org.freedesktop.login1.Manager";

/// Main body of the sleep-handler thread.
fn run_sleep_loop(strategy: Arc<dyn DisplayStrategy>, shutdown: Arc<AtomicBool>) {
    // ── Connect to the system bus ──────────────────────────────────────────
    let conn = match Connection::system() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "sleep-handler: D-Bus system connection failed — sleep protection disabled");
            return;
        }
    };

    // ── Acquire the initial inhibitor lock ────────────────────────────────
    let mut inhibitor: Option<OwnedFd> = acquire_inhibitor(&conn);
    if inhibitor.is_none() {
        tracing::warn!("sleep-handler: could not acquire sleep inhibitor — disconnect-before-sleep will not be guaranteed");
    }

    // ── Build the logind manager proxy ────────────────────────────────────
    let proxy = match Proxy::new(
        &conn,
        LOGIND_DEST,
        LOGIND_PATH,
        LOGIND_IFACE,
    ) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "sleep-handler: failed to build logind proxy");
            return;
        }
    };

    // ── Subscribe to PrepareForSleep signals ─────────────────────────────
    // `receive_signal` returns a `SignalIterator` that blocks per `next()`.
    // We interleave PrepareForShutdown by subscribing to both, then driving
    // them sequentially in the same loop.  Since both are infrequent, the
    // sequential approach is correct — we won't miss a shutdown while blocked
    // on sleep signals because the kernel serialises sleep/shutdown events.
    //
    // We use a single `receive_signal` on "PrepareForSleep" and rely on the
    // fact that PrepareForShutdown is also backed by the connection's message
    // queue.  For correctness we subscribe to both signals through their own
    // iterators and drive them on separate threads — but this adds complexity.
    //
    // Pragmatic approach: subscribe to both signals through separate iterators,
    // then poll each in a round-robin with a short park.  This is correct
    // because:
    //   - Sleep/wake events are rare (seconds apart at minimum).
    //   - We only need sub-second latency on the disconnect, which is fine.
    let sleep_iter = match proxy.receive_signal("PrepareForSleep") {
        Ok(it) => it,
        Err(e) => {
            tracing::error!(error = %e, "sleep-handler: cannot subscribe to PrepareForSleep");
            return;
        }
    };

    let shutdown_iter = match proxy.receive_signal("PrepareForShutdown") {
        Ok(it) => it,
        Err(e) => {
            tracing::error!(error = %e, "sleep-handler: cannot subscribe to PrepareForShutdown");
            return;
        }
    };

    tracing::info!("sleep-handler: listening for PrepareForSleep and PrepareForShutdown");

    // ── Message loop ──────────────────────────────────────────────────────
    // `SignalIterator::next()` blocks until a signal arrives.  To avoid
    // permanent block, we run each iterator in its own micro-thread and
    // communicate results back via an `mpsc` channel.
    use std::sync::mpsc;

    #[derive(Debug)]
    enum Event {
        Sleep(bool),
        Shutdown(bool),
    }

    let (tx_sleep, rx) = mpsc::channel::<Event>();
    let tx_shutdown = tx_sleep.clone();

    // Thread: PrepareForSleep listener
    // SignalIterator yields Option<Message> (infallible per signal).
    std::thread::Builder::new()
        .name("sleep-signal".into())
        .spawn(move || {
            for msg in sleep_iter {
                if let Ok((active,)) = msg.body().deserialize::<(bool,)>() {
                    if tx_sleep.send(Event::Sleep(active)).is_err() {
                        break;
                    }
                } else {
                    tracing::warn!("sleep-handler: failed to decode PrepareForSleep body");
                }
            }
        })
        .expect("spawn sleep-signal thread");

    // Thread: PrepareForShutdown listener
    std::thread::Builder::new()
        .name("shutdown-signal".into())
        .spawn(move || {
            for msg in shutdown_iter {
                if let Ok((active,)) = msg.body().deserialize::<(bool,)>() {
                    if tx_shutdown.send(Event::Shutdown(active)).is_err() {
                        break;
                    }
                } else {
                    tracing::warn!("sleep-handler: failed to decode PrepareForShutdown body");
                }
            }
        })
        .expect("spawn shutdown-signal thread");

    // ── Main dispatch loop ────────────────────────────────────────────────
    loop {
        // Check daemon shutdown flag on each iteration (with a short timeout
        // so the thread doesn't spin indefinitely after the daemon stops).
        match rx.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(Event::Sleep(true)) => {
                tracing::info!("sleep-handler: PrepareForSleep(true) — system going to sleep");
                if strategy.status().connected {
                    tracing::info!("sleep-handler: disconnecting virtual display before sleep");
                    if let Err(e) = strategy.disconnect() {
                        tracing::error!(error = %e, "sleep-handler: disconnect failed");
                    }
                }
                // Release the inhibitor AFTER disconnect so the kernel can proceed.
                // Dropping the OwnedFd closes it and releases the delay lock.
                drop(inhibitor.take());
                tracing::info!("sleep-handler: inhibitor released — sleep may proceed");
            }

            Ok(Event::Sleep(false)) => {
                tracing::info!("sleep-handler: PrepareForSleep(false) — system awake");
                // Re-acquire the inhibitor for the next sleep cycle.
                inhibitor = acquire_inhibitor(&conn);
                if inhibitor.is_none() {
                    tracing::warn!("sleep-handler: could not re-acquire inhibitor after wake");
                }
                tracing::info!(
                    "sleep-handler: awake — reconnect-on-wake not yet supported; \
                     run `svd connect` to re-enable the virtual display"
                );
            }

            Ok(Event::Shutdown(true)) => {
                tracing::info!("sleep-handler: PrepareForShutdown(true) — disconnecting before shutdown");
                if strategy.status().connected {
                    if let Err(e) = strategy.disconnect() {
                        tracing::error!(error = %e, "sleep-handler: disconnect before shutdown failed");
                    }
                }
                drop(inhibitor.take());
                // Nothing else to do — the system is shutting down.
                break;
            }

            Ok(Event::Shutdown(false)) => {
                tracing::debug!("sleep-handler: PrepareForShutdown(false) — ignored");
            }

            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Normal — check shutdown flag periodically.
                if shutdown.load(Ordering::Acquire) {
                    tracing::debug!("sleep-handler: shutdown flag set — exiting");
                    break;
                }
            }

            Err(mpsc::RecvTimeoutError::Disconnected) => {
                tracing::warn!("sleep-handler: signal sender disconnected — exiting");
                break;
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Inhibitor helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Acquire a logind sleep inhibitor delay lock.
///
/// Returns `Some(OwnedFd)` on success.  The lock is held as long as the fd
/// is open; dropping it releases the lock and allows sleep to proceed.
///
/// Uses `"delay"` mode so other inhibitors are respected and the system is
/// not permanently blocked.
fn acquire_inhibitor(conn: &Connection) -> Option<OwnedFd> {
    let proxy = match Proxy::new(conn, LOGIND_DEST, LOGIND_PATH, LOGIND_IFACE) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "sleep-handler: failed to build proxy for Inhibit");
            return None;
        }
    };

    let result: zbus::Result<ZOwnedFd> = proxy.call(
        "Inhibit",
        &(
            "sleep",
            "svd-daemon",
            "Disconnect virtual display before sleep",
            "delay",
        ),
    );

    match result {
        Ok(z_fd) => {
            tracing::debug!("sleep-handler: inhibitor acquired");
            Some(OwnedFd::from(z_fd))
        }
        Err(e) => {
            tracing::error!(error = %e, "sleep-handler: Inhibit call failed");
            None
        }
    }
}
