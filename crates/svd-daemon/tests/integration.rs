//! Integration test: socket round-trip for Status request.
//!
//! Starts an in-process svd-daemon IPC server on a temp socket, connects to it
//! directly with a UnixStream, sends a `Request::Status {}` via write_frame,
//! reads the response via read_frame, and verifies it is
//! `Response::Status { ok: true, connected: false, .. }`.

use std::{
    fs::File,
    io::Write,
    os::unix::fs::MetadataExt,
    os::unix::fs::PermissionsExt,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use svd_daemon::ipc::{
    read_frame, run_server, write_frame, RequestContext, RequestHandler, ServerError, StubHandler,
};

const IPC_TIMEOUT: Duration = Duration::from_millis(150);

/// Build a unique temp socket path for this test.
fn temp_socket_path(test_name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "svd_itest_{}_{}.sock",
        test_name,
        std::process::id()
    ));
    p
}

/// Wait up to `timeout` for the socket file to appear.
fn wait_for_socket(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn status_round_trip() {
    let socket_path = temp_socket_path("status_round_trip");

    // Ensure no leftover socket from a previous failed run.
    let _ = std::fs::remove_file(&socket_path);

    let handler: Arc<dyn svd_daemon::ipc::RequestHandler> = Arc::new(StubHandler);
    let shutdown = Arc::new(AtomicBool::new(false));

    // Spawn the server on a background thread.
    let server_socket = socket_path.clone();
    let server_shutdown = Arc::clone(&shutdown);
    let server_thread = thread::spawn(move || {
        run_server(&server_socket, handler, server_shutdown, IPC_TIMEOUT)
            .expect("run_server failed in test thread");
    });

    // Wait up to 2 seconds for the socket to appear.
    assert!(
        wait_for_socket(&socket_path, Duration::from_secs(2)),
        "server socket did not appear within 2 seconds at {}",
        socket_path.display()
    );

    // Connect and send a Status request.
    let mut stream = UnixStream::connect(&socket_path).expect("failed to connect to server socket");

    write_frame(&mut stream, &svd_proto::Request::Status {}).expect("write_frame(Status) failed");

    // Read the response.
    let resp_frame = read_frame(&mut stream).expect("read_frame failed");

    let resp: svd_proto::Response =
        serde_json::from_str(&resp_frame).expect("response is not valid JSON");

    match resp {
        svd_proto::Response::Status { ok, connected, .. } => {
            assert!(ok, "StubHandler should return ok=true for Status");
            assert!(
                !connected,
                "StubHandler should return connected=false for Status"
            );
        }
        other => panic!("unexpected response variant: {other:?}"),
    }

    // Signal shutdown and wait for the server thread to exit.
    shutdown.store(true, Ordering::Relaxed);
    server_thread.join().expect("server thread panicked");

    // Clean up socket file.
    let _ = std::fs::remove_file(&socket_path);
}

#[test]
fn socket_is_group_only() {
    let socket_path = temp_socket_path("socket_mode");
    let _ = std::fs::remove_file(&socket_path);
    let shutdown = Arc::new(AtomicBool::new(false));
    let server_shutdown = Arc::clone(&shutdown);
    let server_socket = socket_path.clone();
    let server_thread = thread::spawn(move || {
        run_server(
            &server_socket,
            Arc::new(StubHandler),
            server_shutdown,
            IPC_TIMEOUT,
        )
        .expect("server");
    });

    assert!(wait_for_socket(&socket_path, Duration::from_secs(2)));
    let mode = std::fs::metadata(&socket_path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o660);

    shutdown.store(true, Ordering::Relaxed);
    server_thread.join().expect("join");
    let _ = std::fs::remove_file(&socket_path);
}

#[test]
fn regular_file_at_socket_path_is_not_removed() {
    let socket_path = temp_socket_path("regular_file");
    let _ = std::fs::remove_file(&socket_path);
    File::create(&socket_path)
        .expect("create sentinel")
        .write_all(b"do not delete")
        .expect("write sentinel");

    let result = run_server(
        &socket_path,
        Arc::new(StubHandler),
        Arc::new(AtomicBool::new(false)),
        IPC_TIMEOUT,
    );
    assert!(matches!(result, Err(ServerError::UnsafeSocketPath { .. })));
    assert_eq!(
        std::fs::read(&socket_path).expect("sentinel remains"),
        b"do not delete"
    );
    let _ = std::fs::remove_file(&socket_path);
}

#[test]
fn stalled_client_is_timed_out() {
    let socket_path = temp_socket_path("stalled_client");
    let _ = std::fs::remove_file(&socket_path);
    let shutdown = Arc::new(AtomicBool::new(false));
    let server_shutdown = Arc::clone(&shutdown);
    let server_socket = socket_path.clone();
    let server_thread = thread::spawn(move || {
        run_server(
            &server_socket,
            Arc::new(StubHandler),
            server_shutdown,
            IPC_TIMEOUT,
        )
        .expect("server");
    });
    assert!(wait_for_socket(&socket_path, Duration::from_secs(2)));

    let stalled = UnixStream::connect(&socket_path).expect("stalled connection");
    let started = Instant::now();
    thread::sleep(IPC_TIMEOUT + Duration::from_millis(50));
    let mut second = UnixStream::connect(&socket_path).expect("second connection");
    write_frame(&mut second, &svd_proto::Request::Status {}).expect("write status");
    let response = read_frame(&mut second).expect("server recovered after timeout");
    assert!(response.contains("\"cmd\":\"status\""));
    assert!(started.elapsed() < Duration::from_secs(1));
    drop(stalled);

    shutdown.store(true, Ordering::Relaxed);
    server_thread.join().expect("join");
    let _ = std::fs::remove_file(&socket_path);
}

struct CredentialHandler(Arc<Mutex<Option<RequestContext>>>);

impl RequestHandler for CredentialHandler {
    fn handle(&self, context: RequestContext, _req: svd_proto::Request) -> svd_proto::Response {
        *self.0.lock().expect("credential lock") = Some(context);
        svd_proto::Response::Status {
            ok: true,
            phase: svd_proto::LifecyclePhase::Disconnected,
            connected: false,
            card: None,
            connector: None,
            mode: None,
            strategy: None,
        }
    }
}

#[test]
fn handler_receives_kernel_peer_credentials() {
    let socket_path = temp_socket_path("peer_credentials");
    let _ = std::fs::remove_file(&socket_path);
    let captured = Arc::new(Mutex::new(None));
    let handler = Arc::new(CredentialHandler(Arc::clone(&captured)));
    let shutdown = Arc::new(AtomicBool::new(false));
    let server_shutdown = Arc::clone(&shutdown);
    let server_socket = socket_path.clone();
    let server_thread = thread::spawn(move || {
        run_server(&server_socket, handler, server_shutdown, IPC_TIMEOUT).expect("server");
    });
    assert!(wait_for_socket(&socket_path, Duration::from_secs(2)));

    let mut stream = UnixStream::connect(&socket_path).expect("connect");
    write_frame(&mut stream, &svd_proto::Request::Status {}).expect("write");
    read_frame(&mut stream).expect("read");

    let context = captured
        .lock()
        .expect("credential lock")
        .expect("credentials captured");
    let process_metadata = std::fs::metadata("/proc/self").expect("process metadata");
    assert_eq!(context.peer.pid, std::process::id());
    assert_eq!(context.peer.uid, process_metadata.uid());
    assert_eq!(context.peer.gid, process_metadata.gid());

    shutdown.store(true, Ordering::Relaxed);
    server_thread.join().expect("join");
    let _ = std::fs::remove_file(&socket_path);
}
