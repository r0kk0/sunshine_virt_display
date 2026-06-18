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
    os::{
        fd::AsRawFd,
        unix::{
            fs::FileTypeExt,
            net::{UnixListener, UnixStream},
        },
    },
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
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
    #[error("refusing to replace non-socket path at {path}")]
    UnsafeSocketPath { path: std::path::PathBuf },
}

// ──────────────────────────────────────────────────────────────────────────────
// Handler trait
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCredentials {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestContext {
    pub peer: PeerCredentials,
}

fn peer_credentials(stream: &UnixStream) -> std::io::Result<PeerCredentials> {
    let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the output buffer is valid for `length`, and the stream owns a
    // live Unix-domain socket descriptor for the duration of the call.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<libc::ucred>() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unexpected SO_PEERCRED length",
        ));
    }
    // SAFETY: getsockopt succeeded and initialized exactly one `ucred` value.
    let credentials = unsafe { credentials.assume_init() };
    let pid = u32::try_from(credentials.pid)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "negative peer pid"))?;
    Ok(PeerCredentials {
        pid,
        uid: credentials.uid,
        gid: credentials.gid,
    })
}

/// Called for each incoming request.  Returns a [`svd_proto::Response`].
///
/// Implementations must be `Send + Sync` because the server may be moved to
/// a background thread while the main thread holds an `Arc` to the same handler.
pub trait RequestHandler: Send + Sync {
    fn handle(&self, context: RequestContext, req: svd_proto::Request) -> svd_proto::Response;
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
    fn handle(&self, _context: RequestContext, req: svd_proto::Request) -> svd_proto::Response {
        use svd_proto::{Request, Response};
        match req {
            Request::Status {} => Response::Status {
                ok: true,
                phase: svd_proto::LifecyclePhase::Disconnected,
                connected: false,
                card: None,
                connector: None,
                mode: None,
                strategy: None,
            },
            _ => Response::Disconnect {
                ok: true,
                error: None,
            },
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
    ipc_timeout: std::time::Duration,
) -> Result<(), ServerError> {
    match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) if metadata.file_type().is_socket() => std::fs::remove_file(socket_path)?,
        Ok(_) => {
            return Err(ServerError::UnsafeSocketPath {
                path: socket_path.to_path_buf(),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(ServerError::Io(error)),
    }

    let listener = UnixListener::bind(socket_path).map_err(|e| ServerError::Bind {
        path: socket_path.to_path_buf(),
        source: e,
    })?;

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
                if let Err(error) = stream.set_read_timeout(Some(ipc_timeout)) {
                    tracing::warn!(%error, "could not set IPC read timeout");
                    continue;
                }
                if let Err(error) = stream.set_write_timeout(Some(ipc_timeout)) {
                    tracing::warn!(%error, "could not set IPC write timeout");
                    continue;
                }
                let context = match peer_credentials(&stream) {
                    Ok(peer) => RequestContext { peer },
                    Err(error) => {
                        tracing::warn!(%error, "could not read IPC peer credentials");
                        continue;
                    }
                };

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

                tracing::debug!(?req, peer = ?context.peer, "received request");

                let resp = handler.handle(context, req);

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

    fn context() -> RequestContext {
        RequestContext {
            peer: PeerCredentials {
                pid: 1,
                uid: 1000,
                gid: 1000,
            },
        }
    }

    #[test]
    fn stub_handler_status_returns_ok() {
        let handler = StubHandler;
        let resp = handler.handle(context(), svd_proto::Request::Status {});
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
        let resp = handler.handle(context(), svd_proto::Request::Disconnect {});
        assert!(matches!(
            resp,
            svd_proto::Response::Disconnect { ok: true, .. }
        ));
    }
}
