//! Real request handler — wires KWinStrategy into the IPC server.
//!
//! T4.8 — RealHandler implements RequestHandler by delegating to KWinStrategy.
//!
//! Architecture notes (SOLID):
//!   - Single Responsibility: this module only translates IPC requests into
//!     strategy calls and maps results to protocol responses.
//!   - Dependency Inversion: depends on DisplayStrategy abstraction, not a
//!     concrete implementation (KWinStrategy is injected at construction time
//!     via Arc).

use std::sync::Arc;

use crate::ipc::server::RequestHandler;
use crate::strategy::{ConnectParams, DisplayStrategy};
use crate::strategy::kwin::KWinStrategy;

// ──────────────────────────────────────────────────────────────────────────────
// RealHandler
// ──────────────────────────────────────────────────────────────────────────────

/// Bridges the IPC layer to the KWin display strategy.
///
/// Wraps a shared `KWinStrategy` instance and translates each
/// [`svd_proto::Request`] into the corresponding strategy call, mapping
/// results to [`svd_proto::Response`] variants.
pub struct RealHandler {
    strategy: Arc<KWinStrategy>,
}

impl RealHandler {
    /// Construct a new `RealHandler` backed by the given `KWinStrategy`.
    pub fn new(strategy: Arc<KWinStrategy>) -> Self {
        RealHandler { strategy }
    }
}

impl RequestHandler for RealHandler {
    fn handle(&self, req: svd_proto::Request) -> svd_proto::Response {
        use svd_proto::{Request, Response};

        match req {
            // ── Connect ────────────────────────────────────────────────────────
            Request::Connect { width, height, refresh, device, dry_run } => {
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

                let params = ConnectParams { width, height, refresh, device };
                match self.strategy.connect(&params) {
                    Ok(result) => Response::Connect {
                        ok: true,
                        connector: Some(result.connector),
                        card: Some(result.card),
                        mode: Some(result.mode),
                        error: None,
                        message: None,
                    },
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
                Ok(()) => Response::Disconnect { ok: true, error: None },
                Err(e) => Response::Disconnect { ok: false, error: Some(e.to_string()) },
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
                Ok(()) => Response::Restore { ok: true, error: None },
                Err(e) => Response::Restore { ok: false, error: Some(e.to_string()) },
            },
        }
    }
}
