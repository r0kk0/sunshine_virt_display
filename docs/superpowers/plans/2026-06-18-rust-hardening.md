# Rust Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a secure, recoverable Rust-only v0.2.0 for KWin/Wayland with reviewable commits and a documented migration.

**Architecture:** Keep a small privileged daemon for validated IPC and DRM/sysfs operations. Authenticate clients with Unix peer credentials, execute KWin-facing commands as the desktop user, and coordinate display mutations through a serialized, journaled lifecycle that can roll back or recover after interruption.

**Tech Stack:** Rust stable, serde/TOML, Unix domain sockets, Linux peer credentials, systemd, KWin/kscreen-doctor, GitHub Actions.

---

### Task 1: Documentation and clean Rust baseline

**Files:** `.gitignore`, `AGENTS.md`, Rust workspace sources, `.github/workflows/rust.yml`

- [ ] Commit the worktree ignore rule separately.
- [ ] Commit `AGENTS.md` separately.
- [ ] Run `cargo fmt --all`, verify with `cargo fmt --all -- --check`, and commit formatting only.
- [ ] Fix every Clippy warning without changing behavior; verify with `cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] Extend Rust CI with fmt, Clippy, and RustSec audit checks; keep build, tests, and binary smoke tests.

### Task 2: Configuration cleanup

**Files:** `crates/svd-daemon/src/config.rs`, daemon startup, `deploy/config.toml.example`

- [ ] Write tests that reject removed keys and invalid timeout, device, connector, and mode values.
- [ ] Remove unused `hdr`, `allow_master_stealing`, `socket_path`, and `state_path` settings.
- [ ] Keep and enforce `device`, `log_level`, `extra_allowed_modes`, `output_ready_timeout_secs`, `ipc_timeout_secs`, and `disable_outputs`.
- [ ] Use fixed runtime paths `/run/sunshine-vd/svd.sock` and `/var/lib/sunshine-vd/state.json`.
- [ ] Load configuration before tracing so `RUST_LOG`, `--verbose`, and `log_level` have deterministic precedence.

### Task 3: IPC authorization and denial-of-service resistance

**Files:** daemon IPC server and integration tests, daemon manifest

- [ ] Write failing tests for socket mode `0660`, refusal to remove a stale regular file, peer credentials, and a stalled client that must not block the next request indefinitely.
- [ ] Add a `PeerCredentials` request context populated from `SO_PEERCRED`.
- [ ] Set bounded read/write timeouts from validated configuration.
- [ ] Remove stale endpoints only when metadata proves the path is a Unix socket; otherwise fail closed.
- [ ] Authorize mutating requests only for root or the UID that owns the selected KWin session; retain group permissions as the first kernel-enforced boundary.

### Task 4: Privileged-operation boundaries

**Files:** strategy interfaces, KWin environment/command modules, handler

- [ ] Write tests for validated `CardId` and `ConnectorId` newtypes, including deserialization and command-argument rejection.
- [ ] Make `RealHandler` depend on `Arc<dyn DisplayStrategy>`.
- [ ] Separate DRM/sysfs operations, KWin session operations, and state persistence behind internal traits with production adapters.
- [ ] Resolve KWin for the authenticated UID, validate its executable and `/run/user/<uid>` environment, and reject ambiguous sessions.
- [ ] Run absolute-path KWin helpers with the session UID/GID and a minimal environment rather than as root.

### Task 5: Versioned recovery journal

**Files:** KWin state module and tests

- [ ] Write round-trip and malformed-state tests for schema version, lifecycle phase, validated identifiers, requested mode, session UID, and previous layout.
- [ ] Persist `Connecting`, `Connected`, `Disconnecting`, and `RecoveryRequired` records atomically with mode `0600`.
- [ ] Sync journal data before rename and preserve recovery data until cleanup has been verified.
- [ ] Reject unsupported schema versions and unsafe state-file ownership or permissions.

### Task 6: Transactional display lifecycle

**Files:** KWin strategy coordinator and fault-injection tests

- [ ] Write a test backend that records side effects and can fail at each operation boundary.
- [ ] Serialize connect, disconnect, restore, watcher, and sleep transitions with one operation lock and observable lifecycle phase.
- [ ] Save recovery intent before EDID/sysfs/KWin mutation.
- [ ] Require successful KWin mode application and output verification before reporting connect success.
- [ ] On failure, restore physical layout before disabling the virtual output and clearing EDID; delete the journal only after verified cleanup.
- [ ] On startup, recover incomplete phases idempotently; missing/unplugged physical outputs are warnings, while remaining connected outputs must be restored.

### Task 7: Watcher and shutdown correctness

**Files:** Sunshine watcher, daemon lifecycle, protocol status types

- [ ] Write tests proving a stale watcher cannot disconnect a later session and shutdown waits for serialized cleanup.
- [ ] Bind Sunshine discovery to the authenticated request PID ancestry and session UID instead of the first process named `sunshine`.
- [ ] Use generation-based cancellation and owned file descriptors; handle thread-spawn and poll errors without panicking or spinning.
- [ ] Add lifecycle `phase` to status output.
- [ ] On SIGTERM/SIGINT stop accepting work, cancel the active connect if needed, and run disconnect before process exit.

### Task 8: Packaging and systemd hardening

**Files:** `install.sh`, systemd service, deployment tests/checks

- [ ] Write shell-level argument tests for explicit `--user`, `--no-user`, default release build, and `--debug`.
- [ ] Provision the `sunshine-vd` system group idempotently and install the socket as `root:sunshine-vd` mode `0660`.
- [ ] Add `RuntimeDirectory` and `StateDirectory` ownership/modes.
- [ ] Bound capabilities to those verified as required, enable `NoNewPrivileges`, filesystem/network restrictions, and safe process hardening compatible with DRM and D-Bus.
- [ ] Verify the unit with `systemd-analyze verify` and record the remaining intentional exposure.

### Task 9: Remove legacy Python and publish migration

**Files:** legacy Python implementation/tests/workflow, README, contributor guide, crate manifests

- [ ] Port only still-relevant behavioral assertions to Rust tests.
- [ ] Remove `main.py`, runtime `src/`, Python tests, Python CI, and Python Makefile service targets; retain the standalone diagnostic script.
- [ ] Set workspace crate versions to `0.2.0`.
- [ ] Document the new install command, group-login requirement, removed config keys, lifecycle phases, recovery procedure, and KWin-only scope.
- [ ] Run the complete verification suite and create a final documentation-only commit.

### Commit policy

Every commit uses an English Conventional Commit title and a body with `Why`, `Behavior`, and `Verification`. Formatting, refactoring, security behavior, packaging, and documentation remain separate. Before each commit run the focused test plus `cargo test --workspace`; before completion run fmt, Clippy, all tests, RustSec audit, and systemd verification.
