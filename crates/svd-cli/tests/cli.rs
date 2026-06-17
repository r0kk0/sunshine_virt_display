// Integration tests for the `svd` binary — T2.2.
//
// Uses `CARGO_BIN_EXE_svd` (set by Cargo for integration test targets) so
// tests always run against the freshly-built binary.
//
// Exit-code conventions (intentional — see main.rs design notes):
//   0     → success
//   1     → semantic validation failure (validate_request returned Err)
//   2     → clap usage / parse error (missing required args, unknown flags, …)
//   non-zero → anything that is not success (covers both 1 and 2)

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
// → exits 0, stdout contains the JSON tag "connect"
#[test]
fn connect_valid_exits_0_and_json_contains_connect() {
    let out = run(&["connect", "--width", "1920", "--height", "1080", "--refresh", "60"]);
    assert_eq!(out.status.code(), Some(0), "expected exit 0, got {:?}", out.status.code());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"connect\""),
        "expected stdout to contain '\"connect\"', got: {stdout}"
    );
}

// ── Test 2 ────────────────────────────────────────────────────────────────────
// `svd connect --width 0 --height 1080 --refresh 60`
// → exits 1 (out_of_range from validate_request, not clap)
#[test]
fn connect_width_zero_exits_1() {
    let out = run(&["connect", "--width", "0", "--height", "1080", "--refresh", "60"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1, got {:?}", out.status.code());
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
    let out = run(&["connect", "--width", "1920", "--height", "1080", "--refresh", "999"]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1, got {:?}", out.status.code());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("out_of_range"),
        "expected 'out_of_range' in stderr, got: {stderr}"
    );
}

// ── Test 4 ────────────────────────────────────────────────────────────────────
// `svd connect --width 1920 --height 1080 --refresh 60 --device card1`
// → exits 0
#[test]
fn connect_with_valid_device_exits_0() {
    let out = run(&[
        "connect", "--width", "1920", "--height", "1080", "--refresh", "60", "--device", "card1",
    ]);
    assert_eq!(out.status.code(), Some(0), "expected exit 0, got {:?}", out.status.code());
}

// ── Test 5 ────────────────────────────────────────────────────────────────────
// `svd connect --width 1920 --height 1080 --refresh 60 --device ../bad`
// → exits 1 (invalid device: path traversal)
#[test]
fn connect_with_path_traversal_device_exits_1() {
    let out = run(&[
        "connect", "--width", "1920", "--height", "1080", "--refresh", "60", "--device", "../bad",
    ]);
    assert_eq!(out.status.code(), Some(1), "expected exit 1, got {:?}", out.status.code());
}

// ── Test 6 ────────────────────────────────────────────────────────────────────
// `svd disconnect` → exits 0
#[test]
fn disconnect_exits_0() {
    let out = run(&["disconnect"]);
    assert_eq!(out.status.code(), Some(0), "expected exit 0, got {:?}", out.status.code());
}

// ── Test 7 ────────────────────────────────────────────────────────────────────
// `svd status` → exits 0
#[test]
fn status_exits_0() {
    let out = run(&["status"]);
    assert_eq!(out.status.code(), Some(0), "expected exit 0, got {:?}", out.status.code());
}

// ── Test 8 ────────────────────────────────────────────────────────────────────
// `svd restore` → exits 0
#[test]
fn restore_exits_0() {
    let out = run(&["restore"]);
    assert_eq!(out.status.code(), Some(0), "expected exit 0, got {:?}", out.status.code());
}

// ── Test 9 ────────────────────────────────────────────────────────────────────
// `svd --help` → exits 0
#[test]
fn help_flag_exits_0() {
    let out = run(&["--help"]);
    assert_eq!(out.status.code(), Some(0), "expected exit 0, got {:?}", out.status.code());
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

// ── Bonus: --json flag is accepted (present per acceptance criterion 2) ───────
#[test]
fn json_flag_accepted_on_status() {
    let out = run(&["status", "--json"]);
    assert_eq!(out.status.code(), Some(0), "expected exit 0 with --json, got {:?}", out.status.code());
}

// ── Bonus: connect --dry-run is accepted ──────────────────────────────────────
#[test]
fn connect_dry_run_flag_accepted() {
    let out = run(&[
        "connect", "--width", "1920", "--height", "1080", "--refresh", "60", "--dry-run",
    ]);
    assert_eq!(out.status.code(), Some(0), "expected exit 0 with --dry-run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("true"),
        "expected dry_run:true in JSON, got: {stdout}"
    );
}
