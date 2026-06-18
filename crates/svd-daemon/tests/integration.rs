//! Integration test: socket round-trip for Status request.
//!
//! Starts an in-process svd-daemon IPC server on a temp socket, connects to it
//! directly with a UnixStream, sends a `Request::Status {}` via write_frame,
//! reads the response via read_frame, and verifies it is
//! `Response::Status { ok: true, connected: false, .. }`.

use std::{
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use svd_daemon::ipc::{read_frame, run_server, write_frame, StubHandler};

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
fn wait_for_socket(path: &PathBuf, timeout: Duration) -> bool {
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
        run_server(&server_socket, handler, server_shutdown)
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
