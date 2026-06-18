// Integration tests for the `svd` binary — T2.2 / T4.9.
//
// Uses `CARGO_BIN_EXE_svd` (set by Cargo for integration test targets) so
// tests always run against the freshly-built binary.
//
// Exit-code conventions (intentional — see main.rs design notes):
//   0     → success
//   1     → semantic validation failure (validate_request returned Err)
//             OR daemon not reachable (transport error)
//   2     → clap usage / parse error (missing required args, unknown flags, …)
//   non-zero → anything that is not success (covers both 1 and 2)
//
// NOTE (T4.9): Tests that previously exercised the stub transport (exit 0 +
// request JSON output) have been updated to reflect the real transport
// behaviour: when no daemon is listening at /run/sunshine-vd/svd.sock, valid
// commands exit 1 with "daemon not running" on stderr.  Success-path coverage
// requires a live daemon and is not provided by this test harness.

use std::process::Command;

fn svd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_svd"))
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn run(args: &[&str]) -> std::process::Output {
    svd().args(args).output().expect("failed to run svd binary")
}

// ── Test 1 ────────────────────────────────────────────────────────────────────
// `svd connect --width 1920 --height 1080 --refresh 60`
// → validation passes; without a live daemon the transport fails with exit 1
//   and "daemon not running" on stderr.
#[test]
fn connect_valid_no_daemon_exits_1() {
    let out = run(&[
        "connect",
        "--width",
        "1920",
        "--height",
        "1080",
        "--refresh",
        "60",
    ]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1 (no daemon), got {:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("daemon not running"),
        "expected 'daemon not running' in stderr, got: {stderr}"
    );
}

// ── Test 2 ────────────────────────────────────────────────────────────────────
// `svd connect --width 0 --height 1080 --refresh 60`
// → exits 1 (out_of_range from validate_request, not clap)
#[test]
fn connect_width_zero_exits_1() {
    let out = run(&[
        "connect",
        "--width",
        "0",
        "--height",
        "1080",
        "--refresh",
        "60",
    ]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1, got {:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("out_of_range"),
        "expected 'out_of_range' in stderr, got: {stderr}"
    );
}

// ── Test 3 ────────────────────────────────────────────────────────────────────
// `svd connect --width 1920 --height 1080 --refresh 999`
// → exits 1 (out_of_range)
#[test]
fn connect_refresh_999_exits_1() {
    let out = run(&[
        "connect",
        "--width",
        "1920",
        "--height",
        "1080",
        "--refresh",
        "999",
    ]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1, got {:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("out_of_range"),
        "expected 'out_of_range' in stderr, got: {stderr}"
    );
}

// ── Test 4 ────────────────────────────────────────────────────────────────────
// `svd connect --width 1920 --height 1080 --refresh 60 --device card1`
// → validation passes; without a live daemon exits 1 with "daemon not running"
#[test]
fn connect_with_valid_device_no_daemon_exits_1() {
    let out = run(&[
        "connect",
        "--width",
        "1920",
        "--height",
        "1080",
        "--refresh",
        "60",
        "--device",
        "card1",
    ]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1 (no daemon), got {:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("daemon not running"),
        "expected 'daemon not running' in stderr, got: {stderr}"
    );
}

// ── Test 5 ────────────────────────────────────────────────────────────────────
// `svd connect --width 1920 --height 1080 --refresh 60 --device ../bad`
// → exits 1 (invalid device: path traversal)
#[test]
fn connect_with_path_traversal_device_exits_1() {
    let out = run(&[
        "connect",
        "--width",
        "1920",
        "--height",
        "1080",
        "--refresh",
        "60",
        "--device",
        "../bad",
    ]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1, got {:?}",
        out.status.code()
    );
}

// ── Test 6 ────────────────────────────────────────────────────────────────────
// `svd disconnect` → validation passes; without a live daemon exits 1
#[test]
fn disconnect_no_daemon_exits_1() {
    let out = run(&["disconnect"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1 (no daemon), got {:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("daemon not running"),
        "expected 'daemon not running' in stderr, got: {stderr}"
    );
}

// ── Test 7 ────────────────────────────────────────────────────────────────────
// `svd status` → validation passes; without a live daemon exits 1
#[test]
fn status_no_daemon_exits_1() {
    let out = run(&["status"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1 (no daemon), got {:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("daemon not running"),
        "expected 'daemon not running' in stderr, got: {stderr}"
    );
}

// ── Test 8 ────────────────────────────────────────────────────────────────────
// `svd restore` → validation passes; without a live daemon exits 1
#[test]
fn restore_no_daemon_exits_1() {
    let out = run(&["restore"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1 (no daemon), got {:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("daemon not running"),
        "expected 'daemon not running' in stderr, got: {stderr}"
    );
}

// ── Test 9 ────────────────────────────────────────────────────────────────────
// `svd --help` → exits 0
#[test]
fn help_flag_exits_0() {
    let out = run(&["--help"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "expected exit 0, got {:?}",
        out.status.code()
    );
}

// ── Test 10 ───────────────────────────────────────────────────────────────────
// `svd connect` (missing required --width, --height, --refresh)
// → exits non-zero (clap usage error, exit code 2)
#[test]
fn connect_missing_required_args_exits_nonzero() {
    let out = run(&["connect"]);
    assert!(
        !out.status.success(),
        "expected non-zero exit for missing required args, got 0"
    );
}

// ── Bonus: mode in-range but not on VIC allowlist → CLI passes it to daemon ──
// 1024×768@60 is within numeric bounds; the CLI no longer checks the allowlist
// (that responsibility is the daemon's, which knows extra_allowed_modes from its
// config). With no daemon running the CLI exits 1 with "daemon not running".
#[test]
fn connect_off_allowlist_mode_reaches_daemon() {
    let out = run(&[
        "connect",
        "--width",
        "1024",
        "--height",
        "768",
        "--refresh",
        "60",
    ]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1 (daemon not running), got {:?}",
        out.status.code()
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    // The CLI must NOT reject this locally — mode allowlist is a daemon concern.
    assert!(
        !stderr.contains("mode_not_allowed"),
        "CLI must not reject off-allowlist mode locally, got: {stderr}"
    );
    assert!(
        stderr.contains("daemon not running"),
        "expected 'daemon not running' in stderr, got: {stderr}"
    );
}

// ── Bonus: --json flag is accepted (present per acceptance criterion 2) ───────
// With no daemon running, the transport fails before JSON output; we only check
// that the flag is parsed correctly (no clap error / exit 2).
#[test]
fn json_flag_accepted_no_daemon() {
    let out = run(&["status", "--json"]);
    // Exit 1 (daemon not running) is expected — what we exclude is exit 2 (clap error).
    assert_ne!(
        out.status.code(),
        Some(2),
        "expected exit 1 (no daemon), not exit 2 (clap parse error)"
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1 (no daemon), got {:?}",
        out.status.code()
    );
}

// ── Bonus: connect --dry-run skips transport and exits 0 ─────────────────────
#[test]
fn connect_dry_run_exits_0_with_dry_run_ok() {
    let out = run(&[
        "connect",
        "--width",
        "1920",
        "--height",
        "1080",
        "--refresh",
        "60",
        "--dry-run",
    ]);
    assert_eq!(out.status.code(), Some(0), "expected exit 0 with --dry-run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Dry run OK"),
        "expected 'Dry run OK' in stdout, got: {stdout}"
    );
}
