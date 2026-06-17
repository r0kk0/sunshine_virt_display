//! Unix-domain socket server for svd-daemon IPC.
//!
//! T3.2 — UnixListener server skeleton + handler dispatch.
//!
//! Protocol: one request per connection, newline-delimited JSON framing
//! (see `ipc::framing`).  One connection is handled at a time; this is
//! intentional — svd is a low-volume control channel.
//!
//! Security notes:
//!   - Stale socket files are removed before binding (crash-safe restart).
//!   - Socket permissions are set to 0660 after binding.
//!   - Malformed / oversized frames are logged and the connection is
//!     closed rather than panicking.

use std::{
    os::unix::net::UnixListener,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use std::os::unix::fs::PermissionsExt as _;

use crate::ipc::{read_frame, write_frame, FrameError};

// ──────────────────────────────────────────────────────────────────────────────
// Error type
// ──────────────────────────────────────────────────────────────────────────────

/// Errors emitted by [`run_server`].
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("failed to bind socket at {path}: {source}")]
    Bind {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("framing error: {0}")]
    Framing(#[from] FrameError),
}

// ──────────────────────────────────────────────────────────────────────────────
// Handler trait
// ──────────────────────────────────────────────────────────────────────────────

/// Called for each incoming request.  Returns a [`svd_proto::Response`].
///
/// Implementations must be `Send + Sync` because the server may be moved to
/// a background thread while the main thread holds an `Arc` to the same handler.
pub trait RequestHandler: Send + Sync {
    fn handle(&self, req: svd_proto::Request) -> svd_proto::Response;
}

// ──────────────────────────────────────────────────────────────────────────────
// Stub handler
// ──────────────────────────────────────────────────────────────────────────────

/// Canned handler used until the real device logic is wired in (T4+).
///
/// - `Status {}` → always reports "daemon alive, not connected".
/// - Any other request → `Disconnect { ok: true }` (safe, no-op).
pub struct StubHandler;

impl RequestHandler for StubHandler {
    fn handle(&self, req: svd_proto::Request) -> svd_proto::Response {
        use svd_proto::{Request, Response};
        match req {
            Request::Status {} => Response::Status {
                ok: true,
                connected: false,
                card: None,
                connector: None,
                mode: None,
                strategy: None,
            },
            _ => Response::Disconnect { ok: true, error: None },
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Server
// ──────────────────────────────────────────────────────────────────────────────

/// Start the IPC server on `socket_path`.
///
/// Listens for connections, reads one request per connection, calls
/// `handler.handle()`, writes the response.  Runs until `shutdown` is set to
/// `true`.
///
/// Each connection is handled synchronously in the accept loop (one at a time —
/// acceptable for the low-volume svd control channel).
///
/// The function returns `Ok(())` when `shutdown` is signalled.
pub fn run_server(
    socket_path: &Path,
    handler: Arc<dyn RequestHandler>,
    shutdown: Arc<AtomicBool>,
) -> Result<(), ServerError> {
    // Remove a stale socket file from a previous run (or crash).
    // Ignoring NotFound is intentional — on first start there is nothing to remove.
    let _ = std::fs::remove_file(socket_path);

    let listener = UnixListener::bind(socket_path).map_err(|e| ServerError::Bind {
        path: socket_path.to_path_buf(),
        source: e,
    })?;

    // Set socket permissions to 0660 (owner + group read/write; no world access).
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o660))?;

    // Non-blocking accept loop so we can poll the shutdown flag.
    listener.set_nonblocking(true)?;

    tracing::info!(socket = %socket_path.display(), "svd-daemon IPC server listening");

    loop {
        if shutdown.load(Ordering::Relaxed) {
            tracing::info!("shutdown signalled; stopping server");
            break;
        }

        match listener.accept() {
            Ok((mut stream, _peer)) => {
                // Ensure the stream is in blocking mode for read_frame / write_frame.
                // Inheriting non-blocking from the listener is OS-defined; be explicit.
                if let Err(e) = stream.set_nonblocking(false) {
                    tracing::warn!(error = %e, "could not set stream to blocking; skipping connection");
                    continue;
                }

                // Read one request frame.
                let frame = match read_frame(&mut stream) {
                    Ok(f) => f,
                    Err(FrameError::ConnectionClosed) => {
                        tracing::debug!("client disconnected before sending a frame");
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "read_frame failed; closing connection");
                        continue;
                    }
                };

                // Deserialize the request.
                let req: svd_proto::Request = match serde_json::from_str(&frame) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(error = %e, "invalid request JSON; closing connection");
                        continue;
                    }
                };

                tracing::debug!(?req, "received request");

                let resp = handler.handle(req);

                tracing::debug!(?resp, "sending response");

                if let Err(e) = write_frame(&mut stream, &resp) {
                    tracing::warn!(error = %e, "write_frame failed");
                }
            }

            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No pending connection — back off briefly before retrying.
                std::thread::sleep(std::time::Duration::from_millis(50));
            }

            Err(e) => {
                tracing::error!(error = %e, "accept() failed");
                return Err(ServerError::Io(e));
            }
        }
    }

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_handler_status_returns_ok() {
        let handler = StubHandler;
        let resp = handler.handle(svd_proto::Request::Status {});
        match resp {
            svd_proto::Response::Status { ok, connected, .. } => {
                assert!(ok);
                assert!(!connected);
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn stub_handler_unknown_returns_disconnect_ok() {
        let handler = StubHandler;
        let resp = handler.handle(svd_proto::Request::Disconnect {});
        assert!(matches!(
            resp,
            svd_proto::Response::Disconnect { ok: true, .. }
        ));
    }
}
