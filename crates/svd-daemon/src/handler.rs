//! Real request handler — wires KWinStrategy into the IPC server.
//!
//! T4.8 — RealHandler implements RequestHandler by delegating to KWinStrategy.
//! T5   — On a successful Connect, spawns the sunshine crash-watcher thread.
//!
//! Architecture notes (SOLID):
//!   - Single Responsibility: this module only translates IPC requests into
//!     strategy calls and maps results to protocol responses.
//!   - Dependency Inversion: depends on DisplayStrategy abstraction, not a
//!     concrete implementation (KWinStrategy is injected at construction time
//!     via Arc).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::ipc::server::RequestHandler;
use crate::strategy::kwin::KWinStrategy;
use crate::strategy::{ConnectParams, DisplayStrategy};

// ──────────────────────────────────────────────────────────────────────────────
// RealHandler
// ──────────────────────────────────────────────────────────────────────────────

/// Bridges the IPC layer to the KWin display strategy.
///
/// Wraps a shared `KWinStrategy` instance and translates each
/// [`svd_proto::Request`] into the corresponding strategy call, mapping
/// results to [`svd_proto::Response`] variants.
///
/// Validates requests server-side against the configured mode allowlist before
/// dispatching to the strategy (defense-in-depth: the CLI validates too, but
/// the daemon socket is world-readable so any process can write to it).
///
/// On a successful `Connect` request the handler spawns the
/// `sunshine-watcher` background thread (T5) so the virtual display is
/// automatically torn down if Sunshine crashes.
pub struct RealHandler {
    strategy: Arc<KWinStrategy>,
    extra_allowed_modes: Vec<svd_proto::Mode>,
    /// Propagated to the crash-watcher so it stops cleanly when the daemon
    /// receives SIGTERM / SIGINT.
    shutdown: Arc<AtomicBool>,
}

impl RealHandler {
    /// Construct a new `RealHandler` backed by the given `KWinStrategy`.
    pub fn new(
        strategy: Arc<KWinStrategy>,
        extra_allowed_modes: Vec<svd_proto::Mode>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        RealHandler {
            strategy,
            extra_allowed_modes,
            shutdown,
        }
    }
}

impl RequestHandler for RealHandler {
    fn handle(&self, req: svd_proto::Request) -> svd_proto::Response {
        use svd_proto::{Request, Response};

        // Server-side validation — rejects out-of-range or non-allowlisted modes
        // before they reach the strategy layer.
        if let Err(e) = svd_proto::validate_request(&req, &self.extra_allowed_modes) {
            return match &req {
                Request::Connect { .. } => Response::Connect {
                    ok: false,
                    connector: None,
                    card: None,
                    mode: None,
                    error: Some(e.to_string()),
                    message: None,
                },
                Request::Disconnect {} => Response::Disconnect {
                    ok: false,
                    error: Some(e.to_string()),
                },
                Request::Status {} => Response::Status {
                    ok: false,
                    connected: false,
                    card: None,
                    connector: None,
                    mode: None,
                    strategy: None,
                },
                Request::Restore {} => Response::Restore {
                    ok: false,
                    error: Some(e.to_string()),
                },
            };
        }

        match req {
            // ── Connect ────────────────────────────────────────────────────────
            Request::Connect {
                width,
                height,
                refresh,
                device,
                dry_run,
                exclusive,
            } => {
                if dry_run {
                    return Response::Connect {
                        ok: true,
                        connector: None,
                        card: None,
                        mode: None,
                        error: None,
                        message: None,
                    };
                }

                let params = ConnectParams {
                    width,
                    height,
                    refresh,
                    device,
                    exclusive,
                };
                match self.strategy.connect(&params) {
                    Ok(result) => {
                        // T5: Spawn crash watcher after a successful connect so that
                        // the virtual display is automatically disconnected if Sunshine
                        // exits unexpectedly.  Cast to the trait object so `watcher`
                        // does not depend on KWinStrategy directly (DIP).
                        crate::watcher::spawn_watcher(
                            Arc::clone(&self.strategy) as Arc<dyn crate::strategy::DisplayStrategy>,
                            Arc::clone(&self.shutdown),
                        );

                        Response::Connect {
                            ok: true,
                            connector: Some(result.connector),
                            card: Some(result.card),
                            mode: Some(result.mode),
                            error: None,
                            message: None,
                        }
                    }
                    Err(e) => Response::Connect {
                        ok: false,
                        connector: None,
                        card: None,
                        mode: None,
                        error: Some(e.to_string()),
                        message: None,
                    },
                }
            }

            // ── Disconnect ─────────────────────────────────────────────────────
            Request::Disconnect {} => match self.strategy.disconnect() {
                Ok(()) => Response::Disconnect {
                    ok: true,
                    error: None,
                },
                Err(e) => Response::Disconnect {
                    ok: false,
                    error: Some(e.to_string()),
                },
            },

            // ── Status ─────────────────────────────────────────────────────────
            Request::Status {} => {
                let s = self.strategy.status();
                Response::Status {
                    ok: true,
                    connected: s.connected,
                    card: s.card,
                    connector: s.connector,
                    mode: s.mode,
                    strategy: s.strategy,
                }
            }

            // ── Restore ────────────────────────────────────────────────────────
            Request::Restore {} => match self.strategy.restore() {
                Ok(()) => Response::Restore {
                    ok: true,
                    error: None,
                },
                Err(e) => Response::Restore {
                    ok: false,
                    error: Some(e.to_string()),
                },
            },
        }
    }
}
